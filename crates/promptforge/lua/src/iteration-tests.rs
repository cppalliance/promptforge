//! Tests for the deterministic `pairs`/`next` installer.

use mlua::{Lua, LuaSerdeExt, Value};
use serde_json::json;

use super::install_deterministic_iteration;

/// A VM with the deterministic iterator installed, as a section VM has it.
fn vm() -> Lua {
    let lua = Lua::new();
    install_deterministic_iteration(&lua).expect("the installer runs");
    lua
}

/// Evaluates a chunk that returns a sequence table and reads it back as JSON.
fn sequence(lua: &Lua, source: &str) -> Vec<serde_json::Value> {
    let value: Value = lua.load(source).eval().expect("the chunk evaluates");
    lua.from_value(value).expect("the result is JSON data")
}

#[test]
fn pairs_visits_string_keys_in_byte_order() {
    // Two insertion orders produce the same key sequence: the order is a
    // function of the keys, not of the table's hash state.
    let lua = vm();
    let source = "local out = {}; for k in pairs({zeta=1, alpha=2, mid=3, beta=4}) \
                  do out[#out+1] = k end; return out";
    assert_eq!(
        sequence(&lua, source),
        vec![json!("alpha"), json!("beta"), json!("mid"), json!("zeta")]
    );
    let source = "local out = {}; for k in pairs({beta=4, mid=3, alpha=2, zeta=1}) \
                  do out[#out+1] = k end; return out";
    assert_eq!(
        sequence(&lua, source),
        vec![json!("alpha"), json!("beta"), json!("mid"), json!("zeta")]
    );
}

#[test]
fn pairs_visits_the_array_part_before_the_hash_part() {
    let lua = vm();
    let source = "local out = {}; for k in pairs({10, 20, 30, extra='x', another='y'}) \
                  do out[#out+1] = k end; return out";
    assert_eq!(
        sequence(&lua, source),
        vec![
            json!(1),
            json!(2),
            json!(3),
            json!("another"),
            json!("extra")
        ]
    );
}

#[test]
fn pairs_orders_mixed_key_types_booleans_then_numbers_then_strings() {
    let lua = vm();
    let source = "local out = {}; \
                  for k in pairs({[true]='t', [7]='seven', b='bee', [false]='f', [2.5]='half', a='ay'}) \
                  do out[#out+1] = k end; return out";
    assert_eq!(
        sequence(&lua, source),
        vec![
            json!(false),
            json!(true),
            json!(2.5),
            json!(7),
            json!("a"),
            json!("b"),
        ]
    );
}

#[test]
fn pairs_honors_a_pairs_metamethod() {
    // The table is empty, so only the metamethod can yield the two pairs; a
    // stock `pairs` would return nothing.
    let lua = vm();
    let source = "local t = setmetatable({}, {__pairs = function() \
                    local i = 0 \
                    return function() i = i + 1; if i <= 2 then return i, i * 10 end end \
                  end}) \
                  local out = {} \
                  for k, v in pairs(t) do out[#out+1] = k .. ':' .. v end \
                  return out";
    assert_eq!(sequence(&lua, source), vec![json!("1:10"), json!("2:20")]);
}

#[test]
fn next_is_nil_for_an_empty_table() {
    let lua = vm();
    let source = "local t = {}; return {next(t) == nil, next(t, nil) == nil}";
    assert_eq!(sequence(&lua, source), vec![json!(true), json!(true)]);
}

#[test]
fn next_returns_the_first_pair_of_a_non_empty_table() {
    let lua = vm();
    let source = "local k, v = next({only=1}); return {k == 'only', v == 1}";
    assert_eq!(sequence(&lua, source), vec![json!(true), json!(true)]);
}

#[test]
fn pairs_skips_a_key_cleared_before_it_is_visited() {
    let lua = vm();
    let source = "local t = {a=1, b=2, c=3} \
                  local out = {} \
                  for k in pairs(t) do \
                    out[#out+1] = k \
                    if k == 'a' then t.b = nil end \
                  end \
                  return out";
    assert_eq!(sequence(&lua, source), vec![json!("a"), json!("c")]);
}

#[test]
fn pairs_skips_the_key_cleared_by_the_current_step() {
    // Clearing the current key leaves the previous-key cursor pointing at a
    // key no longer in the table; the walk resumes at the next live key
    // instead of erroring or dropping the rest of the traversal.
    let lua = vm();
    let source = "local t = {a=1, b=2, c=3} \
                  local out = {} \
                  for k in pairs(t) do \
                    out[#out+1] = k \
                    t[k] = nil \
                  end \
                  return out";
    assert_eq!(
        sequence(&lua, source),
        vec![json!("a"), json!("b"), json!("c")]
    );
}
