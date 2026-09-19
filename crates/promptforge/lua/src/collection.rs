//! The fanout collection's member enumeration and the item renderer, the
//! two captures the `fanout` shim runs over.
//!
//! A section's Lua calls `fanout(worker, collection)`; the collection is any
//! Lua table, and the shim spawns one arm per member in a fixed order. The
//! array part (`1..=#t`) comes first, in order; the hash part follows as
//! `{ key, value }` pair tables sorted by key, because Lua's `pairs` order
//! depends on the string hash seed and a fanout's arm order must not. An
//! array member arrives as the arm's `item` value as itself; a hash member
//! arrives as the pair table (`item.key` / `item.value`).

use std::cmp::Ordering;

use mlua::{Lua, LuaSerdeExt, Table, Value};

use crate::error::{Error, Result};

/// A hash key's sort position: booleans first (`false` before `true`), then
/// numbers by value, then strings bytewise. The ranks keep mixed-type keys
/// totally ordered without inventing a cross-type comparison. An integer
/// key stays an `i64` so two distinct integers past 2^53 never compare
/// equal (which would leave their order to `pairs`, the nondeterminism the
/// sort exists to remove); only a mixed integer/float pair converts.
enum SortKey {
    Bool(bool),
    Integer(i64),
    Float(f64),
    Text(Vec<u8>),
}

impl SortKey {
    fn rank(&self) -> u8 {
        match self {
            SortKey::Bool(_) => 0,
            SortKey::Integer(_) | SortKey::Float(_) => 1,
            SortKey::Text(_) => 2,
        }
    }

    fn compare(&self, other: &SortKey) -> Ordering {
        match (self, other) {
            (SortKey::Bool(left), SortKey::Bool(right)) => left.cmp(right),
            (SortKey::Integer(left), SortKey::Integer(right)) => left.cmp(right),
            (SortKey::Float(left), SortKey::Float(right)) => left.total_cmp(right),
            (SortKey::Integer(integer), SortKey::Float(float)) => {
                compare_integer_float(*integer, *float)
            }
            (SortKey::Float(float), SortKey::Integer(integer)) => {
                compare_integer_float(*integer, *float).reverse()
            }
            (SortKey::Text(left), SortKey::Text(right)) => left.cmp(right),
            _ => self.rank().cmp(&other.rank()),
        }
    }
}

/// Orders an integer key against a finite float key exactly: the float is
/// compared to the integer's neighborhood without rounding the integer,
/// so an integer past 2^53 still sorts on the correct side of a nearby
/// float. A float outside `i64`'s range is beyond every integer; a float
/// inside it is truncated, the integer parts are compared, and a tie is
/// broken by the float's fractional part (an exact integer-valued float
/// ties with its integer).
fn compare_integer_float(integer: i64, float: f64) -> Ordering {
    /// 2^63: one past `i64::MAX`, exactly representable, so a float at or
    /// beyond it is greater than every integer.
    const ABOVE_MAX: f64 = 9_223_372_036_854_775_808.0;
    /// -2^63: exactly `i64::MIN`, so a float below it is less than every
    /// integer.
    const MIN: f64 = -9_223_372_036_854_775_808.0;
    if float >= ABOVE_MAX {
        return Ordering::Less;
    }
    if float < MIN {
        return Ordering::Greater;
    }
    // In range and finite: the truncation is exact for the integer part.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the float is inside i64's range and its fractional part is compared separately"
    )]
    let truncated = float.trunc() as i64;
    match integer.cmp(&truncated) {
        Ordering::Equal => {
            // The integer equals the float's integer part, so it sits below
            // a float with a positive fraction and above one with a
            // negative fraction.
            let fraction = float - float.trunc();
            if fraction > 0.0 {
                Ordering::Less
            } else if fraction < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        ordering => ordering,
    }
}

/// Enumerates fanout's collection argument as the sequence of members the
/// shim spawns arms over: the array part (`1..=#t`) in order, then the hash
/// part as `{ key = k, value = v }` pair tables sorted by key. Members stay
/// Lua values; the `spawn` request converts each one at its own boundary.
///
/// # Errors
/// Returns [`Error::Lua`] when the value is not a table (the message points
/// at `list_from_section` for the list-section case), when a member is a
/// function, userdata, or thread (the error names the member's index), or
/// when a hash key is not a string, number, or boolean.
pub(crate) fn collection_members(lua: &Lua, collection: &Value) -> Result<Table> {
    let Value::Table(table) = collection else {
        return Err(Error::Lua(
            "fanout's second parameter is a collection; for a list section use list_from_section(heading)".to_owned(),
        ));
    };
    let members = lua.create_table().map_err(Error::lua)?;
    let border = table.raw_len();
    for index in 1..=border {
        let member = table.raw_get::<Value>(index).map_err(Error::lua)?;
        check_member(&member, &index.to_string())?;
        members.raw_set(index, member).map_err(Error::lua)?;
    }
    let mut pairs: Vec<(SortKey, Value, Value)> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, member) = pair.map_err(Error::lua)?;
        // The array part was already emitted above, in order.
        if let Value::Integer(index) = &key
            && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
        {
            continue;
        }
        // Each scalar key yields its sort position and its diagnostic label
        // in one match; non-scalar keys are rejected here, so no later code
        // path can meet one.
        let (sort_key, key_label) = match &key {
            Value::String(s) => {
                let text = s.to_str().map_err(Error::lua)?;
                (SortKey::Text(s.as_bytes().to_vec()), text.to_owned())
            }
            Value::Integer(i) => (SortKey::Integer(*i), i.to_string()),
            Value::Number(n) => {
                if !n.is_finite() {
                    return Err(Error::Lua(
                        "fanout collection key is not a finite number".to_owned(),
                    ));
                }
                (SortKey::Float(*n), n.to_string())
            }
            Value::Boolean(b) => (SortKey::Bool(*b), b.to_string()),
            other => {
                return Err(Error::Lua(format!(
                    "fanout collection key must be a string, number, or boolean, got {}",
                    other.type_name()
                )));
            }
        };
        check_member(&member, &key_label)?;
        pairs.push((sort_key, key, member));
    }
    pairs.sort_by(|left, right| left.0.compare(&right.0));
    for (position, (_, key, member)) in pairs.into_iter().enumerate() {
        let entry = lua.create_table().map_err(Error::lua)?;
        entry.raw_set("key", key).map_err(Error::lua)?;
        entry.raw_set("value", member).map_err(Error::lua)?;
        members
            .raw_set(border + position + 1, entry)
            .map_err(Error::lua)?;
    }
    Ok(members)
}

/// Rejects a member that cannot cross into an arm.
///
/// Functions, userdata, and threads cannot serialize, so they are rejected at
/// the call boundary with an error naming the member's index rather than the
/// spawn's own type error.
fn check_member(member: &Value, index: &str) -> Result<()> {
    match member {
        Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
            "fanout collection member at index {index} is a {}; members must be data",
            member.type_name()
        ))),
        _ => Ok(()),
    }
}

/// Renders a fanout arm's item for prose substitution and stub text:
/// strings verbatim, numbers and booleans in their natural string form,
/// arrays and objects as compact JSON.
#[must_use]
pub fn render_item(item: &serde_json::Value) -> String {
    match item {
        serde_json::Value::String(value) => value.clone(),
        serde_json::Value::Bool(value) => value.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        // Serializing a `Value` cannot fail; the default is unreachable.
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            serde_json::to_string(item).unwrap_or_default()
        }
    }
}

/// Renders a Lua member value as [`render_item`] renders its JSON form: the
/// `fanout` shim's capture for the tool-loop-exhausted stub, so the stub's
/// heading reads exactly as `{{ item }}` would render the same member.
///
/// # Errors
/// Returns [`Error::Lua`] when the value has no JSON form.
pub(crate) fn render_item_value(lua: &Lua, item: Value) -> Result<String> {
    let json: serde_json::Value = lua.from_value(item).map_err(Error::lua)?;
    Ok(render_item(&json))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn eval(lua: &mlua::Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk evaluates")
    }

    /// Reads the member sequence back as JSON for comparison.
    fn members_json(lua: &Lua, source: &str) -> Vec<serde_json::Value> {
        let value = eval(lua, source);
        let members = collection_members(lua, &value).expect("the collection enumerates");
        lua.from_value(Value::Table(members))
            .expect("the members are JSON data")
    }

    #[test]
    fn collection_members_rejects_a_non_table() {
        let lua = mlua::Lua::new();
        for source in ["return '### Items'", "return 5", "return true"] {
            let value = eval(&lua, source);
            let error =
                collection_members(&lua, &value).expect_err("a non-table is not a collection");
            assert!(
                error.to_string().contains("list_from_section"),
                "the error must point at list_from_section for {source}: {error}"
            );
        }
    }

    #[test]
    fn collection_members_preserves_array_order_and_member_types() {
        let lua = mlua::Lua::new();
        let items = members_json(&lua, "return {'b', 2, true, {nested='x'}}");
        assert_eq!(
            items,
            vec![json!("b"), json!(2), json!(true), json!({"nested": "x"})]
        );
    }

    #[test]
    fn collection_members_sorts_the_hash_part_by_key() {
        let lua = mlua::Lua::new();
        // Five string keys: `pairs` order varies per state, the members do not.
        let items = members_json(
            &lua,
            "return {zeta=1, alpha='two', mid=true, beta=4, omega=5}",
        );
        assert_eq!(
            items,
            vec![
                json!({"key": "alpha", "value": "two"}),
                json!({"key": "beta", "value": 4}),
                json!({"key": "mid", "value": true}),
                json!({"key": "omega", "value": 5}),
                json!({"key": "zeta", "value": 1}),
            ]
        );
    }

    #[test]
    fn collection_members_orders_mixed_keys_booleans_then_numbers_then_strings() {
        let lua = mlua::Lua::new();
        let items = members_json(
            &lua,
            "return {[true]='t', [7]='seven', b='bee', [false]='f', [2.5]='half', a='ay'}",
        );
        assert_eq!(
            items,
            vec![
                json!({"key": false, "value": "f"}),
                json!({"key": true, "value": "t"}),
                json!({"key": 2.5, "value": "half"}),
                json!({"key": 7, "value": "seven"}),
                json!({"key": "a", "value": "ay"}),
                json!({"key": "b", "value": "bee"}),
            ]
        );
    }

    #[test]
    fn collection_members_orders_large_integer_keys_exactly() {
        // 2^53 and 2^53 + 1 round to the same f64: a float-converting sort
        // would tie them and leave their order to `pairs`. Integer keys
        // compare as integers, and a float key sorts against them exactly:
        // one beyond i64's range lands past every integer, one inside it
        // between its integer neighbors.
        let lua = mlua::Lua::new();
        let items = members_json(
            &lua,
            "return {[9007199254740993]='b', [9007199254740992]='a', [1e300]='big', \
             [-1e300]='small', [2.5]='half', [2]='two', [3]='three'}",
        );
        assert_eq!(
            items,
            vec![
                json!({"key": -1e300, "value": "small"}),
                json!({"key": 2, "value": "two"}),
                json!({"key": 2.5, "value": "half"}),
                json!({"key": 3, "value": "three"}),
                json!({"key": 9_007_199_254_740_992_i64, "value": "a"}),
                json!({"key": 9_007_199_254_740_993_i64, "value": "b"}),
                json!({"key": 1e300, "value": "big"}),
            ]
        );
    }

    #[test]
    fn compare_integer_float_orders_without_rounding_the_integer() {
        assert_eq!(compare_integer_float(2, 2.5), Ordering::Less);
        assert_eq!(compare_integer_float(3, 2.5), Ordering::Greater);
        assert_eq!(compare_integer_float(2, 2.0), Ordering::Equal);
        assert_eq!(compare_integer_float(-1, -0.5), Ordering::Less);
        assert_eq!(compare_integer_float(0, -0.5), Ordering::Greater);
        assert_eq!(compare_integer_float(i64::MAX, 1e300), Ordering::Less);
        assert_eq!(compare_integer_float(i64::MIN, -1e300), Ordering::Greater);
        // 2^63 as a float is one past i64::MAX, so the largest integer is
        // still below it; -2^63 is exactly i64::MIN.
        assert_eq!(
            compare_integer_float(i64::MAX, 9_223_372_036_854_775_808.0),
            Ordering::Less
        );
        assert_eq!(
            compare_integer_float(i64::MIN, -9_223_372_036_854_775_808.0),
            Ordering::Equal
        );
    }

    #[test]
    fn collection_members_emits_the_array_part_before_the_hash_part() {
        let lua = mlua::Lua::new();
        let items = members_json(&lua, "return {'a', 'b', extra='c'}");
        assert_eq!(
            items,
            vec![
                json!("a"),
                json!("b"),
                json!({"key": "extra", "value": "c"})
            ]
        );
    }

    #[test]
    fn collection_members_keeps_integer_keys_outside_the_border_as_pairs() {
        let lua = mlua::Lua::new();
        let items = members_json(&lua, "return {[5]='five'}");
        assert_eq!(items, vec![json!({"key": 5, "value": "five"})]);
    }

    #[test]
    fn collection_members_returns_an_empty_sequence_for_an_empty_table() {
        let lua = mlua::Lua::new();
        let items = members_json(&lua, "return {}");
        assert!(items.is_empty());
    }

    #[test]
    fn collection_members_rejects_a_function_member_naming_its_index() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {'a', function() end}");
        let error = collection_members(&lua, &value).expect_err("a function member must error");
        let rendered = error.to_string();
        assert_eq!(
            rendered,
            "fanout collection member at index 2 is a function; members must be data"
        );

        let value = eval(&lua, "return {cb=function() end}");
        let error = collection_members(&lua, &value)
            .expect_err("a hash-position function member must error");
        let rendered = error.to_string();
        assert!(rendered.contains("index cb"), "error was: {rendered}");
        assert!(rendered.contains("function"), "error was: {rendered}");
    }

    struct Stub;
    impl mlua::UserData for Stub {}

    #[test]
    fn collection_members_rejects_a_userdata_member_naming_its_index() {
        let lua = mlua::Lua::new();
        let userdata = lua.create_userdata(Stub).expect("userdata creates");
        let table = lua.create_table().expect("table creates");
        table.raw_set(1, userdata).expect("member installs");
        let error = collection_members(&lua, &Value::Table(table))
            .expect_err("a userdata member must error");
        let rendered = error.to_string();
        assert!(rendered.contains("index 1"), "error was: {rendered}");
        assert!(rendered.contains("userdata"), "error was: {rendered}");
    }

    #[test]
    fn collection_members_rejects_a_non_scalar_key() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "local t = {}; t[{}] = 'x'; return t");
        let error = collection_members(&lua, &value).expect_err("a table key must error");
        assert_eq!(
            error.to_string(),
            "fanout collection key must be a string, number, or boolean, got table"
        );
    }

    #[test]
    fn collection_members_keeps_member_identity() {
        // Members are handed back as the author's own values, not copies:
        // the arm's `item` converts at the spawn boundary, and the result's
        // `.item` is the value the author passed in.
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {{n=3}}");
        let members = collection_members(&lua, &value).expect("the collection enumerates");
        let Value::Table(source) = &value else {
            panic!("the collection is a table");
        };
        let original: Value = source.raw_get(1).expect("the member reads");
        let member: Value = members.raw_get(1).expect("the member reads");
        assert_eq!(member, original, "the member is the author's own table");
    }

    #[test]
    fn render_item_renders_by_type() {
        assert_eq!(render_item(&json!("plain")), "plain");
        assert_eq!(render_item(&json!(7)), "7");
        assert_eq!(render_item(&json!(2.5)), "2.5");
        assert_eq!(render_item(&json!(true)), "true");
        assert_eq!(render_item(&json!([7, "x"])), "[7,\"x\"]");
        assert_eq!(
            render_item(&json!({"key": "alpha", "value": 1})),
            "{\"key\":\"alpha\",\"value\":1}"
        );
    }

    #[test]
    fn render_item_value_renders_a_lua_member_through_its_json_form() {
        let lua = mlua::Lua::new();
        let table = eval(&lua, "return {key='alpha', value=1}");
        assert_eq!(
            render_item_value(&lua, table).expect("a data table renders"),
            "{\"key\":\"alpha\",\"value\":1}"
        );
        let text = eval(&lua, "return 'alpha'");
        assert_eq!(
            render_item_value(&lua, text).expect("a string renders"),
            "alpha"
        );
    }
}
