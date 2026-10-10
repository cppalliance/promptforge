//! Answer-to-envelope rendering: every [`Answer`] variant round-trips through
//! Lua as the `(ok, result)` envelope and retains its typed error. The
//! `chat` answer's error is in `answer_chat`.

use super::*;

#[test]
fn an_ok_infer_answer_round_trips_through_lua() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::<Error>::Infer(Ok("completion".to_owned()))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let Value::String(text) = result else {
        panic!("expected a string result, got {result:?}");
    };
    assert_eq!(text.to_str().expect("the text is UTF-8"), "completion");
}

#[test]
fn an_ok_call_answer_round_trips_through_lua() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::<Error>::Call(Ok("chain text".to_owned()))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let Value::String(text) = result else {
        panic!("expected a string result, got {result:?}");
    };
    assert_eq!(text.to_str().expect("the text is UTF-8"), "chain text");
}

#[test]
fn an_ok_spawn_answer_resumes_the_task_id_as_its_path_text() {
    let lua = Lua::new();
    let task: TaskId = "0.2".parse().expect("a task id parses");
    let (envelope, retained) = Answer::<Error>::Spawn(Ok(task))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let Value::String(text) = result else {
        panic!("expected a string result, got {result:?}");
    };
    assert_eq!(text.to_str().expect("the text is UTF-8"), "0.2");
}

#[test]
fn an_err_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::Call(Err(Error::LuaQuota {
        resource: "instruction",
    }))
    .into_envelope(&lua)
    .expect("the envelope renders");
    match retained {
        Some(Error::LuaQuota {
            resource: "instruction",
        }) => {}
        other => panic!("expected the retained LuaQuota error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "lua");
    assert_eq!(message, "lua instruction quota exceeded");
}

#[test]
fn a_join_any_delivery_of_a_failed_member_retains_the_members_typed_error() {
    // The wait succeeded, so the envelope is `(true, id, false, table)`,
    // but the member's failure is handed back typed as well: a shim that
    // re-raises it at once (`fanout` on a fatal arm) lets the driver
    // substitute the member's own error for the raised table.
    let lua = Lua::new();
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let (envelope, retained) = Answer::<Error>::JoinAny(Ok(TaskDelivery {
        task,
        outcome: Err(Error::LuaQuota {
            resource: "instruction",
        }),
    }))
    .into_envelope(&lua)
    .expect("the envelope renders");
    match retained {
        Some(Error::LuaQuota {
            resource: "instruction",
        }) => {}
        other => panic!("expected the member's retained LuaQuota error, got {other:?}"),
    }
    let (ok, id, member_ok, kind, message): (bool, String, bool, String, String) = lua
        .load(
            "local ok, id, member_ok, err = ...; \
             return ok, id, member_ok, err.kind, tostring(err)",
        )
        .call(envelope)
        .expect("the delivery reads back through Lua");
    assert!(ok, "the wait itself succeeded");
    assert_eq!(id, "0.1");
    assert!(!member_ok, "the member failed");
    assert_eq!(kind, "lua");
    assert_eq!(message, "lua instruction quota exceeded");
}

#[test]
fn a_join_any_delivery_of_a_finished_member_retains_nothing() {
    let lua = Lua::new();
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let (envelope, retained) = Answer::<Error>::JoinAny(Ok(TaskDelivery {
        task,
        outcome: Ok("done".to_owned()),
    }))
    .into_envelope(&lua)
    .expect("the envelope renders");
    assert!(retained.is_none(), "a success retains nothing");
    let (ok, member_ok, text): (bool, bool, String) = lua
        .load("local ok, _, member_ok, text = ...; return ok, member_ok, text")
        .call(envelope)
        .expect("the delivery reads back through Lua");
    assert!(ok);
    assert!(member_ok);
    assert_eq!(text, "done");
}

#[test]
fn an_ok_plain_tool_call_answer_round_trips_as_a_string() {
    let lua = Lua::new();
    let (envelope, retained) =
        Answer::<Error>::ToolCallResult(Ok(ToolCallOutcome::Plain("echoed: hi".to_owned())))
            .into_envelope(&lua)
            .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let Value::String(text) = result else {
        panic!("expected a string result, got {result:?}");
    };
    assert_eq!(text.to_str().expect("the text is UTF-8"), "echoed: hi");
}

#[test]
fn an_ok_structured_tool_call_answer_round_trips_as_a_table() {
    let lua = Lua::new();
    let outcome = ToolCallOutcome::Structured(json!({ "text": "typed", "images": [] }));
    let (envelope, retained) = Answer::<Error>::ToolCallResult(Ok(outcome))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, text, images_len): (bool, String, i64) = lua
        .load("local ok, result = ...; return ok, result.text, #result.images")
        .call(envelope)
        .expect("the table reads back through Lua");
    assert!(ok);
    assert_eq!(text, "typed");
    assert_eq!(images_len, 0);
}

/// A fresh VM with the `tools` namespace installed and one local tool,
/// `grab`, registered through `tools.add_local`.
fn lua_with_local_grab() -> Lua {
    let lua = Lua::new();
    crate::install_tools(
        &lua,
        &lua.globals(),
        &std::sync::Arc::new(std::sync::Mutex::new(crate::ToolSet::default())),
        &std::sync::Arc::new(std::sync::Mutex::new(crate::ToolRuntime {
            added: Vec::new(),
            description_overrides: std::collections::BTreeMap::default(),
            allowed_tasks: None,
        })),
        &crate::vm::LocalTools::default(),
    )
    .expect("the tools install cannot fail on a fresh VM");
    lua.load(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, \
         function(args) return 'got ' .. args.value end)",
    )
    .exec()
    .expect("the local tool registers");
    lua
}

#[test]
fn a_local_tool_call_answer_resumes_with_its_handler_and_args() {
    let lua = lua_with_local_grab();
    let (envelope, retained) = Answer::<Error>::ToolCallResult(Ok(ToolCallOutcome::Local {
        alias: "grab".to_owned(),
        args: json!({ "value": "hi" }),
    }))
    .into_envelope(&lua)
    .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result_is_nil, handler_is_fn, called): (bool, bool, bool, String) = lua
        .load(
            "local ok, result, handler, args = ...\n\
             return ok, result == nil, type(handler) == 'function', handler(args)",
        )
        .call(envelope)
        .expect("the envelope reads back through Lua");
    assert!(ok);
    assert!(
        result_is_nil,
        "the result slot stays nil for a local answer"
    );
    assert!(handler_is_fn, "the handler resumes as a function");
    assert_eq!(called, "got hi", "the registered handler runs on the args");
}

#[test]
fn a_local_tool_call_answer_for_an_unregistered_alias_is_an_error() {
    let lua = lua_with_local_grab();
    let error = Answer::<Error>::ToolCallResult(Ok(ToolCallOutcome::Local {
        alias: "missing".to_owned(),
        args: json!({}),
    }))
    .into_envelope(&lua)
    .expect_err("an alias with no registered handler cannot render");
    assert!(
        error
            .to_string()
            .contains("a local tool answer names a registered handler"),
        "the error names the broken invariant: {error}"
    );
}

#[test]
fn an_err_tool_call_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::ToolCallResult(Err(Error::Interrupted))
        .into_envelope(&lua)
        .expect("the envelope renders");
    match retained {
        Some(Error::Interrupted) => {}
        other => panic!("expected the retained Interrupted error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "cancelled");
    assert_eq!(
        message,
        "interrupted: the run was cancelled or this call was stopped"
    );
}

#[test]
fn from_dispatch_classifies_by_the_declared_output_kind() {
    use crate::ToolOutputKind;

    // Plain output passes through untouched.
    match ToolCallOutcome::from_dispatch(ToolOutputKind::Plain, "echo", "raw".to_owned()) {
        Ok(ToolCallOutcome::Plain(text)) => assert_eq!(text, "raw"),
        other => panic!("expected the plain passthrough, got {other:?}"),
    }
    // Structured output parses as JSON.
    match ToolCallOutcome::from_dispatch(
        ToolOutputKind::Structured,
        "form",
        "{\"text\":\"hi\"}".to_owned(),
    ) {
        Ok(ToolCallOutcome::Structured(json)) => assert_eq!(json, json!({ "text": "hi" })),
        other => panic!("expected the structured parse, got {other:?}"),
    }
    // Invalid JSON from a structured binding is the tool's error.
    match ToolCallOutcome::from_dispatch(ToolOutputKind::Structured, "form", "not json".to_owned())
    {
        Err(Error::Tool { message, source }) => {
            assert_eq!(message, "structured tool \"form\" returned invalid JSON");
            assert!(
                source.downcast_ref::<serde_json::Error>().is_some(),
                "the parse failure must survive as the cause"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}
