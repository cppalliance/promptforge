//! Wire-shape pins for the agent-session frames: each outbound frame
//! against its exact JSON, the inbound requests against their variants
//! and refusals, and both directions against the shared fixture the SPA
//! suite (`crates/workshop/ui/test/agent-wire-fixtures.mjs`) asserts, so a
//! wire drift on either side fails that side's fixture test.

use std::path::Path;

use promptforge::event::{Event, ReplyOrigin};
use promptforge::ids::{ChainId, Provenance, TaskId};
use promptforge::metrics::{
    CallMetrics, ClientTiming, LlamaTimings, ToolCallEvent, Usage, VllmMetrics,
};
use workshop_protocol::{InputFrame, InputResponse};

use super::*;

/// Parses the shared fixture into one object keyed by case name.
fn agent_fixture() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agent-frames.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("the fixture {} is readable: {error}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|error| panic!("the fixture is valid JSON: {error}"))
}

/// The root task's zeroth sequence: the provenance every test event
/// holds, since the wire does not expose it.
fn provenance() -> Provenance {
    Provenance {
        task: TaskId::from(ChainId::root()),
        seq: 0,
    }
}

/// A user-input event in the `chat` section: the fixture's
/// `agent_event_minimal` entry as the engine event it projects from.
fn user_input(text: &str) -> Event {
    Event::UserInput {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        text: text.to_owned(),
    }
}

/// The fixture's `agent_event_stamped` entry as the engine event it
/// projects from, every metrics section populated.
fn stamped_fixture_event() -> Event {
    Event::AssistantReply {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        turn: 2,
        text: "hello".to_owned(),
        finish_reason: Some("stop".to_owned()),
        model: "llama-3".to_owned(),
        origin: ReplyOrigin::Chat,
        metrics: Some(CallMetrics {
            usage: Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: Some(2),
                reasoning_tokens: Some(1),
            }),
            llama: Some(LlamaTimings {
                prompt_n: 7,
                prompt_ms: 12.5,
                prompt_per_second: 560.0,
                predicted_n: 3,
                predicted_ms: 30.5,
                predicted_per_second: 98.5,
                draft_n: 4,
                draft_n_accepted: 2,
            }),
            vllm: Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: Some(22.5),
                queue_time_ms: Some(1.5),
                mean_itl_ms: Some(7.5),
                tokens_per_second: Some(133.5),
            }),
            client: Some(ClientTiming {
                ttft_ms: Some(9.5),
                mean_itl_ms: Some(8.25),
                e2e_ms: 41.5,
            }),
        }),
    }
}

/// Serializes one frame to its JSON value.
fn wire(frame: &impl Serialize) -> serde_json::Value {
    serde_json::to_value(frame).expect("the frame serializes")
}

#[test]
fn an_agents_frame_serializes_the_discovered_names() {
    assert_eq!(
        wire(&AgentsFrame::new(vec![
            "chat".to_owned(),
            "research".to_owned()
        ])),
        serde_json::json!({"type": "agents", "agents": ["chat", "research"]}),
        "the wire shape matches the workshop protocol's frame taxonomy"
    );
}

#[test]
fn an_agent_session_frame_serializes_its_id_and_agent() {
    assert_eq!(
        wire(&AgentSessionFrame::new(
            "a1b2".to_owned(),
            "chat".to_owned()
        )),
        serde_json::json!({"type": "agent_session", "session": "a1b2", "agent": "chat"}),
    );
}

#[test]
fn an_agent_event_frame_has_its_log_index_and_optional_reply_id() {
    let event = user_input("hi");
    let plain = wire(&AgentEventFrame::new(3, None, &event).expect("a user-input event frames"));
    assert_eq!(plain["type"], "agent_event");
    assert_eq!(plain["index"], 3, "the frame reports the entry's log index");
    assert!(
        plain.get("reply").is_none(),
        "an absent reply id is omitted from the wire, not serialized as null"
    );
    assert_eq!(
        plain["event"],
        serde_json::json!({
            "kind": "user_message", "section": "chat", "turn": 0, "content": "hi",
        }),
        "the entry serializes in its ACP-labelled wire shape"
    );
    let stamped =
        wire(&AgentEventFrame::new(4, Some(1), &event).expect("a user-input event frames"));
    assert_eq!(
        stamped["reply"], 1,
        "a superseding event is stamped with the same reply id as its deltas"
    );
}

#[test]
fn an_agent_event_frame_renders_tool_call_batches_and_skips_lifecycle_events() {
    let batch = Event::AssistantToolCalls {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        turn: 1,
        model: "llama-3".to_owned(),
        calls: vec![ToolCallEvent {
            id: "call_1".to_owned(),
            name: "read_file".to_owned(),
            arguments: serde_json::json!({ "path": "notes.txt" }),
        }],
    };
    let frame = wire(&AgentEventFrame::new(0, Some(0), &batch).expect("a tool-call batch frames"));
    assert_eq!(frame["event"]["kind"], "tool_call");
    assert_eq!(
        frame["event"]["content"],
        r#"[{"id":"call_1","name":"read_file","arguments":{"path":"notes.txt"}}]"#,
        "a batch renders as the JSON array of its calls in one string field"
    );
    assert_eq!(frame["event"]["model"], "llama-3");

    let lifecycle = Event::SectionStarted {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
    };
    assert!(
        AgentEventFrame::new(1, None, &lifecycle).is_none(),
        "a lifecycle event has no wire label and never frames"
    );
}

#[test]
fn an_agent_event_frame_keeps_the_model_on_thinking_and_the_call_id_on_tool_results() {
    let thinking = Event::Thinking {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        turn: 2,
        model: "llama-3".to_owned(),
        text: "weighing the options".to_owned(),
    };
    let thought =
        wire(&AgentEventFrame::new(5, Some(2), &thinking).expect("a thinking event frames"));
    assert_eq!(
        thought["event"],
        serde_json::json!({
            "kind": "agent_thought", "section": "chat", "turn": 2,
            "content": "weighing the options", "model": "llama-3",
        }),
        "a thinking block keeps its model and omits the tool-call id"
    );

    let result = Event::ToolResult {
        execution: "run".to_owned(),
        section: "chat".to_owned(),
        provenance: provenance(),
        turn: 2,
        tool_call_id: "call_7".to_owned(),
        alias: "read_file".to_owned(),
        content: "the file's text".to_owned(),
        trusted: false,
    };
    let update = wire(&AgentEventFrame::new(6, None, &result).expect("a tool-result event frames"));
    assert_eq!(
        update["event"],
        serde_json::json!({
            "kind": "tool_call_update", "section": "chat", "turn": 2,
            "content": "the file's text", "tool_call_id": "call_7",
        }),
        "a tool result keeps the id it answers and its content, and omits the model"
    );
}

#[test]
fn an_agent_delta_frame_is_stamped_with_its_superseding_reply_id() {
    assert_eq!(
        wire(&AgentDeltaFrame::new(
            AgentDeltaKind::Text,
            "po".to_owned(),
            2
        )),
        serde_json::json!({"type": "agent_delta", "kind": "text", "content": "po", "reply": 2}),
    );
    assert_eq!(
        wire(&AgentDeltaFrame::new(
            AgentDeltaKind::Reasoning,
            "hmm".to_owned(),
            2
        )),
        serde_json::json!({
            "type": "agent_delta", "kind": "reasoning", "content": "hmm", "reply": 2,
        }),
    );
}

#[test]
fn each_request_frame_parses_into_its_variant() {
    let cases = [
        (
            serde_json::json!({"type": "launch", "agent": "chat"}),
            SessionRequest::Launch {
                agent: "chat".to_owned(),
            },
        ),
        (
            serde_json::json!({"type": "attach", "session": "a1b2"}),
            SessionRequest::Attach {
                session: "a1b2".to_owned(),
            },
        ),
        (
            serde_json::json!({"type": "cancel"}),
            SessionRequest::Cancel,
        ),
        (
            serde_json::json!({"type": "cancel", "id": 4}),
            SessionRequest::Cancel,
        ),
    ];
    for (frame, expected) in cases {
        assert_eq!(SessionRequest::parse(&frame), Ok(expected), "{frame}");
    }
}

#[test]
fn malformed_request_frames_are_refused_with_the_socket_messages() {
    let unknown = "unknown frame type; expected \"launch\", \"attach\", \"input_response\", \
                   or \"cancel\"";
    let cases = [
        (
            serde_json::json!({"type": "launch"}),
            "launch frame without an agent name",
        ),
        (
            serde_json::json!({"type": "launch", "agent": 7}),
            "launch frame without an agent name",
        ),
        (
            serde_json::json!({"type": "attach"}),
            "attach frame without a session id",
        ),
        (
            serde_json::json!({"type": "attach", "session": null}),
            "attach frame without a session id",
        ),
        (serde_json::json!({"type": "mystery"}), unknown),
        (serde_json::json!({"agent": "chat"}), unknown),
        (serde_json::json!({"type": 7}), unknown),
        (serde_json::json!(["cancel"]), unknown),
        (serde_json::json!("cancel"), unknown),
    ];
    for (frame, message) in cases {
        let refusal = SessionRequest::parse(&frame).expect_err("the frame is refused");
        assert_eq!(refusal.to_string(), message, "{frame}");
    }
}

#[test]
fn the_shared_fixture_pins_exactly_the_agreed_case_list() {
    let fixture = agent_fixture();
    let mut cases: Vec<&str> = fixture
        .as_object()
        .expect("the fixture is one object keyed by case name")
        .keys()
        .map(String::as_str)
        .collect();
    cases.sort_unstable();
    assert_eq!(
        cases,
        [
            "agent_delta_reasoning",
            "agent_delta_text",
            "agent_event_minimal",
            "agent_event_stamped",
            "agent_session",
            "agents",
            "attach",
            "cancel",
            "input_cancelled",
            "input_required",
            "input_response",
            "launch",
        ],
        "both suites pin exactly the same case list, so a case added on \
         one side fails the other"
    );
}

#[test]
fn server_to_client_agent_frames_match_the_shared_fixture() {
    // Each typed frame serializes to its fixture entry, compared as
    // values so key order in the file is free.
    let fixture = agent_fixture();
    let agents = AgentsFrame::new(vec!["chat".to_owned(), "research".to_owned()]);
    assert_eq!(wire(&agents), fixture["agents"]);
    let session = AgentSessionFrame::new("a1b2".to_owned(), "chat".to_owned());
    assert_eq!(wire(&session), fixture["agent_session"]);
    assert_eq!(
        wire(&AgentEventFrame::new(3, None, &user_input("hi"))),
        fixture["agent_event_minimal"]
    );
    assert_eq!(
        wire(&AgentEventFrame::new(4, Some(1), &stamped_fixture_event())),
        fixture["agent_event_stamped"],
        "the event serializes in its wire shape, metrics and all"
    );
    let text = AgentDeltaFrame::new(AgentDeltaKind::Text, "po".to_owned(), 2);
    assert_eq!(wire(&text), fixture["agent_delta_text"]);
    let reasoning = AgentDeltaFrame::new(AgentDeltaKind::Reasoning, "hmm".to_owned(), 2);
    assert_eq!(wire(&reasoning), fixture["agent_delta_reasoning"]);
    let token = || "a1b2c3".to_owned();
    assert_eq!(
        wire(&InputFrame::Required { token: token() }),
        fixture["input_required"]
    );
    assert_eq!(
        wire(&InputFrame::Cancelled { token: token() }),
        fixture["input_cancelled"]
    );
}

#[test]
fn client_to_server_agent_frames_match_the_shared_fixture() {
    let fixture = agent_fixture();
    let response: InputResponse = serde_json::from_value(fixture["input_response"].clone())
        .expect("the fixture input_response parses");
    assert_eq!(response.token, "a1b2c3");
    assert_eq!(response.text, "two words");
    assert_eq!(
        SessionRequest::parse(&fixture["launch"]),
        Ok(SessionRequest::Launch {
            agent: "chat".to_owned()
        })
    );
    assert_eq!(
        SessionRequest::parse(&fixture["attach"]),
        Ok(SessionRequest::Attach {
            session: "a1b2".to_owned()
        })
    );
    assert_eq!(
        SessionRequest::parse(&fixture["cancel"]),
        Ok(SessionRequest::Cancel)
    );
}
