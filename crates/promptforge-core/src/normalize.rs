//! Normalize OpenAI-shaped chat-completions JSON into a turn outcome.
//!
//! Wire dialects and the empty-response invariant live here so the rest of the
//! runtime can stay model-agnostic. A normalized turn must yield either
//! non-empty tool calls or non-empty text; anything else is
//! [`Error::EmptyModelReply`]. Reasoning fields are a side channel only and
//! are never promoted into the answer.
//!
//! Some local chat templates (notably Gemma via llama.cpp) never emit a
//! `message.tool_calls` array. They put a sole ` ```tool_code ` fence or a sole
//! fenced OpenAI `tool_calls` JSON blob in `content` instead. When the entire
//! content is that dialect and nowhere else, this module promotes it to
//! [`CompletionResult::ToolCalls`]; mixed prose stays text so fabricated
//! dossiers are not half-parsed as tools.

use std::sync::Arc;

use serde_json::{Map, Value};

use crate::client::{CompletionResult, ToolCall, ToolHistoryStyle};
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
    /// How the tool loop should echo a tool-call turn into history.
    pub tool_history: ToolHistoryStyle,
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
            let calls = parse_openai_tool_calls(raw_calls)?;
            return Ok(NormalizedTurn {
                outcome: CompletionResult::ToolCalls(calls),
                tool_history: ToolHistoryStyle::OpenAi,
                finish_reason,
                reasoning_content,
            });
        }

        if let Some(content) = message
            .get("content")
            .and_then(Value::as_str)
            .filter(|content| !content.is_empty())
        {
            if let Some(calls) = parse_content_tool_dialect(content) {
                return Ok(NormalizedTurn {
                    outcome: CompletionResult::ToolCalls(calls),
                    tool_history: ToolHistoryStyle::ContentFence,
                    finish_reason,
                    reasoning_content,
                });
            }
            return Ok(NormalizedTurn {
                outcome: CompletionResult::Text(content.to_string()),
                tool_history: ToolHistoryStyle::OpenAi,
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

/// Parse the OpenAI `message.tool_calls` array into runtime [`ToolCall`]s.
fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ToolCall>> {
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
    Ok(calls)
}

/// When `content` is entirely a known tool-call dialect, return the calls.
///
/// Mixed prose (evidence text that happens to mention a fence) returns `None`
/// so the turn stays ordinary text.
fn parse_content_tool_dialect(content: &str) -> Option<Vec<ToolCall>> {
    let mut rest = content.trim();
    let mut calls = Vec::new();
    while !rest.is_empty() {
        if let Some((parsed, remain)) = peel_tool_code_fence(rest) {
            if parsed.is_empty() {
                return None;
            }
            calls.extend(parsed);
            rest = remain.trim();
            continue;
        }
        if let Some((parsed, remain)) = peel_json_tool_calls_fence(rest) {
            if parsed.is_empty() {
                return None;
            }
            calls.extend(parsed);
            rest = remain.trim();
            continue;
        }
        return None;
    }
    if calls.is_empty() {
        None
    } else {
        Some(calls)
    }
}

/// Peel one leading ` ```tool_code ` fence into Python-style `name(k=v)` calls.
fn peel_tool_code_fence(input: &str) -> Option<(Vec<ToolCall>, &str)> {
    let rest = strip_fence_open(input, "tool_code")?;
    let (body, after) = split_fence_close(rest)?;
    let mut calls = Vec::new();
    for (index, line) in body.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let call = parse_tool_code_call(line, index)?;
        calls.push(call);
    }
    Some((calls, after))
}

/// Peel one leading ` ```json ` / ` ``` ` fence that holds OpenAI `tool_calls`.
fn peel_json_tool_calls_fence(input: &str) -> Option<(Vec<ToolCall>, &str)> {
    let rest = strip_fence_open(input, "json")
        .or_else(|| strip_fence_open(input, ""))?;
    let (body, after) = split_fence_close(rest)?;
    let value: Value = serde_json::from_str(body.trim()).ok()?;
    let raw_calls = value
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|calls| !calls.is_empty())?;
    let calls = parse_openai_tool_calls(raw_calls).ok()?;
    Some((calls, after))
}

fn strip_fence_open<'a>(input: &'a str, language: &str) -> Option<&'a str> {
    let trimmed = input.trim_start();
    let prefix = if language.is_empty() {
        "```".to_string()
    } else {
        format!("```{language}")
    };
    let rest = trimmed.strip_prefix(&prefix)?;
    let rest = rest.strip_prefix('\r').unwrap_or(rest);
    let rest = rest.strip_prefix('\n')?;
    Some(rest)
}

fn split_fence_close(input: &str) -> Option<(&str, &str)> {
    let idx = input.find("```")?;
    Some((&input[..idx], &input[idx + 3..]))
}

/// Parse `search(query="x", count=3)` into a [`ToolCall`].
fn parse_tool_code_call(line: &str, index: usize) -> Option<ToolCall> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let name = line[..open].trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return None;
    }
    let args_src = line[open + 1..close].trim();
    let arguments = parse_tool_code_args(args_src)?;
    Some(ToolCall {
        id: format!("call_tool_code_{index}"),
        name: name.to_string(),
        arguments,
    })
}

fn parse_tool_code_args(src: &str) -> Option<Value> {
    if src.is_empty() {
        return Some(Value::Object(Map::new()));
    }
    let mut map = Map::new();
    for part in split_top_level_commas(src) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let eq = part.find('=')?;
        let key = part[..eq].trim();
        if key.is_empty() {
            return None;
        }
        let raw = part[eq + 1..].trim();
        map.insert(key.to_string(), parse_tool_code_value(raw)?);
    }
    Some(Value::Object(map))
}

fn parse_tool_code_value(raw: &str) -> Option<Value> {
    if let Some(inner) = strip_quotes(raw, '"').or_else(|| strip_quotes(raw, '\'')) {
        return Some(Value::String(inner.to_string()));
    }
    if raw.eq_ignore_ascii_case("true") {
        return Some(Value::Bool(true));
    }
    if raw.eq_ignore_ascii_case("false") {
        return Some(Value::Bool(false));
    }
    if let Ok(n) = raw.parse::<i64>() {
        return Some(Value::Number(n.into()));
    }
    if let Ok(n) = raw.parse::<f64>() {
        return serde_json::Number::from_f64(n).map(Value::Number);
    }
    None
}

fn strip_quotes(raw: &str, quote: char) -> Option<&str> {
    let raw = raw.strip_prefix(quote)?.strip_suffix(quote)?;
    Some(raw)
}

fn split_top_level_commas(src: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth: i32 = 0;
    let mut in_quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in src.char_indices() {
        if let Some(q) = in_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == q {
                in_quote = None;
            }
            continue;
        }
        match ch {
            '"' | '\'' => in_quote = Some(ch),
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&src[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&src[start..]);
    parts
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

    #[test]
    fn gemma_tool_code_fence_becomes_tool_calls() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```tool_code\nsearch(query=\"C++ Alliance founder\")\n```"
                },
                "finish_reason": "stop"
            }]
        });

        let turn = normalize(&body).unwrap();
        assert_eq!(turn.tool_history, ToolHistoryStyle::ContentFence);
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_tool_code_0");
                assert_eq!(calls[0].name, "search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "C++ Alliance founder" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn fenced_openai_tool_calls_json_becomes_tool_calls() {
        let body = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": "```json\n{\"tool_calls\":[{\"id\":\"1\",\"type\":\"function\",\"function\":{\"name\":\"fetch\",\"arguments\":\"{\\\"url\\\":\\\"https://example.com\\\"}\"}}]}\n```"
                }
            }]
        });

        let turn = normalize(&body).unwrap();
        match turn.outcome {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].name, "fetch");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "url": "https://example.com" })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn tool_code_mixed_with_prose_stays_text() {
        let content = "Here is evidence.\n\n```tool_code\nsearch(query=\"x\")\n```\n";
        let body = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": content }
            }]
        });

        let turn = normalize(&body).unwrap();
        match turn.outcome {
            CompletionResult::Text(text) => assert_eq!(text, content),
            CompletionResult::ToolCalls(_) => panic!("mixed prose must stay text"),
        }
    }
}
