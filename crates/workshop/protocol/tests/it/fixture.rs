//! The shared agent-frame fixture pins: the same JSON the SPA suite
//! (`crates/workshop/ui/test/agent-wire-fixtures.mjs`) asserts, so a wire
//! drift on either side fails that side's fixture test.

use workshop_protocol::{
    AgentDeltaFrame, AgentDeltaKind, AgentEventFrame, AgentSessionFrame, AgentsFrame, InputFrame,
    InputResponse,
};

/// The shared agent-frame fixture, asserted as the same JSON by the SPA
/// suite: a wire drift on either side fails that side's fixture test.
const AGENT_FRAME_FIXTURE: &str = include_str!("../fixtures/agent-frames.json");

/// Parses the shared fixture into one object keyed by case name.
fn agent_fixture() -> serde_json::Value {
    match serde_json::from_str(AGENT_FRAME_FIXTURE) {
        Ok(fixture) => fixture,
        Err(error) => panic!("the fixture is valid JSON: {error}"),
    }
}

/// The fixture's `agent_event_minimal` entry as the vocabulary type.
fn minimal_fixture_event() -> promptforge_api_types::events::RuntimeEvent {
    use promptforge_api_types::events::{RuntimeEvent, RuntimeEventKind};
    RuntimeEvent {
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
    }
}

/// The fixture's `agent_event_stamped` entry as the vocabulary type,
/// every metrics section populated.
fn stamped_fixture_event() -> promptforge_api_types::events::RuntimeEvent {
    use promptforge_api_types::events::{
        CallMetrics, ClientTiming, LlamaTimings, RuntimeEvent, RuntimeEventKind, Usage, VllmMetrics,
    };
    RuntimeEvent {
        kind: RuntimeEventKind::AssistantReply,
        section: "chat".to_owned(),
        chain_id: 1,
        depth: 0,
        turn: 2,
        content: "hello".to_owned(),
        model: Some("llama-3".to_owned()),
        tool_call_id: None,
        finish_reason: Some("stop".to_owned()),
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
    assert_eq!(
        serde_json::to_value(AgentsFrame::new(vec![
            "chat".to_owned(),
            "research".to_owned()
        ]))
        .expect("the frame serializes"),
        fixture["agents"]
    );
    assert_eq!(
        serde_json::to_value(AgentSessionFrame::new("a1b2".to_owned(), "chat".to_owned()))
            .expect("the frame serializes"),
        fixture["agent_session"]
    );
    assert_eq!(
        serde_json::to_value(AgentEventFrame::new(3, None, minimal_fixture_event()))
            .expect("the frame serializes"),
        fixture["agent_event_minimal"]
    );
    assert_eq!(
        serde_json::to_value(AgentEventFrame::new(4, Some(1), stamped_fixture_event()))
            .expect("the frame serializes"),
        fixture["agent_event_stamped"],
        "the event rides in its persisted vocabulary shape, metrics and all"
    );
    assert_eq!(
        serde_json::to_value(AgentDeltaFrame::new(
            AgentDeltaKind::Text,
            "po".to_owned(),
            2
        ))
        .expect("the frame serializes"),
        fixture["agent_delta_text"]
    );
    assert_eq!(
        serde_json::to_value(AgentDeltaFrame::new(
            AgentDeltaKind::Reasoning,
            "hmm".to_owned(),
            2
        ))
        .expect("the frame serializes"),
        fixture["agent_delta_reasoning"]
    );
    assert_eq!(
        serde_json::to_value(InputFrame::Required {
            token: "a1b2c3".to_owned(),
        })
        .expect("the frame serializes"),
        fixture["input_required"]
    );
    assert_eq!(
        serde_json::to_value(InputFrame::Cancelled {
            token: "a1b2c3".to_owned(),
        })
        .expect("the frame serializes"),
        fixture["input_cancelled"]
    );
}

#[test]
fn client_to_server_agent_frames_match_the_shared_fixture() {
    // `input_response` parses through its typed body; `launch`,
    // `attach`, and `cancel` are routed from raw JSON in
    // `session_agents::socket`, so the fixture pins exactly the fields
    // that routing reads.
    let fixture = agent_fixture();
    let response: InputResponse = serde_json::from_value(fixture["input_response"].clone())
        .expect("the fixture input_response parses");
    assert_eq!(response.token, "a1b2c3");
    assert_eq!(response.text, "two words");
    assert_eq!(fixture["launch"]["type"], "launch");
    assert_eq!(fixture["launch"]["agent"], "chat");
    assert_eq!(fixture["attach"]["type"], "attach");
    assert_eq!(fixture["attach"]["session"], "a1b2");
    assert_eq!(fixture["cancel"]["type"], "cancel");
}
