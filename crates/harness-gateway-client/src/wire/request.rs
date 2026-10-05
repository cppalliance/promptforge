//! The chat-completions request body: the one JSON shape every transport
//! sends for a round, built from the wire types and the frozen options.

use promptforge::model::{CompletionOptions, Message, ToolSchema};
use serde_json::Value;

/// Builds the JSON body of a streaming chat-completions request.
///
/// The body names the model given by `options.model()` and carries
/// `messages` as given. Every request streams, so `stream` is always true.
/// The body also sets `stream_options.include_usage`. That asks the backend
/// to send token usage in a final chunk with an empty `choices` list, so
/// token counts still arrive when the reply streams as server-sent events.
///
/// When `tools` is `Some` and holds at least one schema, the body sends each
/// schema in its `tools` array as an `OpenAI` function tool. Each entry is an
/// object with `type` set to `"function"` and a `function` object that holds
/// the tool's `name`, `description`, and `parameters`. The body then also
/// sets `tool_choice` to `"auto"`. When `tools` is `None` or an empty slice,
/// the body omits both fields and the request is a plain chat completion.
///
/// The optional settings in `options` extend the body only when set.
/// `temperature` and `max_tokens` become fields of the same name. `thinking`
/// becomes `chat_template_kwargs.enable_thinking`.
///
/// A transport that performs a `Chat` effect must build its request body
/// here. It passes the effect's messages, tools, and options, sends the
/// result as the JSON body of its chat-completions request, and later hands
/// the same value to
/// [`read_completion_stream`](crate::read_completion_stream). One shared
/// builder keeps every transport sending the same request for one effect.
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
