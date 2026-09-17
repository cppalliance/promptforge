//! Per-frame wire-shape pins: each test asserts one frame against the
//! exact JSON literal the pre-refactor code built with
//! `serde_json::json!`, so a field rename, retype, or optionality change
//! fails here before it reaches a socket.

use workshop_protocol::{
    Activity, AgentDeltaFrame, AgentDeltaKind, AgentEventFrame, AgentSessionFrame, AgentsFrame,
    CatalogPush, ErrorEnvelope, ErrorFrame, InputFrame, InputResponse, Progress, Severity,
    StatusBarUpdate, SwitchProfileFrame, WorkbenchSnapshot,
};

/// Builds a minimal update with the given label.
fn stub(label: impl Into<String>) -> StatusBarUpdate {
    StatusBarUpdate {
        label: label.into(),
        description: String::new(),
        progress: None,
        severity: Severity::Info,
        activity: Activity::General,
    }
}

#[test]
fn a_status_update_serializes_as_a_status_frame() {
    let frame = serde_json::to_value(stub("Ready").frame()).expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "status",
            "label": "Ready",
            "description": "",
            "progress": null,
            "severity": "info",
            "activity": "general",
        }),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn progress_and_the_remaining_variants_serialize() {
    let update = StatusBarUpdate {
        progress: Some(Progress {
            current: 1,
            total: 2,
        }),
        severity: Severity::Error,
        activity: Activity::Thinking,
        ..stub("Working")
    };
    let frame = serde_json::to_value(update.frame()).expect("the frame serializes");
    assert_eq!(
        frame["progress"],
        serde_json::json!({"current": 1, "total": 2})
    );
    assert_eq!(frame["severity"], "error");
    assert_eq!(frame["activity"], "thinking");
    // Debug serializes too; the UI, not the bus, ignores it.
    let debug = serde_json::to_value(
        StatusBarUpdate {
            severity: Severity::Debug,
            activity: Activity::Generating,
            ..stub("x")
        }
        .frame(),
    )
    .expect("the frame serializes");
    assert_eq!(debug["severity"], "debug");
    assert_eq!(debug["activity"], "generating");
}

#[test]
fn a_catalog_push_serializes_as_a_models_frame() {
    let push = CatalogPush {
        models: vec![serde_json::json!({"id": "test-model", "object": "model"})],
    };
    let frame = serde_json::to_value(push.frame()).expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "models",
            "models": [{"id": "test-model", "object": "model"}],
        }),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn a_workbench_snapshot_serializes_as_a_workbench_frame() {
    let snapshot = WorkbenchSnapshot {
        profiles: vec!["main".to_string(), "coding".to_string()],
        active: Some("main".to_string()),
        switching: None,
        switch_in_flight: false,
        selected_model: Some("claude-sonnet-4-6".to_string()),
        chat_ready: true,
    };
    let frame = serde_json::to_value(snapshot.frame()).expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({
            "type": "workbench",
            "profiles": ["main", "coding"],
            "active": "main",
            "switching": null,
            "switch_in_flight": false,
            "selected": "claude-sonnet-4-6",
            "chat_ready": true,
        }),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn a_switch_to_no_profile_is_visible_on_the_workbench_frame() {
    let snapshot = WorkbenchSnapshot {
        profiles: vec!["main".to_string()],
        active: Some("main".to_string()),
        switching: None,
        switch_in_flight: true,
        selected_model: Some("claude-sonnet-4-6".to_string()),
        chat_ready: false,
    };
    let frame = serde_json::to_value(snapshot.frame()).expect("the frame serializes");
    assert_eq!(
        frame["switching"],
        serde_json::Value::Null,
        "a switch to no profile names no target"
    );
    assert_eq!(
        frame["switch_in_flight"], true,
        "the frame still reports the switch in flight, independent of the target name"
    );
}

#[test]
fn an_agents_frame_serializes_the_discovered_names() {
    let frame = serde_json::to_value(AgentsFrame::new(vec![
        "chat".to_owned(),
        "research".to_owned(),
    ]))
    .expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({"type": "agents", "agents": ["chat", "research"]}),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn an_agent_session_frame_serializes_its_id_and_agent() {
    let frame = serde_json::to_value(AgentSessionFrame::new("a1b2".to_owned(), "chat".to_owned()))
        .expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({"type": "agent_session", "session": "a1b2", "agent": "chat"}),
    );
}

#[test]
fn an_agent_event_frame_carries_its_log_index_and_optional_reply_id() {
    use promptforge_api_types::events::{RuntimeEvent, RuntimeEventKind};
    let event = RuntimeEvent {
        kind: RuntimeEventKind::UserInput,
        section: "chat".to_owned(),
        chain_id: 0,
        depth: 0,
        turn: 0,
        content: "hi".to_owned(),
        model: None,
        tool_call_id: None,
        finish_reason: None,
        metrics: None,
    };
    let plain = serde_json::to_value(AgentEventFrame::new(3, None, event.clone()))
        .expect("the frame serializes");
    assert_eq!(plain["type"], "agent_event");
    assert_eq!(plain["index"], 3, "the frame carries the entry's log index");
    assert!(
        plain.get("reply").is_none(),
        "an absent reply id is omitted from the wire, not serialized as null"
    );
    assert_eq!(
        plain["event"],
        serde_json::to_value(&event).expect("events serialize"),
        "the entry rides in its persisted vocabulary shape"
    );
    let stamped = serde_json::to_value(AgentEventFrame::new(4, Some(1), event))
        .expect("the frame serializes");
    assert_eq!(
        stamped["reply"], 1,
        "a superseding event is stamped with the reply id its deltas carried"
    );
}

#[test]
fn an_agent_delta_frame_is_stamped_with_its_superseding_reply_id() {
    let text = serde_json::to_value(AgentDeltaFrame::new(
        AgentDeltaKind::Text,
        "po".to_owned(),
        2,
    ))
    .expect("the frame serializes");
    assert_eq!(
        text,
        serde_json::json!({"type": "agent_delta", "kind": "text", "content": "po", "reply": 2}),
    );
    let reasoning = serde_json::to_value(AgentDeltaFrame::new(
        AgentDeltaKind::Reasoning,
        "hmm".to_owned(),
        2,
    ))
    .expect("the frame serializes");
    assert_eq!(
        reasoning,
        serde_json::json!({
            "type": "agent_delta", "kind": "reasoning", "content": "hmm", "reply": 2,
        }),
    );
}

#[test]
fn an_input_required_frame_serializes_with_its_token() {
    let frame = serde_json::to_value(InputFrame::Required {
        token: "a1b2c3".to_owned(),
    })
    .expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({"type": "input_required", "token": "a1b2c3"}),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn an_input_cancelled_frame_serializes_with_its_token() {
    let frame = serde_json::to_value(InputFrame::Cancelled {
        token: "a1b2c3".to_owned(),
    })
    .expect("the frame serializes");
    assert_eq!(
        frame,
        serde_json::json!({"type": "input_cancelled", "token": "a1b2c3"}),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn an_input_response_parses_its_body_byte_exact_ignoring_the_envelope() {
    let gnarly = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash 🦀";
    let response: InputResponse = serde_json::from_value(serde_json::json!({
        "type": "input_response",
        "token": "a1b2c3",
        "text": gnarly,
    }))
    .expect("the frame parses with its envelope tag present");
    assert_eq!(response.token, "a1b2c3");
    assert_eq!(
        response.text, gnarly,
        "the operator's text survives the wire byte-exact"
    );
}

#[test]
fn a_switch_profile_frame_accepts_a_name_or_null_and_refuses_the_rest() {
    let named: SwitchProfileFrame = serde_json::from_value(serde_json::json!({
        "type": "switch_profile",
        "name": "beta",
        "id": 4,
    }))
    .expect("a named selection parses with its envelope present");
    assert_eq!(named.name.as_deref(), Some("beta"));

    let none: SwitchProfileFrame =
        serde_json::from_value(serde_json::json!({"type": "switch_profile", "name": null}))
            .expect("null selects no profile");
    assert_eq!(none.name, None, "a null name is the no-profile selection");

    assert!(
        serde_json::from_value::<SwitchProfileFrame>(serde_json::json!({
            "type": "switch_profile"
        }))
        .is_err(),
        "an absent name is a malformed frame, not a no-profile selection"
    );
    assert!(
        serde_json::from_value::<SwitchProfileFrame>(serde_json::json!({
            "type": "switch_profile",
            "name": 7,
        }))
        .is_err(),
        "a non-string name is refused"
    );
}

#[test]
fn an_error_frame_serializes_with_and_without_the_echoed_id() {
    let untagged = serde_json::to_value(ErrorFrame::new("Gateway unreachable".to_string(), None))
        .expect("the frame serializes");
    assert_eq!(
        untagged,
        serde_json::json!({"type": "error", "message": "Gateway unreachable"})
    );
    let id = serde_json::json!(7);
    let tagged = serde_json::to_value(ErrorFrame::new(
        "Gateway unreachable".to_string(),
        Some(&id),
    ))
    .expect("the frame serializes");
    assert_eq!(
        tagged,
        serde_json::json!({"type": "error", "message": "Gateway unreachable", "id": 7})
    );
}

#[test]
fn an_error_envelope_serializes_as_message_and_code_under_error() {
    let envelope = ErrorEnvelope::new("file cannot be read", "read_file");
    assert_eq!(
        serde_json::to_value(&envelope).expect("the envelope serializes"),
        serde_json::json!({"error": {"message": "file cannot be read", "code": "read_file"}}),
        "the wire shape matches the envelope the shell has always answered with"
    );
}
