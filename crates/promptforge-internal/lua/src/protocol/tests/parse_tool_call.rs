//! Yield parsing for the `tool_call` request: the alias or tool object,
//! the args table, and the shim-produced call id and turn, with every
//! author-argument failure as the call's own answer and every wrong shim
//! field as a malformed yield.

use super::*;

#[test]
fn tool_call_parses_alias_and_args() {
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'tool_call', alias = 'echo', args = { value = 'hi' } }",
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall {
            alias,
            args,
            call_id,
            turn,
        } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
            assert_eq!(call_id, None, "a script call omits the call id");
            assert_eq!(turn, None, "a script call omits the turn");
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn tool_call_with_a_call_id_parses_it_as_a_model_issued_call() {
    // The loop shim sets `call_id` from the model's tool call; the request
    // stores it so the driver resumes with content and fires ToolResult
    // under that id.
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'tool_call', alias = 'echo', call_id = 'call_7' }",
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall { alias, call_id, .. } => {
            assert_eq!(alias, "echo");
            assert_eq!(call_id.as_deref(), Some("call_7"));
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_non_string_call_id_is_a_malformed_yield() {
    // `call_id` is shim-produced, never author-supplied: a wrong shape is
    // a corrupted yield, not a catchable call error.
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'tool_call', alias = 'echo', call_id = 7 }");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn a_tool_call_without_a_turn_parses_it_as_none() {
    // A script call and the test-only `tools.call_as_model` hook leave
    // `turn` out; the scheduler falls back to the live counter.
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'tool_call', alias = 'echo', call_id = 'call_7' }",
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall { turn, .. } => assert_eq!(turn, None),
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_a_turn_parses_the_requesting_round() {
    // The loop shim passes back the turn its chat round reported the
    // batch under.
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'tool_call', alias = 'echo', call_id = 'call_7', turn = 3 }",
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall { call_id, turn, .. } => {
            assert_eq!(call_id.as_deref(), Some("call_7"));
            assert_eq!(turn, Some(3));
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_turn_outside_u32_or_not_an_integer_is_a_malformed_yield() {
    // `turn` is shim-produced, never author-supplied: a wrong shape is a
    // corrupted yield, not a catchable call error. `4294967296` is one
    // past `u32::MAX`, and `2.0` is a float even with an integral value.
    let lua = Lua::new();
    for bad in ["-1", "4294967296", "2.0", "2.5", "'2'"] {
        let table = lua_table(
            &lua,
            &format!("{{ op = 'tool_call', alias = 'echo', call_id = 'call_7', turn = {bad} }}"),
        );
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }
}

#[test]
fn tool_call_without_args_parses_the_empty_object() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'tool_call', alias = 'echo' }");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall { args, .. } => assert_eq!(args, json!({})),
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_a_tool_object_decodes_to_its_id() {
    // The id-or-tool-object polymorphism at the protocol boundary: a tool
    // object stands for its canonical id, never its wire name.
    let binding = crate::ToolBinding::from_descriptor("tools_echo", &crate::test_vm::echo_tool());
    let vm = crate::test_vm::lua_over(crate::ToolSet::new(
        Vec::new(),
        std::collections::BTreeMap::new(),
        vec![binding],
    ));
    let table: mlua::Table =
        vm.eval("return { op = 'tool_call', alias = tools.get('tools/echo') }");
    let request = expect_request(Request::from_yield(vm.lua(), &Value::Table(table)));
    match request {
        Request::ToolCall { alias, .. } => assert_eq!(alias, "tools/echo"),
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_a_non_alias_alias_is_the_calls_error() {
    // The author-facing argument error returns as the call's answer,
    // framed byte-identically with the other author-argument failures.
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'tool_call', alias = 42 }");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                "tools.call takes a tool id, a local alias, or a tool object, got integer"
            );
        }
        other => panic!("expected the alias call error, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_a_non_table_args_is_the_calls_error() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'tool_call', alias = 'echo', args = 42 }");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
            assert_eq!(message, "args must be a table, got integer");
        }
        other => panic!("expected the args call error, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_an_unrepresentable_args_table_is_the_calls_error() {
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'tool_call', alias = 'echo', args = { f = function() end } }",
    );
    let args: Value = table
        .raw_get("args")
        .expect("the request table holds its args");
    let cause = lua
        .from_value::<serde_json::Value>(args)
        .expect_err("a table holding a function has no JSON form");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                format!("args must be a JSON-representable table: {cause}")
            );
        }
        other => panic!("expected the args call error, got {other:?}"),
    }
}
