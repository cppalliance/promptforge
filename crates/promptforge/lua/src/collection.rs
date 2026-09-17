//! The fanout collection conversion at the protocol boundary.
//!
//! A section's Lua calls `fanout(worker, collection)`; the collection is any
//! Lua table and crosses into the arms as JSON members, converted here one
//! value at a time. The array part (`1..=#t`) iterates in order first, then
//! the hash part in undefined order. An array member arrives as the arm's
//! `item` value as itself; a hash member arrives as a pair table
//! (`item.key` / `item.value`).

use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;

use crate::error::{Error, Result};

/// Converts fanout's collection argument into the JSON members that cross
/// into the arms, one value at a time.
///
/// The array part (`1..=#t`) iterates in order first, then the hash part in
/// undefined order. Array members convert as themselves; hash members convert
/// to `{"key": k, "value": v}` pair tables so no information is lost. Each
/// member converts individually through the same serde bridge that seeds
/// `var`, because whole-table serde cannot represent mixed tables.
///
/// # Errors
/// Returns [`Error::Lua`] when the value is not a table (the message points
/// at `list_from_section` for the list-section case), when a member is a
/// function, userdata, or thread (the error names the member's index), or
/// when a hash key is not a string, number, or boolean.
pub(crate) fn collection_to_items(lua: &Lua, collection: &Value) -> Result<Vec<serde_json::Value>> {
    let Value::Table(table) = collection else {
        return Err(Error::Lua(
            "fanout's second parameter is a collection; for a list section use list_from_section(heading)".to_owned(),
        ));
    };
    let mut items = Vec::new();
    let border = table.raw_len();
    for index in 1..=border {
        let member = table.raw_get::<Value>(index).map_err(Error::lua)?;
        items.push(member_to_json(lua, member, &index.to_string())?);
    }
    for pair in table.pairs::<Value, Value>() {
        let (key, member) = pair.map_err(Error::lua)?;
        // The array part was already emitted above, in order.
        if let Value::Integer(index) = &key
            && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
        {
            continue;
        }
        // Each scalar key converts to its JSON form and its diagnostic label
        // in one match; non-scalar keys are rejected here, so no later code
        // path can meet one.
        let (key_json, key_label) = match &key {
            Value::String(s) => {
                let s = s.to_str().map_err(Error::lua)?;
                (serde_json::Value::String(s.to_owned()), s.to_owned())
            }
            Value::Integer(i) => (serde_json::Value::from(*i), i.to_string()),
            Value::Number(n) => (
                serde_json::Number::from_f64(*n)
                    .map(serde_json::Value::Number)
                    .ok_or_else(|| {
                        Error::Lua("fanout collection key is not a finite number".to_owned())
                    })?,
                n.to_string(),
            ),
            Value::Boolean(b) => (serde_json::Value::Bool(*b), b.to_string()),
            other => {
                return Err(Error::Lua(format!(
                    "fanout collection key must be a string, number, or boolean, got {}",
                    other.type_name()
                )));
            }
        };
        let value_json = member_to_json(lua, member, &key_label)?;
        items.push(json!({ "key": key_json, "value": value_json }));
    }
    Ok(items)
}

/// Converts one collection member to JSON through the serde bridge.
///
/// Functions, userdata, and threads cannot serialize, so they are rejected at
/// the call boundary with an error naming the member's index rather than the
/// bridge's type error.
fn member_to_json(lua: &Lua, member: Value, index: &str) -> Result<serde_json::Value> {
    match &member {
        Value::Function(_) | Value::UserData(_) | Value::Thread(_) => Err(Error::Lua(format!(
            "fanout collection member at index {index} is a {}; members must be data",
            member.type_name()
        ))),
        _ => lua.from_value(member).map_err(Error::lua),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn eval(lua: &mlua::Lua, source: &str) -> Value {
        lua.load(source).eval::<Value>().expect("chunk evaluates")
    }

    #[test]
    fn collection_to_items_rejects_a_non_table() {
        let lua = mlua::Lua::new();
        for source in ["return '### Items'", "return 5", "return true"] {
            let value = eval(&lua, source);
            let error =
                collection_to_items(&lua, &value).expect_err("a non-table is not a collection");
            assert!(
                error.to_string().contains("list_from_section"),
                "the error must point at list_from_section for {source}: {error}"
            );
        }
    }

    #[test]
    fn collection_to_items_preserves_array_order_and_member_types() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {'b', 2, true, {nested='x'}}");
        let items = collection_to_items(&lua, &value).expect("a mixed array converts");
        assert_eq!(
            items,
            vec![json!("b"), json!(2), json!(true), json!({"nested": "x"})]
        );
    }

    #[test]
    fn collection_to_items_wraps_hash_members_as_pair_tables() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {alpha=1, beta='two'}");
        let mut items = collection_to_items(&lua, &value).expect("a hash table converts");
        // The hash part's order is undefined; sort for the comparison.
        items.sort_by_key(ToString::to_string);
        assert_eq!(
            items,
            vec![
                json!({"key": "alpha", "value": 1}),
                json!({"key": "beta", "value": "two"})
            ]
        );
    }

    #[test]
    fn collection_to_items_emits_the_array_part_before_the_hash_part() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {'a', 'b', extra='c'}");
        let items = collection_to_items(&lua, &value).expect("a mixed table converts");
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
    fn collection_to_items_keeps_integer_keys_outside_the_border_as_pairs() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {[5]='five'}");
        let items = collection_to_items(&lua, &value).expect("a sparse table converts");
        assert_eq!(items, vec![json!({"key": 5, "value": "five"})]);
    }

    #[test]
    fn collection_to_items_returns_an_empty_vec_for_an_empty_table() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {}");
        let items = collection_to_items(&lua, &value).expect("an empty table converts");
        assert!(items.is_empty());
    }

    #[test]
    fn collection_to_items_rejects_a_function_member_naming_its_index() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "return {'a', function() end}");
        let error = collection_to_items(&lua, &value).expect_err("a function member must error");
        let rendered = error.to_string();
        assert!(rendered.contains("index 2"), "error was: {rendered}");
        assert!(rendered.contains("function"), "error was: {rendered}");

        let value = eval(&lua, "return {cb=function() end}");
        let error = collection_to_items(&lua, &value)
            .expect_err("a hash-position function member must error");
        let rendered = error.to_string();
        assert!(rendered.contains("index cb"), "error was: {rendered}");
        assert!(rendered.contains("function"), "error was: {rendered}");
    }

    struct Stub;
    impl mlua::UserData for Stub {}

    #[test]
    fn collection_to_items_rejects_a_userdata_member_naming_its_index() {
        let lua = mlua::Lua::new();
        let userdata = lua.create_userdata(Stub).expect("userdata creates");
        let table = lua.create_table().expect("table creates");
        table.raw_set(1, userdata).expect("member installs");
        let error = collection_to_items(&lua, &Value::Table(table))
            .expect_err("a userdata member must error");
        let rendered = error.to_string();
        assert!(rendered.contains("index 1"), "error was: {rendered}");
        assert!(rendered.contains("userdata"), "error was: {rendered}");
    }

    #[test]
    fn collection_to_items_rejects_a_non_scalar_key() {
        let lua = mlua::Lua::new();
        let value = eval(&lua, "local t = {}; t[{}] = 'x'; return t");
        let error = collection_to_items(&lua, &value).expect_err("a table key must error");
        assert!(
            error
                .to_string()
                .contains("key must be a string, number, or boolean"),
            "error was: {error}"
        );
    }
}
