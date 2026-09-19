//! Yield-to-request parsing for the leaf and structural requests (`infer`,
//! `call`, `fanout`, `tool_call`, `user_input`, the reserved `mcp`), and
//! the malformed-yield rejections shared by every op.

use super::*;

#[test]
fn infer_without_a_handle_parses() {
    let lua = Lua::new();
    let table = request_table(&lua, "infer");
    table.raw_set("prompt", "summarize this").expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Infer { prompt, binding } => {
            assert_eq!(prompt, "summarize this");
            assert_eq!(binding, None);
        }
        other => panic!("expected an infer request, got {other:?}"),
    }
}

#[test]
fn infer_with_a_handle_clones_its_frozen_binding() {
    let lua = Lua::new();
    let table = request_table(&lua, "infer");
    table.raw_set("prompt", "hi").expect("raw_set");
    table
        .raw_set("handle", handle_userdata(&lua))
        .expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Infer {
            binding: Some(binding),
            ..
        } => {
            assert_eq!(binding.alias(), "fast");
            assert_eq!(binding.id().name(), "test-model");
        }
        other => panic!("expected an infer request with a binding, got {other:?}"),
    }
}

#[test]
fn call_parses_target_input_and_var_snapshot() {
    let lua = Lua::new();
    let table = request_table(&lua, "call");
    table.raw_set("target", "## Child").expect("raw_set");
    table.raw_set("input", "override").expect("raw_set");
    set_var_snapshot(&lua, &table);
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Call { target, input, var } => {
            assert_eq!(target, "## Child");
            assert_eq!(input.as_deref(), Some("override"));
            assert_eq!(var, json!({ "k": 1 }));
        }
        other => panic!("expected a call request, got {other:?}"),
    }
}

#[test]
fn call_without_input_yields_none() {
    let lua = Lua::new();
    let table = request_table(&lua, "call");
    table.raw_set("target", "## Child").expect("raw_set");
    set_var_snapshot(&lua, &table);
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Call { input, .. } => assert_eq!(input, None),
        other => panic!("expected a call request, got {other:?}"),
    }
}

#[test]
fn fanout_parses_and_converts_the_collection_member_wise() {
    let lua = Lua::new();
    let table = request_table(&lua, "fanout");
    table.raw_set("worker", "### Worker").expect("raw_set");
    let collection = lua.create_table().expect("table creation cannot fail");
    collection.raw_set(1, "a").expect("raw_set");
    collection.raw_set(2, 2).expect("raw_set");
    collection.raw_set("key", true).expect("raw_set");
    table.raw_set("collection", collection).expect("raw_set");
    set_var_snapshot(&lua, &table);
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Fanout { worker, items, var } => {
            assert_eq!(worker, "### Worker");
            assert_eq!(
                items,
                vec![json!("a"), json!(2), json!({ "key": "key", "value": true })]
            );
            assert_eq!(var, json!({ "k": 1 }));
        }
        other => panic!("expected a fanout request, got {other:?}"),
    }
}

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
        } => {
            assert_eq!(alias, "echo");
            assert_eq!(args, json!({ "value": "hi" }));
            assert_eq!(call_id, None, "a script call carries no call id");
        }
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn tool_call_with_a_call_id_parses_it_as_a_model_issued_call() {
    // The loop shim sets `call_id` from the model's tool call; the request
    // carries it so the driver resumes with content and fires ToolResult
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
fn a_tool_call_with_a_tool_object_alias_decodes_to_its_alias() {
    // The alias-or-Tool polymorphism at the protocol boundary: a Tool
    // object (a captured alias global, a `tools.bind` return) names the
    // binding it was created from.
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    let handle = crate::LuaToolHandle::from_binding(
        "echo",
        "echo tool",
        &promptforge_api_types::tools::ToolId::parse("tests/tools/echo").expect("valid id"),
    );
    let userdata = lua.create_userdata(handle).expect("userdata");
    table.raw_set("alias", userdata).expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::ToolCall { alias, .. } => assert_eq!(alias, "echo"),
        other => panic!("expected a tool_call request, got {other:?}"),
    }
}

#[test]
fn a_tool_call_with_a_non_alias_alias_is_the_calls_error() {
    // The author-facing argument error rides back as the call's answer,
    // framed byte-identically with the other author-argument failures.
    let lua = Lua::new();
    let table = request_table(&lua, "tool_call");
    table.raw_set("alias", 42).expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::ToolCallResult(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                "tools.call alias must be a string or Tool object, got integer"
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

#[test]
fn a_user_input_yield_parses_to_the_request() {
    let lua = Lua::new();
    let table = request_table(&lua, "user_input");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    assert!(
        matches!(request, Request::UserInput),
        "a user_input yield is the unit request, got {request:?}"
    );
}

#[test]
fn mcp_reserved_fields_parse() {
    let lua = Lua::new();
    let table = request_table(&lua, "mcp");
    table.raw_set("server", "srv").expect("raw_set");
    table.raw_set("tool", "tl").expect("raw_set");
    let args = lua.create_table().expect("table creation cannot fail");
    table.raw_set("args", args).expect("raw_set");
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Mcp { server, tool, args } => {
            assert_eq!(server, "srv");
            assert_eq!(tool, "tl");
            assert_eq!(args, json!({}));
        }
        other => panic!("expected an mcp request, got {other:?}"),
    }
}

#[test]
fn a_received_mcp_request_is_a_typed_protocol_error() {
    match Request::mcp_reserved() {
        Error::Lua(message) => assert!(message.contains("mcp")),
        other => panic!("expected a typed Lua protocol error, got {other:?}"),
    }
}

#[test]
fn a_non_table_yield_is_rejected() {
    let lua = Lua::new();
    assert_direct_yield(Request::from_yield(&lua, &Value::Integer(1)));
    let text = lua.create_string("infer").expect("string creation");
    assert_direct_yield(Request::from_yield(&lua, &Value::String(text)));
}

#[test]
fn a_yield_without_an_op_is_rejected() {
    let lua = Lua::new();
    let table = lua.create_table().expect("table creation cannot fail");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn an_unknown_op_is_rejected() {
    let lua = Lua::new();
    let table = request_table(&lua, "teleport");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn an_infer_with_a_missing_or_non_string_prompt_is_the_calls_error() {
    // The author-facing argument error rides back as the call's answer,
    // so the shim raises it at the call site (pcall-able), exactly as
    // the legacy callback's conversion error surfaced.
    let lua = Lua::new();
    let missing = request_table(&lua, "infer");
    match Request::from_yield(&lua, &Value::Table(missing)) {
        YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
            assert_eq!(message, "prompt must be a string, got nil");
        }
        other => panic!("expected the prompt call error, got {other:?}"),
    }
    let typed_wrong = request_table(&lua, "infer");
    typed_wrong.raw_set("prompt", 42).expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(typed_wrong)) {
        YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
            assert_eq!(message, "prompt must be a string, got integer");
        }
        other => panic!("expected the prompt call error, got {other:?}"),
    }
}

#[test]
fn an_infer_with_a_wrong_handle_type_is_the_calls_error() {
    // The handle is author-supplied under namespace-only invocation, so
    // a wrong shape is the call's error (pcall-able at the call site),
    // not a malformed-yield block failure.
    let lua = Lua::new();
    let as_string = request_table(&lua, "infer");
    as_string.raw_set("prompt", "hi").expect("raw_set");
    as_string
        .raw_set("handle", "not a handle")
        .expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(as_string)) {
        YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                "models.infer handle must be a model handle, got string"
            );
        }
        other => panic!("expected the handle call error, got {other:?}"),
    }
    let as_other_userdata = request_table(&lua, "infer");
    as_other_userdata.raw_set("prompt", "hi").expect("raw_set");
    let wrong = lua
        .create_userdata(LuaFanoutResult::success(json!(1), "x"))
        .expect("userdata creation cannot fail on a fresh VM");
    as_other_userdata.raw_set("handle", wrong).expect("raw_set");
    match Request::from_yield(&lua, &Value::Table(as_other_userdata)) {
        YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
            assert_eq!(message, "models.infer handle must be a model handle");
        }
        other => panic!("expected the handle call error, got {other:?}"),
    }
}

#[test]
fn a_call_with_a_non_string_target_keeps_the_resolve_error() {
    let lua = Lua::new();
    let table = request_table(&lua, "call");
    table.raw_set("target", 42).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Call(Err(Error::LuaRuntime { message, .. }))) => {
            assert!(
                message.contains("section target must be a string, got integer"),
                "unexpected message: {message}"
            );
        }
        other => panic!("expected the resolve_section_target call error, got {other:?}"),
    }
}

#[test]
fn a_fanout_with_a_non_string_worker_is_the_calls_error() {
    // The author-facing argument error rides back as the call's answer,
    // so the shim raises it at the call site (pcall-able), exactly as
    // the legacy callback's conversion error surfaced.
    let lua = Lua::new();
    let table = request_table(&lua, "fanout");
    table.raw_set("worker", 42).expect("raw_set");
    let collection = lua.create_table().expect("table creation cannot fail");
    table.raw_set("collection", collection).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => {
            assert_eq!(message, "worker must be a string, got integer");
        }
        other => panic!("expected the worker call error, got {other:?}"),
    }
}

#[test]
fn a_request_without_a_var_snapshot_is_rejected() {
    let lua = Lua::new();
    let table = request_table(&lua, "call");
    table.raw_set("target", "## Child").expect("raw_set");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn fanout_collection_member_errors_stay_byte_identical() {
    let lua = Lua::new();
    let table = request_table(&lua, "fanout");
    table.raw_set("worker", "### Worker").expect("raw_set");
    let collection = lua.create_table().expect("table creation cannot fail");
    let member = lua
        .create_function(|_, ()| Ok(()))
        .expect("function creation cannot fail");
    collection.raw_set(1, member).expect("raw_set");
    table.raw_set("collection", collection).expect("raw_set");
    set_var_snapshot(&lua, &table);
    match Request::from_yield(&lua, &Value::Table(table)) {
        YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => assert_eq!(
            message,
            "fanout collection member at index 1 is a function; members must be data"
        ),
        other => panic!("expected the collection member call error, got {other:?}"),
    }
}

#[test]
fn metatable_spoofed_fields_are_not_read() {
    let lua = Lua::new();
    let table = lua.create_table().expect("table creation cannot fail");
    let index = lua.create_table().expect("table creation cannot fail");
    index.raw_set("op", "infer").expect("raw_set");
    index.raw_set("prompt", "hi").expect("raw_set");
    let metatable = lua.create_table().expect("table creation cannot fail");
    metatable.raw_set("__index", index).expect("raw_set");
    table
        .set_metatable(Some(metatable))
        .expect("set_metatable on a fresh table cannot fail");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}
