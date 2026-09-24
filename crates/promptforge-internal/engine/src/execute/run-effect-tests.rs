//! The answer record: a `Chat` answer projects to the reply or the
//! requested tool names beside the model and finish reason, never the
//! bodies; a `ToolCall` answer projects to its text and trust; and every
//! failure projects to its display text. Each record round-trips through
//! serde, as a run log and a replay depend on.

use promptforge_model_client::client::StreamAccumulator;
use serde_json::json;

use super::*;
use crate::model::ClientError;

/// Serializes the record and reads it back.
fn round_trip(record: &AnswerRecord) -> AnswerRecord {
    let text = serde_json::to_string(record).expect("an answer record serializes");
    serde_json::from_str(&text).expect("a serialized answer record deserializes")
}

/// A completion with `finish_reason` and both bodies set: what a
/// transport hands the loop, as opposed to the bare canned one. `delta`
/// is the one streamed chunk's delta.
fn completion(delta: &serde_json::Value, finish_reason: &str) -> Completion {
    let chunk = json!({
        "model": "test-model",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    });
    let mut accumulator = StreamAccumulator::new();
    accumulator
        .apply(&chunk.to_string(), &|_| {})
        .expect("a well-formed chunk applies");
    accumulator
        .finish(
            json!({ "messages": [{ "role": "user", "content": "ask" }] }),
            None,
        )
        .expect("a complete turn finishes")
}

#[test]
fn a_chat_answer_records_the_reply_model_and_finish_reason_without_the_bodies() {
    let answer = EffectAnswer::Chat(Ok(Box::new(completion(
        &json!({ "content": "the reply" }),
        "stop",
    ))));
    let record = answer.record();
    assert_eq!(
        record,
        AnswerRecord::Chat(Ok(ChatAnswerRecord {
            model: "test-model".to_owned(),
            finish_reason: Some("stop".to_owned()),
            reply: Some("the reply".to_owned()),
            tool_calls: Vec::new(),
        }))
    );
    let text = serde_json::to_string(&record).expect("an answer record serializes");
    assert!(
        !text.contains("choices") && !text.contains("messages"),
        "the round's bodies travel as debug events, not in the answer: {text}"
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_chat_answer_with_tool_calls_records_their_names_in_call_order_and_no_reply() {
    let calls = json!({ "tool_calls": [
        { "index": 0, "id": "call-1", "type": "function",
          "function": { "name": "grab", "arguments": "{\"value\":\"hi\"}" } },
        { "index": 1, "id": "call-2", "type": "function",
          "function": { "name": "echo", "arguments": "{}" } },
    ] });
    let answer = EffectAnswer::Chat(Ok(Box::new(completion(&calls, "tool_calls"))));
    let record = answer.record();
    assert_eq!(
        record,
        AnswerRecord::Chat(Ok(ChatAnswerRecord {
            model: "test-model".to_owned(),
            finish_reason: Some("tool_calls".to_owned()),
            reply: None,
            tool_calls: vec!["grab".to_owned(), "echo".to_owned()],
        }))
    );
    let text = serde_json::to_string(&record).expect("an answer record serializes");
    assert!(
        !text.contains("call-1") && !text.contains("value"),
        "the calls' ids and arguments stay in the tool-call event: {text}"
    );
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_canned_chat_answer_records_no_finish_reason() {
    let answer = EffectAnswer::Chat(Ok(Box::new(Completion::from_result(
        CompletionResult::Text("canned".to_owned()),
        "canned-model",
    ))));
    assert_eq!(
        answer.record(),
        AnswerRecord::Chat(Ok(ChatAnswerRecord {
            model: "canned-model".to_owned(),
            finish_reason: None,
            reply: Some("canned".to_owned()),
            tool_calls: Vec::new(),
        }))
    );
}

#[test]
fn a_failed_chat_answer_records_the_errors_display_text() {
    let error = CompletionError::from(ClientError::GatewayDisabled);
    let expected = error.to_string();
    assert!(!expected.is_empty(), "the error displays as something");
    let record = EffectAnswer::Chat(Err(error)).record();
    assert_eq!(record, AnswerRecord::Chat(Err(expected)));
    assert_eq!(round_trip(&record), record);
}

#[test]
fn a_tool_call_answer_records_the_outputs_text_and_trust() {
    let trusted = EffectAnswer::ToolCall(Ok(ToolOutput::trusted("done"))).record();
    assert_eq!(
        trusted,
        AnswerRecord::ToolCall(Ok(ToolAnswerRecord {
            text: "done".to_owned(),
            trusted: true,
        }))
    );
    assert_eq!(round_trip(&trusted), trusted);

    let untrusted = EffectAnswer::ToolCall(Ok(ToolOutput::untrusted("<html>"))).record();
    assert_eq!(
        untrusted,
        AnswerRecord::ToolCall(Ok(ToolAnswerRecord {
            text: "<html>".to_owned(),
            trusted: false,
        }))
    );
    assert_eq!(round_trip(&untrusted), untrusted);
}

#[test]
fn a_failed_tool_call_answer_records_the_errors_display_text_and_hides_its_source() {
    let error = ToolError::with_source("backend failed", std::io::Error::other("boom"));
    let record = EffectAnswer::ToolCall(Err(error)).record();
    assert_eq!(
        record,
        AnswerRecord::ToolCall(Err("backend failed".to_owned())),
        "the display text is the model-safe message; the source stays behind it"
    );
    assert_eq!(round_trip(&record), record);
}
