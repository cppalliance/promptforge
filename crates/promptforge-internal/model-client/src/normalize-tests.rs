//! Tests for the strict turn parse: tool calls, empty replies, and the
//! reasoning side channel.

use super::*;

const EMPTY_REPLY_PHRASE: &str = "the model replied with no text and no tool calls";

/// Whether `result` failed as a `MalformedResponse`.
fn is_malformed<T>(result: Result<T>) -> bool {
    result.is_err_and(|error| error.kind() == CompletionErrorKind::MalformedResponse)
}

/// Whether `result` failed as an `EmptyReply`.
fn is_empty_reply<T>(result: Result<T>) -> bool {
    result.is_err_and(|error| error.kind() == CompletionErrorKind::EmptyReply)
}

/// Wraps one assistant message in the gateway's one-choice envelope.
fn one_choice(message: impl Into<Value>) -> Value {
    let message = message.into();
    serde_json::json!({ "choices": [{ "message": message }] })
}

/// Wraps one raw tool call in a null-content assistant message.
fn one_tool_call(call: impl Into<Value>) -> Value {
    let call = call.into();
    one_choice(serde_json::json!({
        "role": "assistant",
        "content": null,
        "tool_calls": [call]
    }))
}

#[test]
fn answer_and_reasoning_keeps_side_channel() {
    let body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "answer",
                "reasoning_content": "scratch work"
            },
            "finish_reason": "stop"
        }]
    });

    let turn = normalize(&body).unwrap();
    assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
    assert_eq!(turn.reasoning_content.as_deref(), Some("scratch work"));
    match turn.outcome {
        CompletionResult::Text(text) => assert_eq!(text, "answer"),
        CompletionResult::ToolCalls(_) => panic!("expected text, got tool calls"),
    }
}

#[test]
fn tools_with_empty_content_succeed() {
    let body = serde_json::json!({
        "choices": [{
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "id": "call_1",
                    "type": "function",
                    "function": {
                        "name": "web_search",
                        "arguments": "{\"query\":\"rust\",\"count\":3}"
                    }
                }]
            },
            "finish_reason": "tool_calls"
        }]
    });

    let turn = normalize(&body).unwrap();
    assert_eq!(turn.finish_reason.as_deref(), Some("tool_calls"));
    match turn.outcome {
        CompletionResult::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_1");
            assert_eq!(calls[0].name, "web_search");
            assert_eq!(
                calls[0].arguments,
                serde_json::json!({ "query": "rust", "count": 3 })
            );
        }
        CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
    }
}

#[test]
fn tools_with_null_content_succeed() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_2",
        "type": "function",
        "function": { "name": "web_fetch", "arguments": "{\"url\":\"https://example.com\"}" }
    }));

    let turn = normalize(&body).unwrap();
    match turn.outcome {
        CompletionResult::ToolCalls(calls) => {
            assert_eq!(
                calls[0].arguments,
                serde_json::json!({ "url": "https://example.com" })
            );
        }
        CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
    }
}

#[test]
fn malformed_tool_arguments_are_rejected_not_coerced() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_bad",
        "type": "function",
        "function": { "name": "web_fetch", "arguments": "not json" }
    }));

    assert!(
        is_malformed(normalize(&body)),
        "invalid-JSON tool arguments must be rejected, never coerced to a string"
    );
}

#[test]
fn non_string_tool_arguments_are_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_obj",
        "type": "function",
        "function": { "name": "web_fetch", "arguments": { "url": "x" } }
    }));

    assert!(is_malformed(normalize(&body)));
}

#[test]
fn absent_tool_arguments_are_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_none",
        "type": "function",
        "function": { "name": "ping" }
    }));

    assert!(
        is_malformed(normalize(&body)),
        "missing tool arguments must be rejected, not coerced to null"
    );
}

#[test]
fn non_object_decoded_arguments_are_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_arr",
        "type": "function",
        "function": { "name": "ping", "arguments": "[1,2,3]" }
    }));

    assert!(
        is_malformed(normalize(&body)),
        "arguments that decode to a non-object must be rejected"
    );
}

#[test]
fn blank_tool_call_id_is_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "   ",
        "type": "function",
        "function": { "name": "ping", "arguments": "{}" }
    }));
    assert!(is_malformed(normalize(&body)));
}

#[test]
fn duplicate_tool_call_ids_are_rejected() {
    let body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    { "id": "dup", "type": "function", "function": { "name": "a", "arguments": "{}" } },
                    { "id": "dup", "type": "function", "function": { "name": "b", "arguments": "{}" } }
                ]
            }
        }]
    });
    assert!(is_malformed(normalize(&body)));
}

#[test]
fn wrong_type_type_field_is_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_x",
        "type": "not_function",
        "function": { "name": "ping", "arguments": "{}" }
    }));
    assert!(is_malformed(normalize(&body)));
}

#[test]
fn missing_or_null_type_field_is_rejected() {
    // `type` is required to be exactly "function"; a missing or
    // null value is malformed rather than tacitly accepted.
    for type_field in [None, Some(serde_json::Value::Null)] {
        let mut call = serde_json::json!({
            "id": "call_x",
            "function": { "name": "ping", "arguments": "{}" }
        });
        if let Some(value) = type_field {
            call["type"] = value;
        }
        let body = one_tool_call(call);
        assert!(is_malformed(normalize(&body)));
    }
}

#[test]
fn wrong_typed_top_level_fields_are_malformed() {
    // choices not an array
    assert!(is_malformed(normalize(
        &serde_json::json!({ "choices": {} })
    )));
    // message not an object
    assert!(is_malformed(normalize(
        &serde_json::json!({ "choices": [{ "message": 7 }] })
    )));
    // finish_reason not a string
    assert!(is_malformed(normalize(&serde_json::json!({
        "choices": [{ "message": { "content": "hi" }, "finish_reason": 3 }]
    }))));
    // content wrong type
    assert!(is_malformed(normalize(&serde_json::json!({
        "choices": [{ "message": { "content": [] } }]
    }))));
    // tool_calls wrong type
    assert!(is_malformed(normalize(&serde_json::json!({
        "choices": [{ "message": { "content": null, "tool_calls": {} } }]
    }))));
    // reasoning wrong type
    assert!(is_malformed(normalize(&serde_json::json!({
        "choices": [{ "message": { "content": "hi", "reasoning_content": 5 } }]
    }))));
}

#[test]
fn whitespace_only_content_is_empty_reply() {
    let body = one_choice(serde_json::json!({ "content": "   \n\t " }));
    assert!(is_empty_reply(normalize(&body)));
}

#[test]
fn empty_content_with_reasoning_is_error() {
    let body = serde_json::json!({
        "choices": [{
            "message": {
                "role": "assistant",
                "content": "",
                "reasoning_content": "only thinking"
            },
            "finish_reason": "stop"
        }]
    });

    let error = normalize(&body).expect_err("an empty turn must fail");
    assert_eq!(error.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(
        error.to_string(),
        format!("{EMPTY_REPLY_PHRASE}: reasoning content was present but ignored")
    );
    assert_eq!(
        error.finish_reason(),
        Some("stop"),
        "the choice's finish_reason must survive on the error"
    );
}

#[test]
fn empty_string_content_without_tools_is_error() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": ""
    }));

    let error = normalize(&body).expect_err("an empty turn must fail");
    assert_eq!(error.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(error.to_string(), EMPTY_REPLY_PHRASE);
    assert_eq!(error.finish_reason(), None, "no finish_reason on the wire");
}

#[test]
fn null_content_without_tools_is_error() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": null
    }));

    let error = normalize(&body).expect_err("an empty turn must fail");
    assert_eq!(error.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(error.to_string(), EMPTY_REPLY_PHRASE);
}

#[test]
fn empty_reply_error_stores_the_finish_reason() {
    let with_reason = empty_reply_error(false, Some("length".to_owned()));
    assert_eq!(
        with_reason.finish_reason(),
        Some("length"),
        "a supplied finish_reason must be stored: {with_reason:?}"
    );
    assert_eq!(with_reason.to_string(), EMPTY_REPLY_PHRASE);

    let without_reason = empty_reply_error(true, None);
    assert_eq!(
        without_reason.finish_reason(),
        None,
        "a missing finish_reason stays missing: {without_reason:?}"
    );
    assert_eq!(
        without_reason.to_string(),
        format!("{EMPTY_REPLY_PHRASE}: reasoning content was present but ignored")
    );
}

#[test]
fn synonym_reasoning_field_is_side_channel() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": "answer",
        "reasoning": "via synonym"
    }));

    let turn = normalize(&body).unwrap();
    assert_eq!(turn.reasoning_content.as_deref(), Some("via synonym"));
    match turn.outcome {
        CompletionResult::Text(text) => assert_eq!(text, "answer"),
        CompletionResult::ToolCalls(_) => panic!("expected text, got tool calls"),
    }
}

#[test]
fn empty_reasoning_synonym_falls_through() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": "answer",
        "reasoning_content": "",
        "thinking": "from thinking"
    }));

    let turn = normalize(&body).unwrap();
    assert_eq!(turn.reasoning_content.as_deref(), Some("from thinking"));
}

#[test]
fn missing_content_and_tools_is_empty_model_reply() {
    let body = one_choice(serde_json::json!({ "role": "assistant" }));

    assert!(is_empty_reply(normalize(&body)));
}

#[test]
fn no_choices_is_malformed() {
    let body = serde_json::json!({ "choices": [] });
    assert!(is_malformed(normalize(&body)));
}

#[test]
fn tool_code_fence_stays_text_in_openai_normalizer() {
    let content = "```tool_code\nsearch(query=\"C++ Alliance founder\")\n```";
    let body = serde_json::json!({
        "choices": [{
            "message": { "role": "assistant", "content": content },
            "finish_reason": "stop"
        }]
    });

    let turn = normalize(&body).unwrap();
    match turn.outcome {
        CompletionResult::Text(text) => assert_eq!(text, content),
        CompletionResult::ToolCalls(_) => {
            panic!("OpenAI normalizer must not parse content fences")
        }
    }
}

#[test]
fn fenced_json_tool_calls_stays_text_in_openai_normalizer() {
    let content = "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"{\\\"url\\\":\\\"https://example.com\\\"}\"}}]}\n```";
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": content
    }));

    let turn = normalize(&body).unwrap();
    match turn.outcome {
        CompletionResult::Text(text) => assert_eq!(text, content),
        CompletionResult::ToolCalls(_) => {
            panic!("OpenAI normalizer must not parse content fences")
        }
    }
}
