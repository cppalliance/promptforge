//! Tests for the shared model-turn report: a completion's metadata
//! diagnostics reach the run log as `model_metadata_degraded` events, in
//! their fixed place after `model_turn_completed`; its metrics ride the
//! reply event; and its raw exchange feeds the debug capture.

use promptforge_model_client::client::{RawExchange, StreamAccumulator};
use promptforge_types::emitter::{DebugMode, EventSink};
use promptforge_types::metrics::{CallMetrics, Usage};
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
    report_with(completion, DebugMode::Off)
}

/// Reports `completion` as chat turn 3 under `debug` and returns the events
/// it pushed.
fn report_with(completion: Completion, debug: DebugMode) -> Vec<Event> {
    let sink = EventSink::default();
    let emitter = Emitter::root(sink.clone(), "run-1", debug);
    let _ = report_model_turn(&emitter, "Chat", 3, completion, ReplyOrigin::Chat);
    sink.take()
}

/// A text completion built without a transport: it carries no metrics and
/// no raw exchange until a builder adds them.
fn canned() -> Completion {
    Completion::from_result(CompletionResult::Text("hi".to_owned()), "canned-model")
        .expect("a text result is accepted")
}

/// The metrics every `AssistantReply` in `events` carries, in order.
fn reply_metrics(events: &[Event]) -> Vec<Option<CallMetrics>> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantReply { metrics, .. } => Some(metrics.clone()),
            _ => None,
        })
        .collect()
}

/// What one `Response` debug event carried.
#[derive(Debug, PartialEq)]
struct Captured {
    body: Value,
    finish_reason: Option<String>,
    reasoning_content: Option<String>,
}

/// The body of every `Request` debug event in `events`, in order.
fn requests(events: &[Event]) -> Vec<Value> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Request { body, .. } => Some(body.clone()),
            _ => None,
        })
        .collect()
}

/// Every `Response` debug event in `events`, in order.
fn responses(events: &[Event]) -> Vec<Captured> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Response {
                body,
                finish_reason,
                reasoning_content,
                ..
            } => Some(Captured {
                body: body.clone(),
                finish_reason: finish_reason.clone(),
                reasoning_content: reasoning_content.clone(),
            }),
            _ => None,
        })
        .collect()
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

#[test]
fn the_reply_event_carries_the_metrics_the_completion_holds() {
    let metrics = CallMetrics {
        usage: Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
            total_tokens: 10,
            cached_tokens: None,
            reasoning_tokens: None,
        }),
        llama: None,
        vllm: None,
        client: None,
    };
    let events = report(canned().with_metrics(metrics.clone()));
    assert_eq!(reply_metrics(&events), vec![Some(metrics)]);
}

#[test]
fn a_completion_with_no_metrics_reports_none_on_the_reply_event() {
    let events = report(canned());
    assert_eq!(
        reply_metrics(&events),
        vec![None],
        "the Engine adds no metrics of its own: {events:?}"
    );
}

#[test]
fn a_streamed_completion_reports_its_usage_on_the_reply_event() {
    let events = report(completion(json!({
        "model": "test-model",
        "usage": { "prompt_tokens": 5, "completion_tokens": 1, "total_tokens": 6 },
    })));
    let usage = reply_metrics(&events)
        .into_iter()
        .flatten()
        .find_map(|metrics| metrics.usage)
        .expect("the usage chunk reaches the reply event");
    assert_eq!(usage.total_tokens, 6);
}

#[test]
fn debug_capture_emits_the_request_and_response_from_the_raw_exchange() {
    let request = json!({ "model": "m", "messages": [{ "role": "user", "content": "ask" }] });
    let response = json!({ "choices": [{ "message": { "content": "hi" } }] });
    let completion = canned()
        .with_raw(RawExchange::new(request.clone(), response.clone()))
        .with_finish_reason("stop");
    let events = report_with(completion, DebugMode::On);
    assert_eq!(requests(&events), vec![request]);
    assert_eq!(
        responses(&events),
        vec![Captured {
            body: response,
            finish_reason: Some("stop".to_owned()),
            reasoning_content: None,
        }]
    );
    assert!(
        matches!(
            events.as_slice(),
            [
                Event::Request { turn: 3, .. },
                Event::Response { turn: 3, .. },
                Event::ModelTurnCompleted { .. },
                Event::AssistantReply { .. },
            ]
        ),
        "the pair comes first, before the completed-turn report: {events:?}"
    );
}

#[test]
fn debug_capture_of_a_streamed_completion_holds_the_bodies_the_reader_built() {
    let events = report_with(completion(json!({ "model": "test-model" })), DebugMode::On);
    assert_eq!(requests(&events), vec![json!({ "messages": [] })]);
    let captured = responses(&events);
    assert_eq!(captured.len(), 1, "{events:?}");
    assert_eq!(captured[0].body["choices"][0]["message"]["content"], "hi");
    assert_eq!(captured[0].finish_reason.as_deref(), Some("stop"));
}

#[test]
fn a_completion_with_no_raw_exchange_still_reports_its_pair_with_null_bodies() {
    let events = report_with(canned(), DebugMode::On);
    assert_eq!(requests(&events), vec![Value::Null], "{events:?}");
    assert_eq!(
        responses(&events),
        vec![Captured {
            body: Value::Null,
            finish_reason: None,
            reasoning_content: None,
        }],
        "{events:?}"
    );
}

#[test]
fn a_raw_exchange_is_not_captured_while_debug_is_off() {
    let completion = canned().with_raw(RawExchange::new(json!({}), json!({})));
    let events = report_with(completion, DebugMode::Off);
    assert!(requests(&events).is_empty(), "{events:?}");
    assert!(responses(&events).is_empty(), "{events:?}");
}
