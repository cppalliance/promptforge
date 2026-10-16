//! Yield-to-request parsing for the leaf and structural requests (`infer`,
//! `call`, `tool_calls`, `local_tool_done`), and the malformed-yield
//! rejections shared by every op. The `tool_call` request is in
//! `parse_tool_call` and the task-operation requests (`spawn`, `timer`,
//! `concurrency`) are in `parse_tasks`.

use super::*;

#[test]
fn infer_without_a_handle_parses() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'infer', prompt = 'summarize this' }");
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
    let table = lua_table_with(
        &lua,
        "local handle = ...\nreturn { op = 'infer', prompt = 'hi', handle = handle }",
        handle_userdata(&lua),
    );
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
    let table = lua_table(
        &lua,
        "{ op = 'call', target = '## Child', input = 'override', var = { k = 1 } }",
    );
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
    let table = lua_table(
        &lua,
        "{ op = 'call', target = '## Child', var = { k = 1 } }",
    );
    let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
    match request {
        Request::Call { input, .. } => assert_eq!(input, None),
        other => panic!("expected a call request, got {other:?}"),
    }
}

#[test]
fn a_fanout_op_is_no_longer_a_request() {
    // The fanout shim is Lua over `spawn` and `join_any`; a yield naming
    // a `fanout` op is a hand-built yield and fails as one.
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'fanout', worker = '### Worker', collection = {}, var = { k = 1 } }",
    );
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn a_tool_calls_yield_missing_its_turn_a_table_per_call_or_a_call_id_is_a_malformed_yield() {
    // Only the loop's step yields `tool_calls`, so a missing or wrong turn,
    // entry, or call id is a corrupted yield, not a catchable call error.
    let lua = Lua::new();
    let whole = "{ op = 'tool_calls', turn = 7, calls = { { alias = 'echo', call_id = 'c1' } } }";
    let table = lua_table(&lua, whole);
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::ToolCalls { calls, turn } => {
            assert_eq!(turn, 7);
            let echo = ToolCallRecord {
                id: "c1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({}),
            };
            assert_eq!(calls, [echo]);
        }
        other => panic!("expected a tool_calls request, got {other:?}"),
    }
    for source in [
        "{ op = 'tool_calls', calls = { { alias = 'echo', call_id = 'c1' } } }",
        "{ op = 'tool_calls', turn = 7 }",
        "{ op = 'tool_calls', turn = 7, calls = 'c1' }",
        "{ op = 'tool_calls', turn = 7, calls = { 'c1' } }",
        "{ op = 'tool_calls', turn = 7, calls = { { alias = 'echo' } } }",
        "{ op = 'tool_calls', turn = 7, calls = { { alias = 'echo', call_id = 7 } } }",
    ] {
        let table = lua_table(&lua, source);
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }
}

/// Parses a `local_tool_done` yield with `ok = true` and `value` set to
/// what `source` evaluates to, returning the outcome.
fn local_tool_done_returning(lua: &Lua, source: &str) -> LocalToolOutcome {
    let table = lua_table(
        lua,
        &format!("{{ op = 'local_tool_done', ok = true, value = {source} }}"),
    );
    match expect_request(Request::from_yield(lua, &Value::Table(table))) {
        Request::LocalToolDone { outcome } => outcome,
        other => panic!("expected a local_tool_done request, got {other:?}"),
    }
}

#[test]
fn a_local_tool_done_return_becomes_its_scalar_text() {
    let lua = Lua::new();
    for (source, expected) in [("'hi'", "hi"), ("42", "42"), ("true", "true"), ("nil", "")] {
        match local_tool_done_returning(&lua, source) {
            LocalToolOutcome::Returned(text) => assert_eq!(text, expected, "{source}"),
            other => panic!("{source} must return its text, got {other:?}"),
        }
    }
}

#[test]
fn a_local_tool_done_table_return_is_a_bad_return() {
    let lua = Lua::new();
    match local_tool_done_returning(&lua, "{ 1, 2 }") {
        LocalToolOutcome::BadReturn(Error::Lua(message)) => {
            assert_eq!(message, "cannot return a table as a result");
        }
        other => panic!("a table return must be rejected, got {other:?}"),
    }
}

#[test]
fn a_local_tool_done_with_ok_false_is_raised() {
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "{ op = 'local_tool_done', ok = false, value = 'ignored' }",
    );
    match expect_request(Request::from_yield(&lua, &Value::Table(table))) {
        Request::LocalToolDone {
            outcome: LocalToolOutcome::Raised,
        } => {}
        other => panic!("expected a raised outcome, got {other:?}"),
    }
}

#[test]
fn a_local_tool_done_without_a_boolean_ok_is_a_malformed_yield() {
    let lua = Lua::new();
    for source in [
        "{ op = 'local_tool_done', value = 'hi' }",
        "{ op = 'local_tool_done', ok = 1, value = 'hi' }",
    ] {
        let table = lua_table(&lua, source);
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }
}

#[test]
fn a_non_table_yield_is_rejected() {
    let lua = Lua::new();
    assert_direct_yield(Request::from_yield(&lua, &Value::Integer(1)));
    let text = lua
        .create_string("infer")
        .expect("a fresh VM creates a short string");
    assert_direct_yield(Request::from_yield(&lua, &Value::String(text)));
}

#[test]
fn a_yield_without_an_op_is_rejected() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{}");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn an_unknown_op_is_rejected() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'teleport' }");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn an_infer_with_a_missing_or_non_string_prompt_is_the_calls_error() {
    // The author-facing argument error returns as the call's answer,
    // so the shim raises it at the call site, where an author `pcall`
    // catches it.
    let lua = Lua::new();
    for (source, expected) in [
        ("{ op = 'infer' }", "prompt must be a string, got nil"),
        (
            "{ op = 'infer', prompt = 42 }",
            "prompt must be a string, got integer",
        ),
    ] {
        let table = lua_table(&lua, source);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
                assert_eq!(message, expected, "{source}");
            }
            other => panic!("expected the prompt call error for {source}, got {other:?}"),
        }
    }
}

#[test]
fn an_infer_with_a_wrong_handle_type_is_the_calls_error() {
    // The handle is the author-supplied receiver of `h:infer`, so a
    // wrong shape is the call's error (pcall-able at the call site), not
    // a malformed-yield block failure.
    let lua = Lua::new();
    let as_string = lua_table(
        &lua,
        "{ op = 'infer', prompt = 'hi', handle = 'not a handle' }",
    );
    match Request::from_yield(&lua, &Value::Table(as_string)) {
        YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
            assert_eq!(
                message,
                "models.infer handle must be a model handle, got string"
            );
        }
        other => panic!("expected the handle call error, got {other:?}"),
    }
    let wrong = lua
        .create_userdata(OtherUserData)
        .expect("userdata creation cannot fail on a fresh VM");
    let as_other_userdata = lua_table_with(
        &lua,
        "local handle = ...\nreturn { op = 'infer', prompt = 'hi', handle = handle }",
        wrong,
    );
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
    let table = lua_table(&lua, "{ op = 'call', target = 42, var = { k = 1 } }");
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
fn a_request_without_a_var_snapshot_is_rejected() {
    let lua = Lua::new();
    let table = lua_table(&lua, "{ op = 'call', target = '## Child' }");
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}

#[test]
fn metatable_spoofed_fields_are_not_read() {
    let lua = Lua::new();
    let table = lua_table(
        &lua,
        "setmetatable({}, { __index = { op = 'infer', prompt = 'hi' } })",
    );
    assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
}
