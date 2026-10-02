//! Tests for the strict turn parse: tool calls, empty replies, and the
//! reasoning side channel.

use super::*;

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
        matches!(normalize(&body), Err(Error::MalformedResponse(_))),
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

    assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
}

#[test]
fn absent_tool_arguments_are_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_none",
        "type": "function",
        "function": { "name": "ping" }
    }));

    assert!(
        matches!(normalize(&body), Err(Error::MalformedResponse(_))),
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
        matches!(normalize(&body), Err(Error::MalformedResponse(_))),
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
    assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
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
    assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
}

#[test]
fn wrong_type_type_field_is_rejected() {
    let body = one_tool_call(serde_json::json!({
        "id": "call_x",
        "type": "not_function",
        "function": { "name": "ping", "arguments": "{}" }
    }));
    assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
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
        assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
    }
}

#[test]
fn wrong_typed_top_level_fields_are_malformed() {
    // choices not an array
    assert!(matches!(
        normalize(&serde_json::json!({ "choices": {} })),
        Err(Error::MalformedResponse(_))
    ));
    // message not an object
    assert!(matches!(
        normalize(&serde_json::json!({ "choices": [{ "message": 7 }] })),
        Err(Error::MalformedResponse(_))
    ));
    // finish_reason not a string
    assert!(matches!(
        normalize(&serde_json::json!({
            "choices": [{ "message": { "content": "hi" }, "finish_reason": 3 }]
        })),
        Err(Error::MalformedResponse(_))
    ));
    // content wrong type
    assert!(matches!(
        normalize(&serde_json::json!({
            "choices": [{ "message": { "content": [] } }]
        })),
        Err(Error::MalformedResponse(_))
    ));
    // tool_calls wrong type
    assert!(matches!(
        normalize(&serde_json::json!({
            "choices": [{ "message": { "content": null, "tool_calls": {} } }]
        })),
        Err(Error::MalformedResponse(_))
    ));
    // reasoning wrong type
    assert!(matches!(
        normalize(&serde_json::json!({
            "choices": [{ "message": { "content": "hi", "reasoning_content": 5 } }]
        })),
        Err(Error::MalformedResponse(_))
    ));
}

#[test]
fn whitespace_only_content_is_empty_reply() {
    let body = one_choice(serde_json::json!({ "content": "   \n\t " }));
    assert!(matches!(
        normalize(&body),
        Err(Error::EmptyModelReply { .. })
    ));
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

    match normalize(&body) {
        Err(Error::EmptyModelReply {
            detail,
            finish_reason,
        }) => {
            assert_eq!(detail, EMPTY_REPLY_REASONING_IGNORED);
            assert_eq!(
                finish_reason.as_deref(),
                Some("stop"),
                "the choice's finish_reason must survive on the error"
            );
        }
        other => panic!("expected EmptyModelReply, got {other:?}"),
    }
}

#[test]
fn empty_string_content_without_tools_is_error() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": ""
    }));

    match normalize(&body) {
        Err(Error::EmptyModelReply {
            detail,
            finish_reason,
        }) => {
            assert_eq!(detail, EMPTY_REPLY);
            assert_eq!(finish_reason, None, "no finish_reason on the wire");
        }
        other => panic!("expected EmptyModelReply, got {other:?}"),
    }
}

#[test]
fn null_content_without_tools_is_error() {
    let body = one_choice(serde_json::json!({
        "role": "assistant",
        "content": null
    }));

    match normalize(&body) {
        Err(Error::EmptyModelReply { detail, .. }) => assert_eq!(detail, EMPTY_REPLY),
        other => panic!("expected EmptyModelReply, got {other:?}"),
    }
}

#[test]
fn empty_reply_error_stores_the_finish_reason() {
    let with_reason = empty_reply_error(false, Some("length".to_owned()));
    assert!(
        matches!(
            with_reason,
            Error::EmptyModelReply {
                finish_reason: Some(ref reason),
                ..
            } if reason == "length"
        ),
        "a supplied finish_reason must be stored: {with_reason:?}"
    );

    let without_reason = empty_reply_error(true, None);
    assert!(
        matches!(
            without_reason,
            Error::EmptyModelReply {
                finish_reason: None,
                ..
            }
        ),
        "a missing finish_reason stays missing: {without_reason:?}"
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

    assert!(matches!(
        normalize(&body),
        Err(Error::EmptyModelReply { .. })
    ));
}

#[test]
fn no_choices_is_malformed() {
    let body = serde_json::json!({ "choices": [] });
    assert!(matches!(normalize(&body), Err(Error::MalformedResponse(_))));
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
