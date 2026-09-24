//! The chat-completions request body: the one JSON shape every transport
//! sends for a round, built from the wire types and the frozen options.

use serde_json::Value;

use super::{Message, ToolSchema};
use crate::model::CompletionOptions;

/// Builds the completion request body.
///
/// Every request streams: `stream` is always true and
/// `stream_options.include_usage` asks the backend for the final
/// empty-choices usage chunk, so token accounting survives the SSE path.
/// When `tools` is `Some` and non-empty, each schema is wrapped into the
/// `OpenAI` function shape and sent as the request's `tools` array (with
/// `tool_choice` set to `auto`); passing `None` or an empty slice omits the
/// `tools` field, preserving the plain chat-completions behavior.
/// `options.model` names the model on the wire; optional `temperature`,
/// `max_tokens`, and `thinking` extend the request when present.
///
/// `#[doc(hidden)]`: a cross-crate seam for the transports that send a
/// round (the harness's model client and the engine's test client), not
/// host API.
#[doc(hidden)]
#[must_use]
pub fn build_request_body(
    messages: &[Message],
    tools: Option<&[ToolSchema]>,
    options: &CompletionOptions,
) -> Value {
    let mut body = serde_json::json!({
        "model": options.model,
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
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    },
                })
            })
            .collect();
        body["tools"] = Value::Array(wrapped);
        body["tool_choice"] = Value::String("auto".into());
    }
    if let Some(temperature) = options.temperature {
        body["temperature"] = serde_json::json!(temperature.get());
    }
    if let Some(max_tokens) = options.max_tokens {
        body["max_tokens"] = serde_json::json!(max_tokens.get());
    }
    if let Some(thinking) = options.thinking {
        body["chat_template_kwargs"] = serde_json::json!({
            "enable_thinking": thinking,
        });
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Temperature;

    #[test]
    fn the_body_always_streams_and_asks_for_usage() {
        let body = build_request_body(
            &[Message::user("hi")],
            None,
            &CompletionOptions::new("analyst"),
        );
        assert_eq!(body["model"], "analyst");
        assert_eq!(body["stream"], true);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("tools").is_none(), "no tools field without tools");
        assert!(body.get("temperature").is_none());
    }

    #[test]
    fn options_and_tools_reach_the_body() {
        let options = CompletionOptions {
            model: "analyst".into(),
            temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: Some(std::num::NonZeroU32::new(128).expect("128 is non-zero")),
            thinking: Some(false),
        };
        let schema = ToolSchema::new("echo", "Echo.", serde_json::json!({ "type": "object" }))
            .expect("a valid schema");
        let body = build_request_body(&[Message::user("hi")], Some(&[schema]), &options);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["function"]["name"], "echo");
    }

    #[test]
    fn an_empty_tool_list_sends_no_tools_field() {
        let body = build_request_body(
            &[Message::user("hi")],
            Some(&[]),
            &CompletionOptions::new("m"),
        );
        assert!(body.get("tools").is_none());
        assert!(body.get("tool_choice").is_none());
    }
}
