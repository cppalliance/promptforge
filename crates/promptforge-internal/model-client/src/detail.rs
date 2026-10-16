//! Operations on the wire types that only the Engine performs.
//!
//! The `promptforge` facade never re-exports this module, so only Engine
//! crates reach it. Each function stands in for what would
//! otherwise be an inherent method or a public field on a facade-visible
//! type: building a message from pre-validated parts, reading the raw JSON
//! a message holds, building a tool schema, and reading a tool call's raw
//! arguments.

use serde_json::Value;

use crate::client::{
    Completion, CompletionResult, Message, Role, ToolCall, ToolSchema, ToolSchemaError,
};

/// Constructs a message from parts a caller has already validated.
///
/// `content` is the raw wire content value - a string for a plain message
/// or an `OpenAI` content-parts array for a multimodal one - and serializes
/// into the request verbatim. For each chat round the Engine sends, its Lua
/// layer has already validated each author-built record once, as the
/// message list added it, and hands the validated parts here.
#[must_use]
pub fn message_from_validated_parts(
    role: Role,
    content: Value,
    tool_call_id: Option<String>,
    tool_calls: Option<Vec<Value>>,
) -> Message {
    Message {
        role,
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
/// facade-visible constructor.
///
/// # Errors
/// Returns [`ToolSchemaError::InvalidName`] when `name` is empty or contains
/// a character outside `[A-Za-z0-9_.-]`, and
/// [`ToolSchemaError::NonObjectSchema`] when `parameters` is not a JSON
/// object, so a tool can never be advertised to the model with an unusable
/// name or a non-object JSON Schema.
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
