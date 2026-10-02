//! The chat-completions request body: the one JSON shape every transport
//! sends for a round, built from the wire types and the frozen options.

use promptforge::model::{CompletionOptions, Message, ToolSchema};
use serde_json::Value;

/// Builds the completion request body.
///
/// Every request streams: `stream` is always true and
/// `stream_options.include_usage` asks the backend for the final
/// empty-choices usage chunk, so token accounting survives the SSE path.
/// When `tools` is `Some` and non-empty, each schema is wrapped into the
/// `OpenAI` function shape and sent as the request's `tools` array (with
/// `tool_choice` set to `auto`); passing `None` or an empty slice omits the
/// `tools` field, preserving the plain chat-completions behavior.
/// `options.model()` names the model on the wire; optional `temperature`,
/// `max_tokens`, and `thinking` extend the request when present.
///
/// A transport performing a `Chat` effect passes the effect's messages,
/// tools, and options here, sends the result as the JSON body of its
/// chat-completions request, and later hands the same value to
/// [`read_completion_stream`](crate::read_completion_stream). Building the
/// body anywhere else would let two transports send different requests
/// for one effect.
#[must_use]
pub fn build_request_body(
    messages: &[Message],
    tools: Option<&[ToolSchema]>,
    options: &CompletionOptions,
) -> Value {
    let mut body = serde_json::json!({
        "model": options.model(),
        "messages": messages,
        "stream": true,
        "stream_options": { "include_usage": true },
    });
    if let Some(tools) = tools.filter(|tools| !tools.is_empty()) {
        let wrapped: Vec<Value> = tools
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters(),
                    },
                })
            })
            .collect();
        body["tools"] = Value::Array(wrapped);
        body["tool_choice"] = Value::String("auto".into());
    }
    if let Some(temperature) = options.temperature() {
        body["temperature"] = serde_json::json!(temperature.get());
    }
    if let Some(max_tokens) = options.max_tokens() {
        body["max_tokens"] = serde_json::json!(max_tokens.get());
    }
    if let Some(thinking) = options.thinking() {
        body["chat_template_kwargs"] = serde_json::json!({
            "enable_thinking": thinking,
        });
    }
    body
}

#[cfg(test)]
#[path = "request-tests.rs"]
mod tests;
