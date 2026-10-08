//! Turns an MCP answer into tool output or a tool error.

use promptforge_plugin::{ToolError, ToolErrorKind, ToolOutput};
use rmcp::ServiceError;
use rmcp::model::{CallToolResult, ContentBlock, ResourceContents, ServerResult};

/// What a content block this change does not know becomes. rmcp's content
/// types are `#[non_exhaustive]`, so a new kind reaches this text.
const UNSUPPORTED: &str = "[unsupported content omitted]";

/// Says what went wrong from the error's kind alone. rmcp's own `Display`
/// text for a transport failure carries the request URL, and an entry's URL
/// may hold an `${env:NAME}` value, so that text never reaches a reason.
pub(crate) fn service_cause(error: &ServiceError) -> &'static str {
    match error {
        ServiceError::McpError(_) => "the server answered with an error",
        ServiceError::TransportSend(_) => "the request could not be sent to the server",
        ServiceError::TransportClosed => "the connection to the server closed",
        ServiceError::UnexpectedResponse => "the server answered with an unexpected response type",
        ServiceError::Cancelled { .. } => "the request was cancelled",
        ServiceError::Timeout { .. } => "the request timed out",
        _ => "the request failed",
    }
}

/// Maps a transport-level failure to a tool error, with the kind that says
/// whether retrying could help.
pub(crate) fn service_error(error: &ServiceError) -> ToolError {
    let kind = match error {
        ServiceError::McpError(_) | ServiceError::UnexpectedResponse => ToolErrorKind::Backend,
        _ => ToolErrorKind::Transport,
    };
    let text = match error {
        ServiceError::Timeout { timeout } => format!(
            "the MCP call did not finish within {} seconds",
            timeout.as_secs()
        ),
        ServiceError::McpError(e) => format!("the MCP server refused the call: {}", e.message),
        other => format!("the MCP call failed: {}", service_cause(other)),
    };
    ToolError::message(text).with_kind(kind)
}

/// Maps the answer to a `tools/call` request.
pub(crate) fn output(
    response: Result<ServerResult, ServiceError>,
) -> Result<ToolOutput, ToolError> {
    match response.map_err(|e| service_error(&e))? {
        ServerResult::CallToolResult(result) => render(&result),
        _ => Err(ToolError::message(
            "the MCP server answered a tool call with something other than a tool result",
        )
        .with_kind(ToolErrorKind::Backend)),
    }
}

/// Joins a result's text. An `is_error` result becomes a backend error that
/// carries the server's text.
pub(crate) fn render(result: &CallToolResult) -> Result<ToolOutput, ToolError> {
    let text = text_of(result);
    if result.is_error == Some(true) {
        let message = if text.is_empty() {
            "the MCP tool reported an error and gave no text".to_owned()
        } else {
            text
        };
        return Err(ToolError::message(message).with_kind(ToolErrorKind::Backend));
    }
    Ok(ToolOutput::untrusted(text))
}

/// The text blocks, embedded text resources, and placeholders, joined with
/// blank lines. A result with no blocks but with `structured_content`
/// gives that JSON.
fn text_of(result: &CallToolResult) -> String {
    if result.content.is_empty() {
        return result
            .structured_content
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_default();
    }
    result
        .content
        .iter()
        .map(block_text)
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn block_text(block: &ContentBlock) -> String {
    match block {
        ContentBlock::Text(text) => text.text.clone(),
        ContentBlock::Image(image) => format!("[image omitted: {}]", image.mime_type),
        ContentBlock::Audio(audio) => format!("[audio omitted: {}]", audio.mime_type),
        ContentBlock::ResourceLink(link) => format!("[resource: {}]", link.uri),
        ContentBlock::Resource(embedded) => match &embedded.resource {
            ResourceContents::TextResourceContents { text, .. } => text.clone(),
            ResourceContents::BlobResourceContents { mime_type, .. } => format!(
                "[binary resource omitted: {}]",
                mime_type.as_deref().unwrap_or("unknown type")
            ),
            _ => UNSUPPORTED.to_owned(),
        },
        _ => UNSUPPORTED.to_owned(),
    }
}

#[cfg(test)]
#[path = "result-tests.rs"]
mod tests;
