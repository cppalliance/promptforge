//! Answer-to-envelope rendering: every [`Answer`] variant round-trips through
//! Lua as the `(ok, result)` envelope and retains its typed error.

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
fn an_ok_fanout_answer_round_trips_as_an_ordered_result_sequence() {
    let lua = Lua::new();
    let results = vec![
        LuaFanoutResult::success(json!("a"), "text-a"),
        LuaFanoutResult::exhausted_stub(json!("b"), "stub-b"),
    ];
    let (envelope, retained) = Answer::<Error>::Fanout(Ok(results))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, len, first_text, second_ok, second_exhausted, rendered): (
        bool,
        i64,
        String,
        bool,
        bool,
        String,
    ) = lua
        .load(
            "local ok, seq = ...; \
             return ok, #seq, seq[1].text, seq[2].ok, seq[2].exhausted, tostring(seq[1])",
        )
        .call(envelope)
        .expect("the sequence reads back through Lua");
    assert!(ok);
    assert_eq!(len, 2);
    assert_eq!(first_text, "text-a");
    assert!(!second_ok);
    assert!(second_exhausted);
    assert_eq!(rendered, "text-a");
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
    assert_eq!(message, "interrupted by Ctrl-C");
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

#[test]
fn an_ok_chat_reply_answer_resumes_as_a_table_with_nil_tool_calls() {
    use promptforge_api_types::events::{ClientTiming, Usage};

    let lua = Lua::new();
    let result = ChatResult {
        reply: Some("hello there".to_owned()),
        tool_calls: None,
        finish_reason: Some("stop".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: Some(CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: None,
                reasoning_tokens: None,
            }),
            llama: None,
            vllm: None,
            client: Some(ClientTiming {
                ttft_ms: Some(9.5),
                mean_itl_ms: None,
                e2e_ms: 41.5,
            }),
        }),
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    // Presence-branching is the agent contract: absent fields must read
    // back as true Lua nil, never a serde null sentinel.
    let (ok, reply, tools_nil, finish, model, total, llama_nil, e2e): (
        bool,
        String,
        bool,
        String,
        String,
        i64,
        bool,
        f64,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.reply, r.tool_calls == nil, r.finish_reason, r.model, \
             r.metrics.usage.total_tokens, r.metrics.llama == nil, r.metrics.client.e2e_ms",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert_eq!(reply, "hello there");
    assert!(
        tools_nil,
        "an absent tool_calls must be nil, not a null sentinel"
    );
    assert_eq!(finish, "stop");
    assert_eq!(model, "fixture-model");
    assert_eq!(total, 10);
    assert!(llama_nil, "an absent metrics section must be nil");
    assert!((e2e - 41.5).abs() < f64::EPSILON);
}

#[test]
fn an_ok_chat_tool_calls_answer_resumes_with_presence_and_arguments() {
    let lua = Lua::new();
    let result = ChatResult {
        reply: None,
        tool_calls: Some(vec![
            ToolCallEvent {
                id: "call_1".to_owned(),
                name: "echo".to_owned(),
                arguments: json!({ "value": "hi" }),
            },
            ToolCallEvent {
                id: "call_2".to_owned(),
                name: "search".to_owned(),
                arguments: json!({ "query": "rust" }),
            },
        ]),
        finish_reason: Some("tool_calls".to_owned()),
        model: "fixture-model".to_owned(),
        metrics: None,
    };
    let (envelope, retained) = Answer::<Error>::Chat(Ok(Box::new(result)))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, reply_nil, len, id, name, value, second, metrics_nil): (
        bool,
        bool,
        i64,
        String,
        String,
        String,
        String,
        bool,
    ) = lua
        .load(
            "local ok, r = ...; \
             return ok, r.reply == nil, #r.tool_calls, r.tool_calls[1].id, \
             r.tool_calls[1].name, r.tool_calls[1].arguments.value, \
             r.tool_calls[2].arguments.query, r.metrics == nil",
        )
        .call(envelope)
        .expect("the result table reads back through Lua");
    assert!(ok);
    assert!(reply_nil, "a tool-calls round has no reply");
    assert_eq!(len, 2);
    assert_eq!(id, "call_1");
    assert_eq!(name, "echo");
    assert_eq!(value, "hi");
    assert_eq!(second, "rust");
    assert!(metrics_nil);
}

#[test]
fn an_err_chat_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::Chat(Err(Error::Interrupted))
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
    assert_eq!(message, "interrupted by Ctrl-C");
}

#[test]
fn an_ok_loop_answer_resumes_nil() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::<Error>::Loop(Ok(()))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result): (bool, Value) = lua
        .load("local ok, result = ...; return ok, result")
        .call(envelope)
        .expect("the envelope round-trips through Lua");
    assert!(ok);
    assert_eq!(result, Value::Nil, "a successful loop returns nil");
}

#[test]
fn an_err_loop_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::Loop(Err(Error::ContextExhausted {
        reason: crate::OverflowReason::Precheck,
    }))
    .into_envelope(&lua)
    .expect("the envelope renders");
    match retained {
        Some(Error::ContextExhausted {
            reason: crate::OverflowReason::Precheck,
        }) => {}
        other => panic!("expected the retained ContextExhausted error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let reason: String = lua
        .load("local err = ...; return err.reason")
        .call(result.clone())
        .expect("the exhaustion table carries its reason");
    assert_eq!(reason, "precheck", "the kind's field rides beside it");
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "context_exhausted");
    assert!(
        message.starts_with("context exhausted: "),
        "the envelope carries the typed exhaustion's message"
    );
}

#[test]
fn an_ok_user_input_answer_round_trips_text_and_availability() {
    let lua = Lua::new();
    let outcome = UserInputOutcome {
        text: "the operator's answer".to_owned(),
        available: true,
    };
    let (envelope, retained) = Answer::<Error>::UserInput(Ok(outcome))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, text, available): (bool, String, bool) = lua
        .load("local ok, text, available = ...; return ok, text, available")
        .call(envelope)
        .expect("the three resume values read back through Lua");
    assert!(ok);
    assert_eq!(text, "the operator's answer");
    assert!(available, "operator text resumes as available");
}

#[test]
fn an_unavailable_user_input_answer_resumes_the_fallback_as_unavailable() {
    let lua = Lua::new();
    let outcome = UserInputOutcome {
        text: "User input is unavailable in this host; continue without it.".to_owned(),
        available: false,
    };
    let (envelope, retained) = Answer::<Error>::UserInput(Ok(outcome))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, available): (bool, bool) = lua
        .load("local ok, text, available = ...; return ok, available")
        .call(envelope)
        .expect("the resume values read back through Lua");
    assert!(ok);
    assert!(
        !available,
        "the fallback sentence resumes with available false, so identical human text cannot spoof it"
    );
}

#[test]
fn an_err_user_input_answer_round_trips_and_retains_the_typed_error() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::UserInput(Err(Error::Lua("broker down".to_owned())))
        .into_envelope(&lua)
        .expect("the envelope renders");
    match retained {
        Some(Error::Lua(message)) => assert_eq!(message, "broker down"),
        other => panic!("expected the retained Lua error, got {other:?}"),
    }
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(!ok);
    let (kind, message) = failure_parts(&lua, result);
    assert_eq!(kind, "lua");
    assert_eq!(message, "broker down");
}
