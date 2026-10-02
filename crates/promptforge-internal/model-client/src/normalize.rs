//! Normalize OpenAI-shaped chat-completions JSON into a turn outcome.
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
//! Beside the strict turn parse, [`response_metadata`] leniently parses the
//! body's call metadata - the serving `model`, `usage` token accounting,
//! llama.cpp's `timings` extension, and vLLM's `metrics` extension - into the
//! canonical `promptforge-types` vocabulary. Metadata never fails a
//! completion: a malformed section degrades to `None` with a returned
//! diagnostic naming it.

use std::collections::HashSet;

use promptforge_types::metrics::{CallMetrics, LlamaTimings, Usage, VllmMetrics};
use serde::Deserialize;
use serde_json::Value;

use crate::Result;
use crate::client::{CompletionResult, ToolCall};
use crate::model::{CompletionError, CompletionErrorKind};

/// The specific added to an empty reply when reasoning was present but
/// ignored as answer text.
const REASONING_IGNORED: &str = "reasoning content was present but ignored";

/// A parsed assistant turn: outcome plus payload-free metadata.
#[derive(Debug, Clone, PartialEq)]
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

/// Extracts and shape-validates the first choice's per-turn context.
///
/// # Errors
/// Returns a `MalformedResponse`-kind failure when `choices` is missing or not a
/// non-empty array of objects, `finish_reason` is a present non-string,
/// `message` is missing or not an object, or a reasoning field has the wrong
/// type.
pub(crate) fn turn_context(body: &Value) -> Result<TurnContext<'_>> {
    let choices = match body.get("choices") {
        None => return Err(CompletionError::malformed("no choices in response")),
        Some(Value::Array(choices)) => choices,
        Some(_) => {
            return Err(CompletionError::malformed(
                "`choices` was present but not an array",
            ));
        }
    };
    let choice = choices
        .first()
        .ok_or_else(|| CompletionError::malformed("response had zero choices"))?;
    if !choice.is_object() {
        return Err(CompletionError::malformed("`choices[0]` was not an object"));
    }
    let finish_reason = match choice.get("finish_reason") {
        None | Some(Value::Null) => None,
        Some(Value::String(reason)) => Some(reason.clone()),
        Some(_) => {
            return Err(CompletionError::malformed(
                "`finish_reason` was present but not a string",
            ));
        }
    };
    let message = choice
        .get("message")
        .ok_or_else(|| CompletionError::malformed("choice had no message"))?;
    if !message.is_object() {
        return Err(CompletionError::malformed(
            "`message` was present but not an object",
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
/// reasoning side channel was present and holding the choice's
/// `finish_reason` so the tool loop can classify the empty turn.
pub(crate) fn empty_reply_error(
    reasoning_present: bool,
    finish_reason: Option<String>,
) -> CompletionError {
    let error = if reasoning_present {
        CompletionError::specific(CompletionErrorKind::EmptyReply, REASONING_IGNORED)
    } else {
        CompletionError::phrased(CompletionErrorKind::EmptyReply)
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
            return Err(CompletionError::malformed(
                "`tool_calls` was present but not an array",
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
            return Err(CompletionError::malformed(
                "`content` was present but not a string",
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

/// Parses the OpenAI `message.tool_calls` array into runtime [`ToolCall`]s.
///
/// Each call must be an object with a nonblank string `id`, an object
/// `function` containing a nonblank string `name`, and an `arguments`
/// field that is present, a JSON-encoded string, and decodes to a JSON
/// object. Blank identifiers, duplicate ids within the turn, missing or
/// null arguments, and arguments that do not decode to an object are all
/// rejected rather than coerced.
pub(crate) fn parse_openai_tool_calls(raw_calls: &[Value]) -> Result<Vec<ToolCall>> {
    let mut calls = Vec::with_capacity(raw_calls.len());
    let mut seen_ids = HashSet::new();
    for raw in raw_calls {
        if !raw.is_object() {
            return Err(CompletionError::malformed("tool call was not an object"));
        }
        // `type` must be present and name a function call: the OpenAI protocol
        // invariant requires `"type": "function"`, so a missing, null,
        // non-string, or other value is a malformed shape, not an absence.
        match raw.get("type") {
            Some(Value::String(kind)) if kind == "function" => {}
            _ => {
                return Err(CompletionError::malformed(
                    "tool call `type` must be the string \"function\"",
                ));
            }
        }
        let id = raw
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| CompletionError::malformed("tool call had no string id"))?;
        check_call_id(id)?;
        check_unique_call_id(&mut seen_ids, id)?;
        let function = raw
            .get("function")
            .ok_or_else(|| CompletionError::malformed("tool call had no function"))?;
        if !function.is_object() {
            return Err(CompletionError::malformed(
                "tool call `function` was not an object",
            ));
        }
        let name = function
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| CompletionError::malformed("tool call had no string name"))?;
        check_call_name(name)?;
        // OpenAI encodes `function.arguments` as a JSON string. It must be
        // present, a string, and decode to a JSON object - the shape tools
        // accept. Missing, null, non-string, invalid-JSON, and non-object
        // decoded values are all rejected rather than coerced.
        let arguments = match function.get("arguments") {
            Some(Value::String(raw_args)) => {
                let decoded = serde_json::from_str::<Value>(raw_args).map_err(|error| {
                    CompletionError::malformed(format!(
                        "tool call arguments were not valid JSON: {error}"
                    ))
                })?;
                check_call_arguments(&decoded)?;
                decoded
            }
            None | Some(Value::Null) => {
                return Err(CompletionError::malformed(
                    "tool call arguments were missing",
                ));
            }
            Some(_) => {
                return Err(CompletionError::malformed(
                    "tool call arguments were not a JSON-encoded string",
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

/// Refuses a blank tool-call id.
pub(crate) fn check_call_id(id: &str) -> Result<()> {
    if id.trim().is_empty() {
        return Err(CompletionError::malformed("tool call id was blank"));
    }
    Ok(())
}

/// Refuses a blank tool-call name.
pub(crate) fn check_call_name(name: &str) -> Result<()> {
    if name.trim().is_empty() {
        return Err(CompletionError::malformed("tool call name was blank"));
    }
    Ok(())
}

/// Refuses tool-call arguments that are not a JSON object, the shape tools
/// accept.
pub(crate) fn check_call_arguments(arguments: &Value) -> Result<()> {
    if !arguments.is_object() {
        return Err(CompletionError::malformed(
            "tool call arguments were not a JSON object",
        ));
    }
    Ok(())
}

/// Records `id` in `seen`, refusing an id another call in the turn has.
pub(crate) fn check_unique_call_id<'a>(seen: &mut HashSet<&'a str>, id: &'a str) -> Result<()> {
    if !seen.insert(id) {
        return Err(CompletionError::malformed(format!(
            "duplicate tool call id {id:?} within one turn"
        )));
    }
    Ok(())
}

/// First nonblank string among the known reasoning field synonyms.
///
/// A reasoning synonym that is present but neither a string nor JSON null is a
/// malformed shape; whitespace-only strings are treated as absent.
///
/// # Errors
/// Returns a `MalformedResponse`-kind failure when a present reasoning field is not a
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
                return Err(CompletionError::malformed(format!(
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
    /// What the backend reported: `usage`, llama.cpp's `timings`, and vLLM's
    /// `metrics`. The `client` section is always `None` here; only the
    /// transport's own clock fills it.
    pub(crate) metrics: CallMetrics,
    /// One line per section that was present but malformed and so
    /// degraded to `None`, and one for a body naming no string `model`:
    /// the Engine reports each as a `model_metadata_degraded` event, since
    /// this crate reaches no logger.
    pub(crate) diagnostics: Vec<String>,
}

/// Parses the serving model and every metrics family from a response body.
///
/// An absent or JSON-null section is `None` with no complaint; a present
/// section that does not parse degrades to `None` with a diagnostic naming
/// the section, so a backend with a broken metrics extension still
/// completes the call.
pub(crate) fn response_metadata(body: &Value) -> ResponseMetadata {
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
/// string and a diagnostic, never fails the call.
fn parse_model(body: &Value, diagnostics: &mut Vec<String>) -> String {
    if let Some(Value::String(model)) = body.get("model") {
        model.clone()
    } else {
        diagnostics
            .push("completion response named no string `model`; recorded as empty".to_owned());
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
    parse: impl FnOnce(&Value) -> std::result::Result<T, serde_json::Error>,
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
#[path = "normalize-tests.rs"]
mod tests;

#[cfg(test)]
#[path = "normalize-metadata-tests.rs"]
mod metadata_tests;
