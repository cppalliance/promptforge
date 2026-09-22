//! Answer-to-envelope rendering: every [`Answer`] variant round-trips through
//! Lua as the `(ok, result)` envelope and retains its typed error. The
//! `chat` answer's shapes are in `answer_chat`.

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
fn a_task_events_answer_resumes_event_tables_with_absent_fields_nil() {
    // Two events, one lifecycle and one content: the sequence keeps their
    // order, each table holds the event's serialized shape, and an
    // absent optional field (`finish_reason`, `metrics`) is nil rather
    // than the serde bridge's NULL sentinel, so an author's truth test
    // works. An empty answer is still a sequence.
    use promptforge_api_types::event::Event;
    use promptforge_api_types::ids::Provenance;
    let lua = Lua::new();
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let events = vec![
        Event::SectionStarted {
            execution: "run".to_owned(),
            section: "Child".to_owned(),
            provenance: Provenance {
                task: task.clone(),
                seq: 0,
            },
        },
        Event::AssistantReply {
            execution: "run".to_owned(),
            section: "Child".to_owned(),
            provenance: Provenance { task, seq: 3 },
            turn: 1,
            text: "hi".to_owned(),
            finish_reason: None,
            model: "m".to_owned(),
            metrics: None,
            origin: promptforge_api_types::event::ReplyOrigin::Chat,
        },
    ];
    let (envelope, retained) = Answer::<Error>::TaskEvents(Ok(events))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let summary: String = lua
        .load(
            "local events = ...\n\
             assert(#events == 2)\n\
             assert(events[2].finish_reason == nil, 'an absent field is nil')\n\
             assert(events[2].metrics == nil, 'an absent field is nil')\n\
             return events[1].kind .. '|' .. events[1].provenance.seq .. '|' \
             .. events[2].kind .. '|' .. events[2].provenance.seq .. '|' .. events[2].text",
        )
        .call(result)
        .expect("the event tables read back through Lua");
    assert_eq!(summary, "section_started|0|assistant_reply|3|hi");

    let (envelope, _) = Answer::<Error>::TaskEvents(Ok(Vec::new()))
        .into_envelope(&lua)
        .expect("the envelope renders");
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    assert!(
        matches!(&result, Value::Table(table) if table.raw_len() == 0),
        "an empty answer is an empty sequence, got {result:?}"
    );
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
fn a_when_any_delivery_of_a_failed_member_retains_the_members_typed_error() {
    // The wait succeeded, so the envelope is `(true, id, false, table)`,
    // but the member's failure is handed back typed as well: a shim that
    // re-raises it at once (`fanout` on a fatal arm) lets the driver
    // substitute the member's own error for the raised table.
    let lua = Lua::new();
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let (envelope, retained) = Answer::<Error>::WhenAny(Ok(TaskDelivery {
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
fn a_when_any_delivery_of_a_finished_member_retains_nothing() {
    let lua = Lua::new();
    let task: TaskId = "0.1".parse().expect("a task id parses");
    let (envelope, retained) = Answer::<Error>::WhenAny(Ok(TaskDelivery {
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
fn an_ok_drain_task_notices_answer_resumes_the_texts_as_a_sequence() {
    let lua = Lua::new();
    let notices = vec![
        "Task id=0.0 (## Child) completed: done".to_owned(),
        "Task id=0.1 (## Child) failed: boom".to_owned(),
    ];
    let (envelope, retained) = Answer::<Error>::DrainTaskNotices(Ok(notices))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, result) = echo_through_lua(&lua, envelope);
    assert!(ok);
    let Value::Table(sequence) = result else {
        panic!("expected a sequence result, got {result:?}");
    };
    let texts: Vec<String> = sequence
        .sequence_values::<String>()
        .collect::<mlua::Result<_>>()
        .expect("the notices read back as strings");
    assert_eq!(
        texts,
        vec![
            "Task id=0.0 (## Child) completed: done",
            "Task id=0.1 (## Child) failed: boom"
        ],
        "the notices resume in arrival order"
    );
}

#[test]
fn an_empty_drain_task_notices_answer_resumes_an_empty_sequence() {
    let lua = Lua::new();
    let (envelope, retained) = Answer::<Error>::DrainTaskNotices(Ok(Vec::new()))
        .into_envelope(&lua)
        .expect("the envelope renders");
    assert!(retained.is_none());
    let (ok, len): (bool, i64) = lua
        .load("local ok, notices = ...; return ok, #notices")
        .call(envelope)
        .expect("the sequence reads back through Lua");
    assert!(ok);
    assert_eq!(len, 0, "no notices resume as an empty sequence, never nil");
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
