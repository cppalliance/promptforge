//! Tests for the shared model-turn report: a completion's metadata
//! diagnostics reach the run log as `model_metadata_degraded` events, in
//! their fixed place after `model_turn_completed`.

use promptforge_model_client::client::StreamAccumulator;
use promptforge_types::emitter::{DebugMode, EventSink};
use serde_json::{Value, json};

use super::*;

/// A text completion streamed as one chunk whose top level is `metadata`
/// (the `model` and any metrics sections) beside one text choice.
fn completion(metadata: Value) -> Completion {
    let mut chunk = metadata;
    chunk["choices"] =
        json!([{ "index": 0, "delta": { "content": "hi" }, "finish_reason": "stop" }]);
    let mut accumulator = StreamAccumulator::new();
    accumulator
        .apply(&chunk.to_string(), &|_| {})
        .expect("a well-formed chunk applies");
    accumulator
        .finish(json!({ "messages": [] }), None)
        .expect("a complete turn finishes")
}

/// Reports `completion` as chat turn 3 and returns the events it pushed.
fn report(completion: Completion) -> Vec<Event> {
    let sink = EventSink::default();
    let emitter = Emitter::root(sink.clone(), "run-1", DebugMode::Off);
    let _ = report_model_turn(&emitter, "Chat", 3, completion, ReplyOrigin::Chat);
    sink.take()
}

/// The `(turn, message)` of every `ModelMetadataDegraded` in `events`.
fn degraded(events: &[Event]) -> Vec<(u32, &str)> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ModelMetadataDegraded { turn, message, .. } => Some((*turn, message.as_str())),
            _ => None,
        })
        .collect()
}

#[test]
fn a_malformed_metadata_section_reports_a_degraded_event_after_the_completed_turn() {
    let events = report(completion(json!({
        "model": "test-model",
        "usage": { "prompt_tokens": "lots", "completion_tokens": 1, "total_tokens": 2 },
    })));

    let found = degraded(&events);
    assert_eq!(
        found.len(),
        1,
        "one malformed section, one event: {events:?}"
    );
    let (turn, message) = found[0];
    assert_eq!(turn, 3, "the event carries the turn it degraded");
    assert!(
        message.starts_with("malformed `usage` in completion response ignored: "),
        "the message names the section: {message}"
    );
    assert!(
        matches!(
            events.as_slice(),
            [
                Event::ModelTurnCompleted { section, .. },
                Event::ModelMetadataDegraded { .. },
                Event::AssistantReply { .. },
            ] if section == "Chat"
        ),
        "the report sits between the completed turn and the reply: {events:?}"
    );
}

#[test]
fn each_malformed_section_and_a_missing_model_report_once_each() {
    let events = report(completion(json!({
        "usage": "not an object",
        "metrics": ["not", "an", "object"],
    })));
    let messages: Vec<&str> = degraded(&events)
        .into_iter()
        .map(|(_, message)| message)
        .collect();
    assert_eq!(messages.len(), 3, "{events:?}");
    assert_eq!(
        messages[0],
        "completion response named no string `model`; recorded as empty"
    );
    assert!(messages[1].starts_with("malformed `usage`"), "{messages:?}");
    assert!(
        messages[2].starts_with("malformed `metrics`"),
        "{messages:?}"
    );
}

#[test]
fn well_formed_metadata_reports_no_degraded_event() {
    let events = report(completion(json!({
        "model": "test-model",
        "usage": { "prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6 },
    })));
    assert!(degraded(&events).is_empty(), "{events:?}");
}
