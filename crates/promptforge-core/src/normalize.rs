//! Normalize OpenAI-shaped chat-completions JSON into a turn outcome.
//!
//! Wire dialects and the empty-response invariant live here so the rest of the
//! runtime can stay model-agnostic. A normalized turn must yield either
//! non-empty tool calls or non-empty text; anything else is
//! [`Error::EmptyModelReply`]. Reasoning fields are a side channel only and
//! are never promoted into the answer.

use std::sync::Arc;

use serde_json::Value;

use crate::client::{CompletionResult, ToolCall};
use crate::{Error, Result};

/// Fixed detail when the turn had no product and no reasoning side channel.
const EMPTY_REPLY: &str = "empty model reply";
/// Fixed detail when reasoning was present but ignored as answer text.
const EMPTY_REPLY_REASONING_IGNORED: &str =
    "empty model reply: reasoning content was present but ignored";

/// A parsed assistant turn: outcome plus payload-free metadata.
#[derive(Debug)]
#[non_exhaustive]
pub struct NormalizedTurn {
    /// The text or tool-call product the tool loop consumes.
    pub outcome: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub finish_reason: Option<String>,
    /// Reasoning text from the wire, never used as the answer.
    pub reasoning_content: Option<String>,
}

/// Turns a chat-completions response body into a [`NormalizedTurn`].
///
/// Implementors concentrate vendor wire quirks. The default is
/// [`OpenAiChatNormalizer`].
pub trait CompletionNormalizer: Send + Sync {
    /// Parse `body` into a turn that satisfies the empty-response invariant.
    ///
    /// # Errors
    /// Returns [`Error::MalformedResponse`] when the body has no usable choice
    /// shape, and [`Error::EmptyModelReply`] when the choice has neither
    /// non-empty tool calls nor non-empty text.
    fn normalize(&self, body: &Value) -> Result<NormalizedTurn>;
}

/// Default normalizer for OpenAI-compatible `/chat/completions` bodies.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiChatNormalizer;

impl OpenAiChatNormalizer {
    /// Construct the default OpenAI chat normalizer.
    #[must_use]
    pub fn new() -> OpenAiChatNormalizer {
        OpenAiChatNormalizer
    }

    /// An [`Arc`] of the default normalizer for storing on a client.
    #[must_use]
    pub fn shared() -> Arc<dyn CompletionNormalizer> {
        Arc::new(OpenAiChatNormalizer)
    }
}

impl CompletionNormalizer for OpenAiChatNormalizer {
    fn normalize(&self, body: &Value) -> Result<NormalizedTurn> {
        let choice = body
            .get("choices")
            .and_then(Value::as_array)
            .and_then(|choices| choices.first())
            .ok_or_else(|| Error::MalformedResponse("no choices in response".into()))?;
        let finish_reason = choice
            .get("finish_reason")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let message = choice
            .get("message")
            .ok_or_else(|| Error::MalformedResponse("choice had no message".into()))?;
        let reasoning_content = extract_reasoning(message);

        if let Some(raw_calls) = message
            .get("tool_calls")
            .and_then(Value::as_array)
            .filter(|calls| !calls.is_empty())
        {
            let mut calls = Vec::with_capacity(raw_calls.len());
            for raw in raw_calls {
                let id = raw
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::MalformedResponse("tool call had no id".into()))?
                    .to_string();
                let function = raw
                    .get("function")
                    .ok_or_else(|| Error::MalformedResponse("tool call had no function".into()))?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::MalformedResponse("tool call had no name".into()))?
                    .to_string();
                let arguments = match function.get("arguments").and_then(Value::as_str) {
                    Some(raw_args) => serde_json::from_str::<Value>(raw_args)
                        .unwrap_or_else(|_| Value::String(raw_args.to_string())),
                    None => Value::Null,
                };
                calls.push(ToolCall {
                    id,
                    name,
                    arguments,
                });
            }
            return Ok(NormalizedTurn {
                outcome: CompletionResult::ToolCalls(calls),
                finish_reason,
                reasoning_content,
            });
        }

        if let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.is_empty())
        {
            return Ok(NormalizedTurn {
                outcome: CompletionResult::Text(content.to_string()),
                finish_reason,
                reasoning_content,
            });
        }

        Err(Error::EmptyModelReply {
            detail: if reasoning_content.is_some() {
                EMPTY_REPLY_REASONING_IGNORED
            } else {
                EMPTY_REPLY
            },
        })
    }
}

/// First non-empty string among the known reasoning field synonyms.
fn extract_reasoning(message: &Value) -> Option<String> {
    for key in ["reasoning_content", "reasoning", "thinking"] {
        if let Some(text) = message
            .get(key)
            .and_then(Value::as_str)
            .filter(|text| !text.is_empty())
        {
            return Some(text.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn normalize(body: &Value) -> Result<NormalizedTurn> {
        OpenAiChatNormalizer.normalize(body)
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
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "web_fetch", "arguments": "not json" }
                    }]
                }
            }]
        });

        let turn = normalize(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].arguments, Value::String("not json".into()));
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
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
            Err(Error::EmptyModelReply { detail }) => {
                assert_eq!(detail, EMPTY_REPLY_REASONING_IGNORED);
            }
            other => panic!("expected EmptyModelReply, got {other:?}"),
        }
    }

    #[test]
    fn empty_string_content_without_tools_is_error() {
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "" }
            }]
        });

        match normalize(&body) {
            Err(Error::EmptyModelReply { detail }) => assert_eq!(detail, EMPTY_REPLY),
            other => panic!("expected EmptyModelReply, got {other:?}"),
        }
    }

    #[test]
    fn null_content_without_tools_is_error() {
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": null }
            }]
        });

        match normalize(&body) {
            Err(Error::EmptyModelReply { detail }) => assert_eq!(detail, EMPTY_REPLY),
            other => panic!("expected EmptyModelReply, got {other:?}"),
        }
    }

    #[test]
    fn synonym_reasoning_field_is_side_channel() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "answer",
                    "reasoning": "via synonym"
                }
            }]
        });

        let turn = normalize(&body).unwrap();
        assert_eq!(turn.reasoning_content.as_deref(), Some("via synonym"));
        match turn.outcome {
            CompletionResult::Text(text) => assert_eq!(text, "answer"),
            CompletionResult::ToolCalls(_) => panic!("expected text, got tool calls"),
        }
    }

    #[test]
    fn empty_reasoning_synonym_falls_through() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "answer",
                    "reasoning_content": "",
                    "thinking": "from thinking"
                }
            }]
        });

        let turn = normalize(&body).unwrap();
        assert_eq!(turn.reasoning_content.as_deref(), Some("from thinking"));
    }

    #[test]
    fn missing_content_and_tools_is_empty_model_reply() {
        let body = serde_json::json!({
            "choices": [{ "message": { "role": "assistant" } }]
        });

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
}
