//! Yield parsing for the `tool_call` request: the alias or tool object,
//! the args table, and the shim-produced call id and turn, with every
//! author-argument failure as the call's own answer and every wrong shim
//! field as a malformed yield.

use super::*;

#[test]
fn tool_call_parses_alias_and_args() {
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    let args = lua.create_table().expect("table creation cannot fail");
    args.raw_set("value", "hi").expect("raw_set");
    table.raw_set("args", args).expect("raw_set");
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    table.raw_set("call_id", "call_7").expect("raw_set");
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    table.raw_set("call_id", 7).expect("raw_set");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn a_tool_call_without_a_turn_parses_it_as_none() {
    // A script call and the test-only `tools.call_as_model` hook leave
    // `turn` out; the scheduler falls back to the live counter.
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    table.raw_set("call_id", "call_7").expect("raw_set");
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    table.raw_set("call_id", "call_7").expect("raw_set");
    table.raw_set("turn", 3).expect("raw_set");
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
    // corrupted yield, not a catchable call error.
    let lua = Lua::new();
    let bad_turns = [
        Value::Integer(-1),
        Value::Integer(i64::from(u32::MAX) + 1),
        Value::Number(2.0),
        Value::Number(2.5),
        Value::String(lua.create_string("2").expect("string")),
    ];
    for bad in bad_turns {
        let table = request_table(&lua, "tool_call");
        table.raw_set("alias", "echo").expect("raw_set");
        table.raw_set("call_id", "call_7").expect("raw_set");
        table.raw_set("turn", bad).expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }
}

#[test]
fn tool_call_without_args_parses_the_empty_object() {
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
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
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    let descriptor = promptforge_types::tools::ToolDescriptor::new(
        promptforge_types::tools::ToolId::parse("tools/echo").expect("valid id"),
        "echo tool",
        json!({ "type": "object" }),
    );
    let handle = crate::LuaToolHandle::from_binding(&crate::ToolBinding::from_descriptor(
        "tools_echo",
        &descriptor,
    ));
    let userdata = lua.create_userdata(handle).expect("userdata");
    table.raw_set("alias", userdata).expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", 42).expect("raw_set");
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    table.raw_set("args", 42).expect("raw_set");
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
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", "echo").expect("raw_set");
    let args = lua.create_table().expect("table creation cannot fail");
    let member = lua
        .create_function(|_, ()| Ok(()))
        .expect("function creation cannot fail");
    args.raw_set("f", member).expect("raw_set");
    table.raw_set("args", args).expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
            assert_eq!(message, "args must be a JSON-representable table");
        }
        other => panic!("expected the args call error, got {other:?}"),
    }
}
