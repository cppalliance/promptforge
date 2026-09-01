//! Normalize OpenAI-shaped chat-completions JSON into a turn outcome.
//!
//! The wire canonicalization and the empty-response invariant live here so the
//! rest of the runtime can stay model-agnostic. A normalized turn must yield
//! either non-empty tool calls or non-empty text; anything else is
//! [`Error::EmptyModelReply`], carrying the choice's `finish_reason` so the
//! tool loop can classify the empty turn. The loop may still accept such a
//! turn as its clean exit - empty text with `finish_reason == "stop"` after
//! at least one successful tool dispatch - but normalization always raises
//! and lets the loop decide. Reasoning fields are a side channel only and
//! are never promoted into the answer.
//!
//! Beside the strict turn parse, [`response_metadata`] leniently parses the
//! body's call metadata - the serving `model`, `usage` token accounting,
//! llama.cpp's `timings` extension, and vLLM's `metrics` extension - into the
//! canonical `promptforge-core-support` vocabulary. Metadata never fails a
//! completion: a malformed section degrades to `None` with a warning.

use promptforge_core_support::events::{LlamaTimings, Usage, VllmMetrics};
use serde::Deserialize;
use serde_json::Value;

use crate::client::{CompletionResult, ToolCall};
use crate::{Error, Result};

/// Fixed detail when the turn had no product and no reasoning side channel.
const EMPTY_REPLY: &str = "empty model reply";
/// Fixed detail when reasoning was present but ignored as answer text.
const EMPTY_REPLY_REASONING_IGNORED: &str =
    "empty model reply: reasoning content was present but ignored";

/// A parsed assistant turn: outcome plus payload-free metadata.
///
/// `Eq` is intentionally omitted: [`CompletionResult`] carries tool-call
/// arguments as a [`serde_json::Value`], which is not `Eq` (it can hold an
/// `f64`), so only `Clone` and `PartialEq` are coherent here.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub(crate) struct NormalizedTurn {
    /// The text or tool-call product the tool loop consumes.
    pub(crate) outcome: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub(crate) finish_reason: Option<String>,
    /// Reasoning text from the wire, never used as the answer.
    pub(crate) reasoning_content: Option<String>,
}

/// The shared per-turn context extracted from a chat-completions body: the
/// first choice's `message`, `finish_reason`, and reasoning side channel.
pub(crate) struct TurnContext<'a> {
    /// The first choice's `message` object.
    pub(crate) message: &'a Value,
    /// The choice's `finish_reason`, when the backend supplied a string one.
    pub(crate) finish_reason: Option<String>,
    /// Reasoning side-channel text, never promoted into the answer.
    pub(crate) reasoning_content: Option<String>,
}

/// Extract and shape-validate the first choice's per-turn context.
///
/// # Errors
/// Returns [`Error::MalformedResponse`] when `choices` is missing or not a
/// non-empty array of objects, `finish_reason` is a present non-string,
/// `message` is missing or not an object, or a reasoning field has the wrong
/// type.
pub(crate) fn turn_context(body: &Value) -> Result<TurnContext<'_>> {
    let choices = match body.get("choices") {
        None => return Err(Error::MalformedResponse("no choices in response".into())),
        Some(Value::Array(choices)) => choices,
        Some(_) => {
            return Err(Error::MalformedResponse(
                "`choices` was present but not an array".into(),
            ));
        }
    };
    let choice = choices
        .first()
        .ok_or_else(|| Error::MalformedResponse("response had zero choices".into()))?;
    if !choice.is_object() {
        return Err(Error::MalformedResponse(
            "`choices[0]` was not an object".into(),
        ));
    }
    let finish_reason = match choice.get("finish_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) => Some(reason.clone()),
        Some(_) => {
            return Err(Error::MalformedResponse(
                "`finish_reason` was present but not a string".into(),
            ));
        }
    };
    let message = choice
        .get("message")
        .ok_or_else(|| Error::MalformedResponse("choice had no message".into()))?;
    if !message.is_object() {
        return Err(Error::MalformedResponse(
            "`message` was present but not an object".into(),
        ));
    }
    let reasoning_content = extract_reasoning(message)?;
    Ok(TurnContext {
        message,
        finish_reason,
        reasoning_content,
    })
}

/// The empty-reply error for a turn with no product, noting whether an ignored
/// reasoning side channel was present and carrying the choice's
/// `finish_reason` so the tool loop can classify the empty turn.
pub(crate) fn empty_reply_error(reasoning_present: bool, finish_reason: Option<String>) -> Error {
    Error::EmptyModelReply {
        detail: if reasoning_present {
            EMPTY_REPLY_REASONING_IGNORED
        } else {
            EMPTY_REPLY
        },
        finish_reason,
    }
}

/// Turns a chat-completions response body into a [`NormalizedTurn`].
///
/// # Errors
/// Returns [`Error::MalformedResponse`] when the body has no usable choice
/// shape, and [`Error::EmptyModelReply`] when the choice has neither
/// non-empty tool calls nor non-empty text.
pub(crate) fn normalize(body: &Value) -> Result<NormalizedTurn> {
    let TurnContext {
        message,
        finish_reason,
        reasoning_content,
    } = turn_context(body)?;

    // `tool_calls`, when present, must be an array; a present non-array is a
    // malformed shape, not an absence.
    let tool_calls = match message.get("tool_calls") {
        None | Some(Value::Null) => None,
        Some(Value::Array(calls)) => Some(calls),
        Some(_) => {
            return Err(Error::MalformedResponse(
                "`tool_calls` was present but not an array".into(),
            ));
        }
    };
    if let Some(raw_calls) = tool_calls.filter(|calls| !calls.is_empty()) {
        let calls = parse_openai_tool_calls(raw_calls)?;
        return Ok(NormalizedTurn {
            outcome: CompletionResult::ToolCalls(calls),
            finish_reason,
            reasoning_content,
        });
    }

    // `content`, when present, must be a string or JSON null; a present
    // value of any other type is a malformed shape.
    let content = match message.get("content") {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.as_str()),
        Some(_) => {
            return Err(Error::MalformedResponse(
                "`content` was present but not a string".into(),
            ));
        }
    };
    // Whitespace-only content is not a product; classify with `trim`, but
    // preserve the original nonblank payload verbatim.
    if let Some(text) = content.filter(|text| !text.trim().is_empty()) {
        return Ok(NormalizedTurn {
            outcome: CompletionResult::Text(text.to_string()),
            finish_reason,
            reasoning_content,
        });
    }

    Err(empty_reply_error(
        reasoning_content.is_some(),
        finish_reason,
    ))
}

/// Parse the OpenAI `message.tool_calls` array into runtime [`ToolCall`]s.
///
/// Each call must be an object with a nonblank string `id`, an object
/// `function` carrying a nonblank string `name`, and an `arguments` field that
/// is present, a JSON-encoded string, and decodes to a JSON object. Blank
/// identifiers, duplicate ids within the turn, missing or null arguments, and
/// arguments that do not decode to an object are all rejected rather than
/// coerced.
pub(crate) fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    let mut seen_ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for raw in raw_calls {
        if !raw.is_object() {
            return Err(Error::MalformedResponse(
                "tool call was not an object".into(),
            ));
        }
        // `type` must be present and name a function call (PF-NORM-003): the
        // OpenAI protocol invariant requires `"type": "function"`, so a missing,
        // null, non-string, or other value is a malformed shape, not an absence.
        match raw.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            _ => {
                return Err(Error::MalformedResponse(
                    "tool call `type` must be the string \"function\"".into(),
                ));
            }
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::MalformedResponse("tool call had no string id".into()))?;
        if id.trim().is_empty() {
            return Err(Error::MalformedResponse("tool call id was blank".into()));
        }
        if !seen_ids.insert(id) {
            return Err(Error::MalformedResponse(format!(
                "duplicate tool call id {id:?} within one turn"
            )));
        }
        let function = raw
            .get("function")
            .ok_or_else(|| Error::MalformedResponse("tool call had no function".into()))?;
        if !function.is_object() {
            return Err(Error::MalformedResponse(
                "tool call `function` was not an object".into(),
            ));
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::MalformedResponse("tool call had no string name".into()))?;
        if name.trim().is_empty() {
            return Err(Error::MalformedResponse("tool call name was blank".into()));
        }
        // OpenAI encodes `function.arguments` as a JSON string. It must be
        // present, a string, and decode to a JSON object - the shape tools
        // accept. Missing, null, non-string, invalid-JSON, and non-object
        // decoded values are all rejected rather than coerced.
        let arguments = match function.get("arguments") {
            Some(Value::String(raw_args)) => {
                let decoded = serde_json::from_str::<Value>(raw_args).map_err(|error| {
                    Error::MalformedResponse(format!(
                        "tool call arguments were not valid JSON: {error}"
                    ))
                })?;
                if !decoded.is_object() {
                    return Err(Error::MalformedResponse(
                        "tool call arguments did not decode to a JSON object".into(),
                    ));
                }
                decoded
            }
            None | Some(Value::Null) => {
                return Err(Error::MalformedResponse(
                    "tool call arguments were missing".into(),
                ));
            }
            Some(_) => {
                return Err(Error::MalformedResponse(
                    "tool call arguments were not a JSON-encoded string".into(),
                ));
            }
        };
        calls.push(ToolCall {
            id: id.to_string(),
            name: name.to_string(),
            arguments,
        });
    }
    Ok(calls)
}

/// First nonblank string among the known reasoning field synonyms.
///
/// A reasoning synonym that is present but neither a string nor JSON null is a
/// malformed shape; whitespace-only strings are treated as absent.
///
/// # Errors
/// Returns [`Error::MalformedResponse`] when a present reasoning field is not a
/// string or null.
pub(crate) fn extract_reasoning(message: &Value) -> Result<Option<String>> {
    for key in ["reasoning_content", "reasoning", "thinking"] {
        match message.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => {
                if !text.trim().is_empty() {
                    return Ok(Some(text.clone()));
                }
            }
            Some(_) => {
                return Err(Error::MalformedResponse(format!(
                    "`{key}` reasoning field was present but not a string"
                )));
            }
        }
    }
    Ok(None)
}

/// Call metadata parsed from a chat-completions body: the serving model and
/// every metrics family the backend reported.
///
/// Parsing is infallible by design. The turn outcome has its own strict
/// parser ([`normalize`]); metadata must never fail a completion whose turn
/// was usable, so each family degrades to `None` independently.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResponseMetadata {
    /// The model that served the call, or empty when the body named none.
    pub(crate) model: String,
    /// Token accounting, when the backend reported `usage`.
    pub(crate) usage: Option<Usage>,
    /// llama.cpp's `timings` extension, when that backend served the call.
    pub(crate) llama_timings: Option<LlamaTimings>,
    /// vLLM's `metrics` extension, when that backend served the call.
    pub(crate) vllm_metrics: Option<VllmMetrics>,
}

/// Parses the serving model and every metrics family from a response body.
///
/// An absent or JSON-null section is `None` with no complaint; a present
/// section that does not parse degrades to `None` with a `tracing` warning
/// naming the section, so a backend with a broken metrics extension still
/// completes the call.
pub(crate) fn response_metadata(body: &Value) -> ResponseMetadata {
    ResponseMetadata {
        model: parse_model(body),
        usage: parse_section(body, "usage", parse_usage),
        llama_timings: parse_section(body, "timings", parse_llama_timings),
        vllm_metrics: parse_section(body, "metrics", parse_vllm_metrics),
    }
}

/// The serving model from the body's top-level `model` field.
///
/// Every OpenAI-shaped backend names the model in its response, so a missing
/// or non-string value is anomalous: it warns and records an empty string,
/// never fails the call.
fn parse_model(body: &Value) -> String {
    if let Some(Value::String(model)) = body.get("model") {
        model.clone()
    } else {
        tracing::warn!("completion response named no string `model`; recorded as empty");
        String::new()
    }
}

/// Parses one top-level metadata section leniently.
///
/// Absent or JSON-null is `None` silently - a frontier body has no `timings`
/// and that is not a defect. A present section that fails `parse` degrades to
/// `None` with a warning naming the section and the parse failure.
fn parse_section<T>(
    body: &Value,
    key: &str,
    parse: impl FnOnce(&Value) -> std::result::Result<T, serde_json::Error>,
) -> Option<T> {
    match body.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => match parse(value) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                tracing::warn!("malformed `{key}` in completion response ignored: {error}");
                None
            }
        },
    }
}

/// The wire shape of the `usage` object: the flat core every backend sends,
/// plus the nested detail objects frontier backends and vLLM add.
#[derive(Deserialize)]
struct WireUsage {
    prompt_tokens: u32,
    completion_tokens: u32,
    total_tokens: u32,
    #[serde(default)]
    prompt_tokens_details: Option<WirePromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<WireCompletionTokensDetails>,
}

/// The nested `prompt_tokens_details` object carrying the cache detail.
#[derive(Deserialize)]
struct WirePromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

/// The nested `completion_tokens_details` object carrying the reasoning
/// detail.
#[derive(Deserialize)]
struct WireCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// Parses the `usage` object, flattening the nested detail fields into the
/// canonical [`Usage`] shape.
fn parse_usage(value: &Value) -> std::result::Result<Usage, serde_json::Error> {
    let wire = WireUsage::deserialize(value)?;
    Ok(Usage {
        prompt_tokens: wire.prompt_tokens,
        completion_tokens: wire.completion_tokens,
        total_tokens: wire.total_tokens,
        cached_tokens: wire
            .prompt_tokens_details
            .and_then(|details| details.cached_tokens),
        reasoning_tokens: wire
            .completion_tokens_details
            .and_then(|details| details.reasoning_tokens),
    })
}

/// The wire shape of llama.cpp's top-level `timings` extension.
///
/// The draft counters appear only when a speculative-decoding draft model
/// ran, so an absent counter means zero drafted tokens, not an unknown; the
/// per-token rates the server also sends are derivable and ignored.
#[derive(Deserialize)]
struct WireLlamaTimings {
    prompt_n: u32,
    prompt_ms: f64,
    prompt_per_second: f64,
    predicted_n: u32,
    predicted_ms: f64,
    predicted_per_second: f64,
    #[serde(default)]
    draft_n: u32,
    #[serde(default)]
    draft_n_accepted: u32,
}

/// Parses llama.cpp's `timings` object into the canonical [`LlamaTimings`].
fn parse_llama_timings(value: &Value) -> std::result::Result<LlamaTimings, serde_json::Error> {
    let wire = WireLlamaTimings::deserialize(value)?;
    Ok(LlamaTimings {
        prompt_n: wire.prompt_n,
        prompt_ms: wire.prompt_ms,
        prompt_per_second: wire.prompt_per_second,
        predicted_n: wire.predicted_n,
        predicted_ms: wire.predicted_ms,
        predicted_per_second: wire.predicted_per_second,
        draft_n: wire.draft_n,
        draft_n_accepted: wire.draft_n_accepted,
    })
}

/// Parses vLLM's `metrics` object into the canonical [`VllmMetrics`].
///
/// The canonical type is its own wire shape: every field is optional because
/// vLLM omits what it did not measure, and unknown keys are ignored.
fn parse_vllm_metrics(value: &Value) -> std::result::Result<VllmMetrics, serde_json::Error> {
    VllmMetrics::deserialize(value)
}

#[cfg(test)]
mod tests {
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
        // PF-NORM-003: `type` is required to be exactly "function"; a missing or
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

    /// Counts WARN-level tracing events while `f` runs, so the tests can pin
    /// both halves of the degrade policy: malformed sections warn, and
    /// well-formed or absent sections stay silent.
    fn with_warn_count<T>(f: impl FnOnce() -> T) -> (T, usize) {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct WarnCounter(Arc<AtomicUsize>);

        impl tracing::Subscriber for WarnCounter {
            fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
                true
            }
            fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
                tracing::span::Id::from_u64(1)
            }
            fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
            fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
            fn event(&self, event: &tracing::Event<'_>) {
                if *event.metadata().level() == tracing::Level::WARN {
                    self.0.fetch_add(1, Ordering::SeqCst);
                }
            }
            fn enter(&self, _: &tracing::span::Id) {}
            fn exit(&self, _: &tracing::span::Id) {}
        }

        let count = Arc::new(AtomicUsize::new(0));
        let result = tracing::subscriber::with_default(WarnCounter(Arc::clone(&count)), f);
        let warnings = count.load(Ordering::SeqCst);
        (result, warnings)
    }

    /// One assistant text choice, shared by the metadata fixture bodies.
    fn reply_choice() -> Value {
        serde_json::json!([{
            "index": 0,
            "message": { "role": "assistant", "content": "hi" },
            "finish_reason": "stop"
        }])
    }

    #[test]
    fn llama_body_parses_model_usage_and_timings() {
        let body = serde_json::json!({
            "id": "chatcmpl-llama",
            "object": "chat.completion",
            "created": 1_726_000_000_u64,
            "model": "qwen3-30b",
            "choices": reply_choice(),
            "usage": { "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 },
            "timings": {
                "prompt_n": 7,
                "prompt_ms": 12.5,
                "prompt_per_token_ms": 1.75,
                "prompt_per_second": 560.0,
                "predicted_n": 3,
                "predicted_ms": 30.5,
                "predicted_per_token_ms": 10.25,
                "predicted_per_second": 98.5,
                "draft_n": 4,
                "draft_n_accepted": 2
            }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 0, "a well-formed body must not warn");
        assert_eq!(metadata.model, "qwen3-30b");
        assert_eq!(
            metadata.usage,
            Some(Usage {
                prompt_tokens: 7,
                completion_tokens: 3,
                total_tokens: 10,
                cached_tokens: None,
                reasoning_tokens: None,
            }),
            "flat llama.cpp usage carries no detail fields"
        );
        assert_eq!(
            metadata.llama_timings,
            Some(LlamaTimings {
                prompt_n: 7,
                prompt_ms: 12.5,
                prompt_per_second: 560.0,
                predicted_n: 3,
                predicted_ms: 30.5,
                predicted_per_second: 98.5,
                draft_n: 4,
                draft_n_accepted: 2,
            })
        );
        assert_eq!(metadata.vllm_metrics, None);
    }

    #[test]
    fn llama_timings_without_draft_counters_default_to_zero() {
        // Without a configured draft model llama.cpp omits the draft
        // counters entirely; zero drafted tokens is the truthful reading,
        // so the common non-speculative body must not degrade to None.
        let body = serde_json::json!({
            "model": "qwen3-30b",
            "choices": reply_choice(),
            "timings": {
                "prompt_n": 7,
                "prompt_ms": 12.5,
                "prompt_per_second": 560.0,
                "predicted_n": 3,
                "predicted_ms": 30.5,
                "predicted_per_second": 98.5
            }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 0);
        let timings = metadata.llama_timings.unwrap();
        assert_eq!(timings.draft_n, 0);
        assert_eq!(timings.draft_n_accepted, 0);
    }

    #[test]
    fn vllm_body_parses_metrics_and_cached_tokens() {
        let body = serde_json::json!({
            "id": "chatcmpl-vllm",
            "object": "chat.completion",
            "model": "meta-llama/Llama-3.1-8B-Instruct",
            "choices": reply_choice(),
            "usage": {
                "prompt_tokens": 20,
                "completion_tokens": 5,
                "total_tokens": 25,
                "prompt_tokens_details": { "cached_tokens": 16 }
            },
            "metrics": {
                "time_to_first_token_ms": 8.5,
                "generation_time_ms": 22.5,
                "queue_time_ms": 1.5,
                "mean_itl_ms": 7.5,
                "tokens_per_second": 133.5
            }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 0, "a well-formed body must not warn");
        assert_eq!(metadata.model, "meta-llama/Llama-3.1-8B-Instruct");
        assert_eq!(
            metadata.usage,
            Some(Usage {
                prompt_tokens: 20,
                completion_tokens: 5,
                total_tokens: 25,
                cached_tokens: Some(16),
                reasoning_tokens: None,
            }),
            "the prompt_tokens_details cache detail must flatten into usage"
        );
        assert_eq!(metadata.llama_timings, None);
        assert_eq!(
            metadata.vllm_metrics,
            Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: Some(22.5),
                queue_time_ms: Some(1.5),
                mean_itl_ms: Some(7.5),
                tokens_per_second: Some(133.5),
            })
        );
    }

    #[test]
    fn vllm_metrics_omit_what_was_not_measured() {
        let body = serde_json::json!({
            "model": "m",
            "choices": reply_choice(),
            "metrics": { "time_to_first_token_ms": 8.5 }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 0);
        assert_eq!(
            metadata.vllm_metrics,
            Some(VllmMetrics {
                time_to_first_token_ms: Some(8.5),
                generation_time_ms: None,
                queue_time_ms: None,
                mean_itl_ms: None,
                tokens_per_second: None,
            }),
            "fields vLLM did not measure stay None inside a parsed section"
        );
    }

    #[test]
    fn frontier_body_parses_usage_detail_fields() {
        let body = serde_json::json!({
            "id": "chatcmpl-frontier",
            "object": "chat.completion",
            "model": "gpt-5.2",
            "choices": reply_choice(),
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 40,
                "total_tokens": 140,
                "prompt_tokens_details": { "cached_tokens": 64, "audio_tokens": 0 },
                "completion_tokens_details": {
                    "reasoning_tokens": 25,
                    "audio_tokens": 0,
                    "accepted_prediction_tokens": 0,
                    "rejected_prediction_tokens": 0
                }
            }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 0, "a well-formed body must not warn");
        assert_eq!(metadata.model, "gpt-5.2");
        assert_eq!(
            metadata.usage,
            Some(Usage {
                prompt_tokens: 100,
                completion_tokens: 40,
                total_tokens: 140,
                cached_tokens: Some(64),
                reasoning_tokens: Some(25),
            })
        );
        assert_eq!(
            metadata.llama_timings, None,
            "frontier bodies have no timings"
        );
        assert_eq!(
            metadata.vllm_metrics, None,
            "frontier bodies have no metrics"
        );
    }

    #[test]
    fn absent_metadata_sections_are_none_without_warning() {
        let bare = serde_json::json!({ "model": "m", "choices": reply_choice() });
        let with_nulls = serde_json::json!({
            "model": "m",
            "choices": reply_choice(),
            "usage": null,
            "timings": null,
            "metrics": null
        });

        for body in [bare, with_nulls] {
            let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
            assert_eq!(warnings, 0, "absence is normal, never a warning: {body}");
            assert_eq!(metadata.model, "m");
            assert_eq!(metadata.usage, None);
            assert_eq!(metadata.llama_timings, None);
            assert_eq!(metadata.vllm_metrics, None);
        }
    }

    #[test]
    fn malformed_metadata_degrades_to_none_with_a_warning() {
        // The deliberate degrade path: every section malformed at once, each
        // one warning and dropping to None, and the call still succeeds.
        let body = serde_json::json!({
            "model": 7,
            "choices": reply_choice(),
            "usage": { "prompt_tokens": "seven" },
            "timings": { "prompt_n": 7 },
            "metrics": ["not", "an", "object"]
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(metadata.model, "", "a non-string model records as empty");
        assert_eq!(metadata.usage, None, "non-numeric token counts degrade");
        assert_eq!(
            metadata.llama_timings, None,
            "timings missing required fields degrade"
        );
        assert_eq!(metadata.vllm_metrics, None, "a non-object metrics degrades");
        assert_eq!(warnings, 4, "each malformed section warns exactly once");
    }

    #[test]
    fn metadata_sections_degrade_independently() {
        let body = serde_json::json!({
            "model": "qwen3-30b",
            "choices": reply_choice(),
            "usage": "broken",
            "timings": {
                "prompt_n": 7,
                "prompt_ms": 12.5,
                "prompt_per_second": 560.0,
                "predicted_n": 3,
                "predicted_ms": 30.5,
                "predicted_per_second": 98.5
            }
        });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(warnings, 1, "only the broken section warns");
        assert_eq!(metadata.usage, None);
        assert!(
            metadata.llama_timings.is_some(),
            "a malformed sibling section must not take timings down with it"
        );
    }

    #[test]
    fn missing_model_records_empty_and_warns() {
        let body = serde_json::json!({ "choices": reply_choice() });

        let (metadata, warnings) = with_warn_count(|| response_metadata(&body));
        assert_eq!(metadata.model, "");
        assert_eq!(warnings, 1, "an OpenAI-shaped body without a model warns");
    }
}
