//! Deterministic `pairs`/`next` for section VMs.
//!
//! Lua leaves a table's hash traversal order unspecified, so two runs (or two
//! fresh VMs) can visit the same table differently. [`install_deterministic_iteration`]
//! replaces the `pairs` and `next` globals with one deterministic walk: the
//! array part (`1..=#t`) first in index order, then the hash part by the shared
//! [`SortKey`](crate::collection::SortKey) order (booleans, then numbers, then
//! strings). A `__pairs` metamethod still wins, exactly as in stock Lua.
//!
//! `pairs` captures that order once and advances it by position, so a key
//! whose value becomes `nil` mid-traversal is skipped rather than visited with
//! a nil value, whatever its type: a cleared key is skipped without a lookup
//! that would need it to be sortable. `next` stays stateless and rebuilds the
//! ordered key sequence from the live table on each call; for a key still
//! present it advances strictly, and for a cleared scalar key it resumes at
//! the first key after the cleared key's sort position. A cleared non-scalar
//! key has no cross-process position, so `next` falls back to the trailing
//! segment such keys occupy, the best a stateless resume can do. Rebuilding
//! costs `O(n log n)` per step; section tables are small and the determinism
//! is the point. The ordering covers string, number, and boolean keys, the
//! only keys a payload the run log stores can carry. A non-scalar key (table,
//! function, userdata, or thread) is still visited rather than dropped, but it
//! sits after every scalar key in the order Lua's own `next` produced it: such
//! a key has no cross-process position, and it cannot back a logged value.

use std::cmp::Ordering;

use mlua::{Lua, MultiValue, Table, Value};

use crate::collection::{SortKey, sort_key};
use crate::error::{Error, Result};

/// Installs the deterministic `next` and `pairs` globals, replacing the base
/// library's hash-order versions.
///
/// # Errors
/// Returns [`Error::Lua`] if either global cannot be created or installed.
pub(crate) fn install_deterministic_iteration(lua: &Lua) -> Result<()> {
    let globals = lua.globals();
    let next_fn = lua
        .create_function(deterministic_next)
        .map_err(Error::lua)?;
    globals.raw_set("next", &next_fn).map_err(Error::lua)?;
    let pairs_fn = lua.create_function(pairs).map_err(Error::lua)?;
    globals.raw_set("pairs", pairs_fn).map_err(Error::lua)
}

/// One table key and its position in the deterministic walk order.
struct WalkKey {
    key: Value,
    order: WalkOrder,
}

/// Where a key sits in the walk order: the array segment, the scalar hash
/// segment (by [`SortKey`]), or the trailing non-scalar segment. The segments
/// compare before the inner position, so an array key always precedes a
/// boolean hash key even though [`SortKey`] ranks a boolean below a number;
/// the ordered list and the resumption comparator thus agree. A non-scalar key
/// has no cross-position, so all of them compare equal and keep their raw
/// `pairs` order.
enum WalkOrder {
    Array(i64),
    Scalar(SortKey),
    Unordered,
}

impl WalkOrder {
    /// The segment rank: the array part, then scalar hash keys, then
    /// non-scalar keys. Segments compare before any within-segment position.
    fn segment(&self) -> u8 {
        match self {
            WalkOrder::Array(_) => 0,
            WalkOrder::Scalar(_) => 1,
            WalkOrder::Unordered => 2,
        }
    }

    /// Orders two walk positions by segment, then within the segment. All
    /// non-scalar positions compare equal, so a stable sort keeps their raw
    /// `pairs` order.
    fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (WalkOrder::Array(left), WalkOrder::Array(right)) => left.cmp(right),
            (WalkOrder::Scalar(left), WalkOrder::Scalar(right)) => left.compare(right),
            _ => self.segment().cmp(&other.segment()),
        }
    }
}

/// The `next(table, key)` replacement: the key strictly after `key` in the
/// sorted order, with its value, or `(nil, nil)` at the end.
fn deterministic_next(
    _lua: &Lua,
    (table, previous): (Table, Value),
) -> mlua::Result<(Value, Value)> {
    let keys = ordered_keys(&table).map_err(mlua::Error::external)?;
    let Some(index) = next_index(&keys, &previous, table.raw_len()) else {
        return Ok((Value::Nil, Value::Nil));
    };
    let key = keys[index].key.clone();
    let value: Value = table.raw_get(&key)?;
    Ok((key, value))
}

/// The `pairs(table)` replacement: a `__pairs` metamethod's results when one
/// is present, otherwise a stateful iterator over the table's walk order.
fn pairs(lua: &Lua, value: Value) -> mlua::Result<MultiValue> {
    let table = match &value {
        Value::Table(table) => table.clone(),
        other => {
            return Err(mlua::Error::runtime(format!(
                "bad argument #1 to 'pairs' (table expected, got {})",
                other.type_name()
            )));
        }
    };
    if let Some(metatable) = table.metatable() {
        match metatable.raw_get::<Value>("__pairs")? {
            Value::Nil => {}
            Value::Function(callable) => return callable.call::<MultiValue>(value),
            other => {
                return Err(mlua::Error::runtime(format!(
                    "attempt to call a {} value (metamethod '__pairs')",
                    other.type_name()
                )));
            }
        }
    }
    // Capture the walk order once, then advance it by position. Advancing a
    // snapshot rather than re-deriving order from the live table makes the
    // walk independent of a key's sortability: a key cleared mid-loop reads as
    // nil and is skipped, and no key-based lookup can end the walk early.
    let keys = ordered_keys(&table)?;
    // The VM is single-threaded, so the cursor is a plain `usize` mutated in
    // place rather than an atomic.
    let state = table.clone();
    let mut position = 0usize;
    let iterator = lua.create_function_mut(move |_, ()| {
        while let Some(entry) = keys.get(position) {
            position += 1;
            let value: Value = table.raw_get(&entry.key)?;
            if !matches!(value, Value::Nil) {
                return Ok((entry.key.clone(), value));
            }
        }
        Ok((Value::Nil, Value::Nil))
    })?;
    // Stock `pairs` yields `(next, table, nil)`; the iterator ignores the
    // state, but keeping the shape means a caller reading the second result
    // sees the table, exactly as with the pre-replacement `pairs`.
    let mut iterator_value = MultiValue::new();
    iterator_value.push_back(Value::Function(iterator));
    iterator_value.push_back(Value::Table(state));
    iterator_value.push_back(Value::Nil);
    Ok(iterator_value)
}

/// The table's live keys in deterministic walk order: the array part
/// (`1..=#table`) in index order, then the hash part's scalar keys by
/// [`SortKey`], then any non-scalar keys.
fn ordered_keys(table: &Table) -> mlua::Result<Vec<WalkKey>> {
    let border = table.raw_len();
    let mut keys: Vec<WalkKey> = Vec::new();
    for index in 1..=border {
        let value: Value = table.raw_get(index)?;
        if matches!(value, Value::Nil) {
            continue;
        }
        let index = i64::try_from(index).map_err(|_| {
            mlua::Error::runtime("table index exceeds the integer range".to_owned())
        })?;
        keys.push(WalkKey {
            key: Value::Integer(index),
            order: WalkOrder::Array(index),
        });
    }
    let mut hashed: Vec<WalkKey> = Vec::new();
    let mut unordered: Vec<WalkKey> = Vec::new();
    for pair in table.pairs::<Value, Value>() {
        let (key, value) = pair?;
        if matches!(value, Value::Nil) {
            continue;
        }
        if let Value::Integer(index) = &key
            && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
        {
            continue;
        }
        match sort_key(&key) {
            Ok((position, _)) => hashed.push(WalkKey {
                key,
                order: WalkOrder::Scalar(position),
            }),
            Err(_) => unordered.push(WalkKey {
                key,
                order: WalkOrder::Unordered,
            }),
        }
    }
    hashed.sort_by(|left, right| left.order.compare(&right.order));
    keys.extend(hashed);
    keys.extend(unordered);
    Ok(keys)
}

/// The index in `keys` to return for a `next` call with `previous` as the
/// last key. An exact match advances one slot. A key whose value was cleared
/// (`previous` absent) resumes at the first key that sorts strictly after it,
/// so the cleared key is skipped rather than revisited; a cleared non-scalar
/// key has no computed position, so the resume falls back to the trailing
/// segment, the best a stateless resume can offer.
fn next_index(keys: &[WalkKey], previous: &Value, border: usize) -> Option<usize> {
    if matches!(previous, Value::Nil) {
        return (!keys.is_empty()).then_some(0);
    }
    if let Some(position) = keys.iter().position(|entry| &entry.key == previous) {
        return (position + 1 < keys.len()).then_some(position + 1);
    }
    let order = previous_order(previous, border);
    if matches!(order, WalkOrder::Unordered) {
        return keys
            .iter()
            .position(|entry| matches!(entry.order, WalkOrder::Unordered));
    }
    keys.iter()
        .position(|entry| entry.order.compare(&order) == Ordering::Greater)
}

/// The walk position a lone key would occupy, ignoring whether it is still in
/// the table: an integer inside the array border is an array key, a scalar
/// hash key keeps its [`SortKey`], and anything else is non-scalar.
fn previous_order(previous: &Value, border: usize) -> WalkOrder {
    if let Value::Integer(index) = previous
        && usize::try_from(*index).is_ok_and(|index| (1..=border).contains(&index))
    {
        return WalkOrder::Array(*index);
    }
    match sort_key(previous) {
        Ok((position, _)) => WalkOrder::Scalar(position),
        Err(_) => WalkOrder::Unordered,
    }
}

#[cfg(test)]
#[path = "iteration-tests.rs"]
mod tests;
