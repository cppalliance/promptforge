//! Parse OpenAI-shaped chat-completions JSON into a turn outcome.
//!
//! The wire canonicalization and the empty-response invariant live here so the
//! rest of the runtime can stay model-agnostic. A normalized turn must yield
//! either non-empty tool calls or non-empty text; anything else is
//! an `EmptyReply`-kind failure, holding the choice's `finish_reason` so the
//! tool loop can classify the empty turn. The loop may still accept such a
//! turn as its clean exit - empty text with `finish_reason == "stop"` after
//! at least one successful tool dispatch - but normalization always raises
//! and lets the loop decide. Reasoning fields are a side channel only and
//! are never promoted into the answer.
//!
//! Each parsed call is built with [`ToolCall::from_parts`], so the Engine's
//! neutral checks (blank id, blank name, non-object arguments) run on every
//! decoded call; the duplicate-id check runs when the read loop builds the
//! [`Completion`](promptforge::model::Completion) from the batch.
//!
//! Beside the strict turn parse, [`response_metadata`] leniently parses the
//! body's call metadata - the serving `model`, `usage` token accounting,
//! llama.cpp's `timings` extension, and vLLM's `metrics` extension - into the
//! canonical `promptforge::metrics` vocabulary. Metadata never fails a
//! completion: a malformed section degrades to `None` with a returned
//! diagnostic naming it.

use promptforge::metrics::{CallMetrics, LlamaTimings, Usage, VllmMetrics};
use promptforge::model::{CompletionError, CompletionErrorKind, CompletionResult, ToolCall};
use serde::Deserialize;
use serde_json::Value;

use crate::failure::malformed;

/// The specific added to an empty reply when reasoning was present but
/// ignored as answer text.
const REASONING_IGNORED: &str = "reasoning content was present but ignored";

/// A parsed assistant turn: outcome plus payload-free metadata.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct NormalizedTurn {
    /// The text or tool-call product the tool loop consumes.
    pub(super) outcome: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub(super) finish_reason: Option<String>,
    /// Reasoning text from the wire, never used as the answer.
    pub(super) reasoning_content: Option<String>,
}

/// The shared per-turn context extracted from a chat-completions body: the
/// first choice's `message`, `finish_reason`, and reasoning side channel.
struct TurnContext<'a> {
    /// The first choice's `message` object.
    message: &'a Value,
    /// The choice's `finish_reason`, when the backend supplied a string one.
    finish_reason: Option<String>,
    /// Reasoning side-channel text, never promoted into the answer.
    reasoning_content: Option<String>,
}

/// Extracts and shape-validates the first choice's per-turn context.
///
/// # Errors
/// Returns a `MalformedResponse`-kind failure when `choices` is missing or not a
/// non-empty array of objects, `finish_reason` is a present non-string,
/// `message` is missing or not an object, or a reasoning field has the wrong
/// type.
fn turn_context(body: &Value) -> Result<TurnContext<'_>, CompletionError> {
    let choices = match body.get("choices") {
        None => return Err(malformed("no choices in response")),
        Some(Value::Array(choices)) => choices,
        Some(_) => return Err(malformed("`choices` was present but not an array")),
    };
    let choice = choices
        .first()
        .ok_or_else(|| malformed("response had zero choices"))?;
    if !choice.is_object() {
        return Err(malformed("`choices[0]` was not an object"));
    }
    let finish_reason = match choice.get("finish_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) => Some(reason.clone()),
        Some(_) => return Err(malformed("`finish_reason` was present but not a string")),
    };
    let message = choice
        .get("message")
        .ok_or_else(|| malformed("choice had no message"))?;
    if !message.is_object() {
        return Err(malformed("`message` was present but not an object"));
    }
    let reasoning_content = extract_reasoning(message)?;
    Ok(TurnContext {
        message,
        finish_reason,
        reasoning_content,
    })
}

/// The empty-reply failure for a turn with no product: the kind's phrase,
/// extended with [`REASONING_IGNORED`] when an ignored reasoning side
/// channel was present, holding the choice's `finish_reason` so the tool
/// loop can classify the empty turn.
fn empty_reply(reasoning_present: bool, finish_reason: Option<String>) -> CompletionError {
    let kind = CompletionErrorKind::EmptyReply;
    let error = if reasoning_present {
        CompletionError::new(kind, format!("{}: {REASONING_IGNORED}", kind.phrase()))
    } else {
        CompletionError::new(kind, kind.phrase())
    };
    match finish_reason {
        Some(reason) => error.with_finish_reason(reason),
        None => error,
    }
}

/// Turns a chat-completions response body into a [`NormalizedTurn`].
///
/// # Errors
/// Returns a `MalformedResponse`-kind failure when the body has no usable
/// choice shape, and an `EmptyReply`-kind one when the choice has neither
/// non-empty tool calls nor non-empty text.
pub(super) fn normalize(body: &Value) -> Result<NormalizedTurn, CompletionError> {
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
        Some(_) => return Err(malformed("`tool_calls` was present but not an array")),
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
        Some(_) => return Err(malformed("`content` was present but not a string")),
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

    Err(empty_reply(reasoning_content.is_some(), finish_reason))
}

/// Parses the OpenAI `message.tool_calls` array into [`ToolCall`]s.
///
/// Each call must be an object with `"type": "function"`, a string `id`,
/// and an object `function` containing a string `name` and an `arguments`
/// field that is present, a JSON-encoded string, and valid JSON. Missing or
/// null arguments are rejected rather than coerced. [`ToolCall::from_parts`]
/// then refuses a blank id, a blank name, and arguments that do not decode
/// to an object.
fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ToolCall>, CompletionError> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    for raw in raw_calls {
        if !raw.is_object() {
            return Err(malformed("tool call was not an object"));
        }
        // `type` must be present and name a function call: the OpenAI protocol
        // invariant requires `"type": "function"`, so a missing, null,
        // non-string, or other value is a malformed shape, not an absence.
        match raw.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            _ => {
                return Err(malformed(
                    "tool call `type` must be the string \"function\"",
                ));
            }
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("tool call had no string id"))?;
        let function = raw
            .get("function")
            .ok_or_else(|| malformed("tool call had no function"))?;
        if !function.is_object() {
            return Err(malformed("tool call `function` was not an object"));
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("tool call had no string name"))?;
        // OpenAI encodes `function.arguments` as a JSON string. It must be
        // present, a string, and valid JSON; missing, null, non-string, and
        // invalid-JSON values are all rejected rather than coerced.
        let arguments = match function.get("arguments") {
            Some(Value::String(raw_args)) => {
                serde_json::from_str::<Value>(raw_args).map_err(|error| {
                    malformed(format!("tool call arguments were not valid JSON: {error}"))
                })?
            }
            None | Some(Value::Null) => return Err(malformed("tool call arguments were missing")),
            Some(_) => {
                return Err(malformed(
                    "tool call arguments were not a JSON-encoded string",
                ));
            }
        };
        calls.push(ToolCall::from_parts(id, name, arguments)?);
    }
    Ok(calls)
}

/// First nonblank string among the known reasoning field synonyms.
///
/// A reasoning synonym that is present but neither a string nor JSON null is a
/// malformed shape; whitespace-only strings are treated as absent.
///
/// # Errors
/// Returns a `MalformedResponse`-kind failure when a present reasoning field is not a
/// string or null.
fn extract_reasoning(message: &Value) -> Result<Option<String>, CompletionError> {
    for key in ["reasoning_content", "reasoning", "thinking"] {
        match message.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::String(text)) => {
                if !text.trim().is_empty() {
                    return Ok(Some(text.clone()));
                }
            }
            Some(_) => {
                return Err(malformed(format!(
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
pub(super) struct ResponseMetadata {
    /// The model that served the call, or empty when the body named none.
    pub(super) model: String,
    /// What the backend reported: `usage`, llama.cpp's `timings`, and vLLM's
    /// `metrics`. The `client` section is always `None` here; only the
    /// transport's own clock fills it.
    pub(super) metrics: CallMetrics,
    /// One line per section that was present but malformed and so
    /// degraded to `None`, and one for a body naming no string `model`:
    /// the Engine reports each as a `model_metadata_degraded` event, since
    /// this crate reaches no logger.
    pub(super) diagnostics: Vec<String>,
}

/// Parses the serving model and every metrics family from a response body.
///
/// An absent or JSON-null section is `None` with no complaint; a present
/// section that does not parse degrades to `None` with a diagnostic naming
/// the section, so a backend with a broken metrics extension still
/// completes the call.
pub(super) fn response_metadata(body: &Value) -> ResponseMetadata {
    let mut diagnostics = Vec::new();
    ResponseMetadata {
        model: parse_model(body, &mut diagnostics),
        metrics: CallMetrics {
            usage: parse_section(body, "usage", parse_usage, &mut diagnostics),
            llama: parse_section(body, "timings", parse_llama_timings, &mut diagnostics),
            vllm: parse_section(body, "metrics", parse_vllm_metrics, &mut diagnostics),
            client: None,
        },
        diagnostics,
    }
}

/// The serving model from the body's top-level `model` field.
///
/// Every backend that speaks this protocol names the model in its response,
/// so a missing or non-string value is anomalous: it records an empty
/// string and a diagnostic, never fails the call. The stream's finish
/// labels the completion with the model the request named either way, so
/// the diagnostic says the label came from the request.
fn parse_model(body: &Value, diagnostics: &mut Vec<String>) -> String {
    if let Some(Value::String(model)) = body.get("model") {
        model.clone()
    } else {
        diagnostics.push(
            "completion response named no string `model`; labeled with the requested model"
                .to_owned(),
        );
        String::new()
    }
}

/// Parses one top-level metadata section leniently.
///
/// Absent or JSON-null is `None` silently - a frontier body has no `timings`
/// and that is not a defect. A present section that fails `parse` degrades to
/// `None` with a diagnostic naming the section and the parse failure.
fn parse_section<T>(
    body: &Value,
    key: &str,
    parse: impl FnOnce(&Value) -> Result<T, serde_json::Error>,
    diagnostics: &mut Vec<String>,
) -> Option<T> {
    match body.get(key) {
        None | Some(Value::Null) => None,
        Some(value) => match parse(value) {
            Ok(parsed) => Some(parsed),
            Err(error) => {
                diagnostics.push(format!(
                    "malformed `{key}` in completion response ignored: {error}"
                ));
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

/// The nested `prompt_tokens_details` object holding the cache detail.
#[derive(Deserialize)]
struct WirePromptTokensDetails {
    #[serde(default)]
    cached_tokens: Option<u32>,
}

/// The nested `completion_tokens_details` object holding the reasoning
/// detail.
#[derive(Deserialize)]
struct WireCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: Option<u32>,
}

/// Parses the `usage` object, flattening the nested detail fields into the
/// canonical [`Usage`] shape.
fn parse_usage(value: &Value) -> Result<Usage, serde_json::Error> {
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
fn parse_llama_timings(value: &Value) -> Result<LlamaTimings, serde_json::Error> {
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
fn parse_vllm_metrics(value: &Value) -> Result<VllmMetrics, serde_json::Error> {
    VllmMetrics::deserialize(value)
}

#[cfg(test)]
#[path = "parse-tests.rs"]
mod tests;

#[cfg(test)]
#[path = "parse-metadata-tests.rs"]
mod metadata_tests;
