//! Operations on the wire types that only the engine performs.
//!
//! The `promptforge` facade never re-exports this module, so nothing here
//! is reachable from a host. Each function stands in for what would
//! otherwise be an inherent method or a public field on a host-visible
//! type: building a message from pre-validated parts, reading the raw JSON
//! a message holds, reading a tool schema's wire parts, reading the
//! metadata diagnostics and taking the raw JSON bodies a completion holds,
//! and wrapping a transport failure.

use promptforge_types::metrics::VllmMetrics;
use serde_json::Value;

use crate::client::{Completion, CompletionResult, Message, ToolCall, ToolSchema, ToolSchemaError};
use crate::error::Error;

/// Constructs a message from parts a caller has already validated.
///
/// `role` is one of the wire roles (`system`, `user`, `assistant`, `tool`).
/// `content` is the raw wire content value - a string for a plain message
/// or an `OpenAI` content-parts array for a multimodal one - and serializes
/// into the request verbatim. The agent executor's protocol layer validates
/// author-built message tables once and hands the validated parts here.
#[must_use]
pub fn message_from_validated_parts(
    role: impl Into<String>,
    content: Value,
    tool_call_id: Option<String>,
    tool_calls: Option<Vec<Value>>,
) -> Message {
    Message {
        role: role.into(),
        content,
        tool_call_id,
        tool_calls,
    }
}

/// Returns a message's raw content value (a string or a content-parts
/// array), for the executor's pre-dispatch size estimate, which needs the
/// parts that [`Message::content`] flattens away.
#[must_use]
pub fn message_content_value(message: &Message) -> &Value {
    &message.content
}

/// Returns the raw `tool_calls` array an assistant turn holds, for the
/// executor's pre-dispatch size estimate.
#[must_use]
pub fn message_raw_tool_calls(message: &Message) -> Option<&[Value]> {
    message.tool_calls.as_deref()
}

/// Builds a tool schema, validating the wire name and that the parameters
/// are a JSON object.
///
/// The raw [`serde_json::Value`] schema enters here only from the
/// executor's internal tool contract, so the raw JSON never appears in a
/// host-visible constructor.
///
/// # Errors
/// Returns [`ToolSchemaError::InvalidName`] when `name` is empty or contains
/// a character outside `[A-Za-z0-9_.-]`, and
/// [`ToolSchemaError::NonObjectSchema`] when `parameters` is not a JSON
/// object, so a tool can never be advertised to the model with an unusable
/// name or a non-object JSON Schema (F7).
pub fn tool_schema_new(
    name: impl Into<String>,
    description: impl Into<String>,
    parameters: Value,
) -> Result<ToolSchema, ToolSchemaError> {
    let name = name.into();
    if name.is_empty() {
        return Err(ToolSchemaError::InvalidName {
            name,
            reason: "must not be empty",
        });
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
    {
        return Err(ToolSchemaError::InvalidName {
            name,
            reason: "may contain only [A-Za-z0-9_.-]",
        });
    }
    if !parameters.is_object() {
        return Err(ToolSchemaError::NonObjectSchema { name });
    }
    Ok(ToolSchema {
        name,
        description: description.into(),
        parameters,
    })
}

/// Returns a tool schema's wire name, which keys the executor's dispatch
/// map.
#[must_use]
pub fn tool_schema_name(schema: &ToolSchema) -> &str {
    &schema.name
}

/// Returns a tool schema's model-facing description.
#[must_use]
pub fn tool_schema_description(schema: &ToolSchema) -> &str {
    &schema.description
}

/// Returns a tool call's parsed arguments as the raw wire JSON the
/// dispatcher hands a tool.
#[must_use]
pub fn tool_call_arguments(call: &ToolCall) -> &Value {
    &call.arguments
}

/// Takes the text or tool-call outcome out of a completion.
#[must_use]
pub fn completion_into_result(completion: Completion) -> CompletionResult {
    completion.result
}

/// Returns vLLM's per-request `metrics` for the call, when that backend
/// served it.
#[must_use]
pub fn completion_vllm_metrics(completion: &Completion) -> Option<&VllmMetrics> {
    completion.vllm_metrics.as_ref()
}

/// Returns one line per response metadata section that was present but
/// malformed and so degraded to `None` (or a body naming no string
/// `model`), for the engine to report as `model_metadata_degraded` events.
#[must_use]
pub fn completion_metadata_diagnostics(completion: &Completion) -> &[String] {
    &completion.metadata_diagnostics
}

/// Moves the JSON body sent to the gateway out of the completion, leaving
/// [`Value::Null`] in its place.
#[must_use]
pub fn completion_take_request_body(completion: &mut Completion) -> Value {
    std::mem::take(&mut completion.request_body)
}

/// Moves the buffered chat-completion body out of the completion, leaving
/// [`Value::Null`] in its place. The body is reassembled from the streamed
/// chunks in the shape a non-streaming backend would return.
#[must_use]
pub fn completion_take_response_body(completion: &mut Completion) -> Value {
    std::mem::take(&mut completion.response_body)
}

/// Wraps a transport-layer error, hiding its concrete type.
///
/// A transport that knows the failure was a timeout wraps it in
/// [`Timeout`](crate::Timeout) first, so
/// [`CompletionError::is_timeout`](crate::model::CompletionError::is_timeout)
/// can say so without this crate naming the HTTP client.
pub fn error_http(source: impl std::error::Error + Send + Sync + 'static) -> Error {
    Error::Http(Box::new(source))
}
