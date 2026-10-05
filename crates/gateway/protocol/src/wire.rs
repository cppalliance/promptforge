//! The OpenAI-shaped request and response bodies the gateway speaks.
//!
//! These are the gateway's own view of the wire contract. The executor defines
//! its own copies against the same JSON; the two are deliberately not shared,
//! because JSON is the contract and each side's struct is shaped by its role.
//! The message and choice payloads are kept as opaque JSON so everything the
//! gateway does not route passes through untouched.
//!
//! WIRE-005: the `object` discriminators are fixed `&'static str` literals
//! (`"list"`, `"model"`), so they are already closed.
//!
//! `gateway_warning` is a gateway-specific extension on the OpenAI response
//! shape: when an emulated tool dialect recovers from a malformed tool fence,
//! the affected choice's message reports the reason under `gateway_warning`
//! next to its emptied `content`. Downstream serde ignores the unknown field.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

mod embedding;
mod rerank;
mod speech;

pub use gateway_api_types::ModelInfo;

pub use self::embedding::{EmbeddingInput, EmbeddingRequest, EmbeddingResponse};
pub use self::rerank::{RerankRequest, RerankResponse};
pub use self::speech::{SpeechRequest, SpeechResponseFormat, SpeechStreamFormat, SpeechVoice};

/// An incoming chat completions request.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatRequest {
    /// The model name, resolved against the routing table.
    pub model: String,
    /// The conversation messages, passed through to the backend verbatim.
    pub messages: Vec<Value>,
    /// Whether the caller asked for a streaming (SSE) completion. Absent
    /// means non-streaming; an absent `stream` is never forwarded.
    #[serde(default, skip_serializing_if = "is_false")]
    pub stream: bool,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// serde `skip_serializing_if` predicate for the `stream` flag.
#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde skip_serializing_if predicates receive the field by reference"
)]
fn is_false(value: &bool) -> bool {
    !*value
}

/// Chat roles the gateway recognizes at the request boundary (OpenAI set).
const SUPPORTED_ROLES: [&str; 6] = [
    "system",
    "user",
    "assistant",
    "tool",
    "function",
    "developer",
];

impl ChatRequest {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 3] = ["model", "messages", "stream"];

    /// Validates the request shape at the trust boundary, without coercion.
    ///
    /// Rejects an empty model, an empty `messages` array, any message that
    /// fails the minimal shape check (an object with a supported string
    /// `role` and either `content` or a tool/function call), and any reserved
    /// key smuggled into the flattened `rest` map (WIRE-001/003). Everything
    /// else in each message object passes through verbatim.
    ///
    /// # Errors
    /// Returns a static reason string when the model is empty, `messages` is
    /// empty, a message fails the minimal shape check, or `rest` collides with a
    /// named field.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.model.trim().is_empty() {
            return Err("model must not be empty");
        }
        if self.messages.is_empty() {
            return Err("messages must not be empty");
        }
        for message in &self.messages {
            validate_message(message)?;
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, messages, stream)");
        }
        Ok(())
    }
}

/// Validates one chat message's minimal shape without reconstructing it (WIRE-001).
///
/// A message must be a JSON object with a supported string `role` and must
/// have either `content` (any shape: string, array, or null) or a
/// tool/function call. Unknown fields are left untouched for verbatim
/// passthrough.
fn validate_message(message: &Value) -> Result<(), &'static str> {
    let object = message
        .as_object()
        .ok_or("each message must be a JSON object")?;
    let role = object
        .get("role")
        .and_then(Value::as_str)
        .ok_or("each message must have a string role")?;
    if !SUPPORTED_ROLES.contains(&role) {
        return Err("each message role is not supported");
    }
    let has_content = object.contains_key("content");
    let has_call = object.contains_key("tool_calls") || object.contains_key("function_call");
    if !has_content && !has_call {
        return Err("each message must include content or a tool/function call");
    }
    Ok(())
}

/// An outgoing chat completions response.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatResponse {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The completion choices, passed through from the backend verbatim.
    pub choices: Vec<Value>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl ChatResponse {
    /// Reserved top-level keys that must never appear in the passthrough `rest`.
    const RESERVED: [&'static str; 2] = ["model", "choices"];

    /// Validates the upstream response shape, treating structural failure as an
    /// upstream-protocol error rather than silently passing it through.
    ///
    /// Each choice must pass the minimal shape check: an `index` plus one of
    /// the supported payloads (`message`, `delta`, or `text`). This rejects a
    /// backend that returns a success status with a structurally broken body
    /// (WIRE-002) while leaving every other field untouched for passthrough.
    ///
    /// # Errors
    /// Returns a static reason string when a choice fails the minimal shape
    /// check or a reserved key collides with the flattened `rest` map.
    pub fn validate(&self) -> Result<(), &'static str> {
        for choice in &self.choices {
            validate_choice(choice)?;
        }
        if Self::RESERVED
            .iter()
            .any(|key| self.rest.contains_key(*key))
        {
            return Err("rest must not contain a reserved key (model, choices)");
        }
        Ok(())
    }
}

/// Validates one response choice's minimal shape (WIRE-002).
///
/// A choice must be a JSON object with an `index` and one of the supported
/// payload fields (`message` for non-streaming, `delta` for streaming, or the
/// legacy `text`). Extra fields (for example `finish_reason`, `logprobs`) pass
/// through untouched.
fn validate_choice(choice: &Value) -> Result<(), &'static str> {
    let object = choice
        .as_object()
        .ok_or("upstream returned a non-object choice")?;
    if !object.contains_key("index") {
        return Err("upstream choice is missing index");
    }
    let has_payload = object.contains_key("message")
        || object.contains_key("delta")
        || object.contains_key("text");
    if !has_payload {
        return Err("upstream choice is missing message/delta/text");
    }
    Ok(())
}

/// One chunk of a streaming chat completion (OpenAI streaming shape).
///
/// A `stream: true` completion arrives as a sequence of these chunks, each
/// holding partial `delta` content instead of a complete `message`. The
/// terminal `[DONE]` sentinel is not JSON and never deserializes into this
/// type; the relay special-cases it before parsing.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatChunk {
    /// The model name, rewritten to the caller's requested name.
    pub model: String,
    /// The partial choices for this chunk.
    pub choices: Vec<ChatChunkChoice>,
    /// Every field the gateway does not name (for example `usage` on a
    /// final chunk), preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

impl ChatChunk {
    /// Validates one upstream chunk's minimal shape before it is relayed.
    ///
    /// A chunk must have at least one choice; each choice's `index` and
    /// `delta` are required typed fields, so deserialization has already
    /// proven them present. A chunk that fails this check (for example a
    /// usage-only summary object a backend appends mid-stream) is malformed:
    /// the parser logs and skips it rather than relaying it or ending the
    /// stream.
    ///
    /// # Errors
    /// Returns a static reason string when the chunk has no choices.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.choices.is_empty() {
            return Err("upstream chunk has no choices");
        }
        Ok(())
    }
}

/// One partial choice in a [`ChatChunk`]: an `index` plus a `delta` holding
/// the incremental payload (`role` on the first chunk, content or tool-call
/// fragments thereafter).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ChatChunkChoice {
    /// The completion choice this delta belongs to.
    pub index: u32,
    /// The incremental payload, kept as opaque JSON so every field the
    /// gateway does not route passes through untouched.
    pub delta: Value,
    /// Every field the gateway does not name (for example `finish_reason`
    /// on the terminal chunk), preserved verbatim.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The model list returned by `GET /v1/models` (the OpenAI shape).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
pub struct ModelsResponse {
    /// Always `"list"`.
    pub object: &'static str,
    /// One entry per configured `[[model]]`, in config order.
    pub data: Vec<ModelInfo>,
}

#[cfg(test)]
mod tests;
