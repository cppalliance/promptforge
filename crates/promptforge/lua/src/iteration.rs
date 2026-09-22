//! Deterministic `pairs`/`next` for section VMs.
//!
//! Lua leaves a table's hash traversal order unspecified, so two runs (or two
//! fresh VMs) can visit the same table differently. [`install_deterministic_iteration`]
//! replaces the `pairs` and `next` globals with one deterministic walk: the
//! array part (`1..=#t`) first in index order, then the hash part by the shared
//! [`SortKey`](crate::collection::SortKey) order (booleans, then numbers, then
//! strings). A `__pairs` metamethod still wins, exactly as in stock Lua.
//!
//! `next` is stateless: each call rebuilds the ordered key sequence from the
//! live table, so a key whose value became `nil` mid-traversal is skipped
//! rather than visited with a nil value, and traversal always advances to a
//! key strictly after the previous one. Rebuilding costs `O(n log n)` per
//! step; section tables are small and the determinism is the point. The
//! ordering covers string, number, and boolean keys, the only keys a payload
//! the run log stores can carry. A non-scalar key (table, function, userdata,
//! or thread) is still visited rather than dropped, but it sits after every
//! scalar key in the order Lua's own `next` produced it: such a key has no
//! cross-process position, and it cannot back a logged value.

use std::cmp::Ordering;

use mlua::{Function, Lua, MultiValue, Table, Value};

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
    let pairs_fn = lua
        .create_function(move |_, value: Value| pairs(value, &next_fn))
        .map_err(Error::lua)?;
    globals.raw_set("pairs", pairs_fn).map_err(Error::lua)
}

/// The `next(table, key)` replacement: the key strictly after `key` in the
/// sorted order, with its value, or `(nil, nil)` at the end.
fn deterministic_next(
    _lua: &Lua,
    (table, previous): (Table, Value),
) -> mlua::Result<(Value, Value)> {
    let keys = ordered_keys(&table).map_err(mlua::Error::external)?;
    let Some(index) = next_index(&keys, &previous) else {
        return Ok((Value::Nil, Value::Nil));
    };
    let key = keys[index].clone();
    let value: Value = table.raw_get(&key)?;
    Ok((key, value))
}

/// The `pairs(table)` replacement: a `__pairs` metamethod's results when one
/// is present, otherwise `(next, table, nil)` over the installed `next`.
fn pairs(value: Value, next_fn: &Function) -> mlua::Result<MultiValue> {
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
    let mut iterator = MultiValue::new();
    iterator.push_back(Value::Function(next_fn.clone()));
    iterator.push_back(value);
    iterator.push_back(Value::Nil);
    Ok(iterator)
}

/// The table's live keys in deterministic walk order: the array part
/// (`1..=#table`) in index order, then the hash part's scalar keys by
/// [`SortKey`], then any non-scalar keys.
fn ordered_keys(table: &Table) -> mlua::Result<Vec<Value>> {
    let border = table.raw_len();
    let mut keys: Vec<Value> = Vec::new();
    for index in 1..=border {
        let value: Value = table.raw_get(index)?;
        if matches!(value, Value::Nil) {
            continue;
        }
        let index = i64::try_from(index).map_err(|_| {
            mlua::Error::runtime("table index exceeds the integer range".to_owned())
        })?;
        keys.push(Value::Integer(index));
    }
    let mut hashed: Vec<(SortKey, Value)> = Vec::new();
    let mut unordered: Vec<Value> = Vec::new();
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
            Ok((position, _)) => hashed.push((position, key)),
            Err(_) => unordered.push(key),
        }
    }
    hashed.sort_by(|left, right| left.0.compare(&right.0));
    keys.extend(hashed.into_iter().map(|(_, key)| key));
    keys.extend(unordered);
    Ok(keys)
}

/// The index in `keys` to return for a `next` call with `previous` as the
/// last key. An exact match advances one slot; a key whose value was cleared
/// (`previous` absent) resumes at the first key that sorts strictly after it,
/// so the cleared key is skipped rather than revisited.
fn next_index(keys: &[Value], previous: &Value) -> Option<usize> {
    if matches!(previous, Value::Nil) {
        return (!keys.is_empty()).then_some(0);
    }
    if let Some(index) = keys.iter().position(|key| key == previous) {
        return (index + 1 < keys.len()).then_some(index + 1);
    }
    let Ok((position, _)) = sort_key(previous) else {
        return None;
    };
    let mut unordered = None;
    for (index, key) in keys.iter().enumerate() {
        match sort_key(key) {
            Ok((candidate, _)) if candidate.compare(&position) == Ordering::Greater => {
                return Some(index);
            }
            Err(_) => {
                unordered.get_or_insert(index);
            }
            Ok(_) => {}
        }
    }
    unordered
}

#[cfg(test)]
#[path = "iteration-tests.rs"]
mod tests;
