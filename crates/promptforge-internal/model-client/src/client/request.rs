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
/// A transport performing a `Chat` effect passes the effect's messages,
/// tools, and options here, sends the result as the JSON body of its
/// chat-completions request, and later hands the same value to
/// [`read_completion_stream`](super::read_completion_stream). Building the
/// body anywhere else would let two transports send different requests
/// for one effect.
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
    use serde_json::json;

    use super::*;
    use crate::detail::{message_from_validated_parts, tool_schema_new};
    use crate::model::Temperature;

    /// Checks every `messages[].tool_calls[]` entry against the OpenAI
    /// function-call schema a strict endpoint enforces, returning the
    /// offending path for the first violation.
    fn check_openai_tool_calls(body: &Value) -> Result<(), String> {
        let messages = body
            .get("messages")
            .and_then(Value::as_array)
            .ok_or_else(|| "request body had no messages array".to_owned())?;
        for (message_index, message) in messages.iter().enumerate() {
            let Some(calls) = message.get("tool_calls").filter(|calls| !calls.is_null()) else {
                continue;
            };
            let calls = calls.as_array().ok_or_else(|| {
                format!("messages[{message_index}].tool_calls was present but not an array")
            })?;
            for (call_index, call) in calls.iter().enumerate() {
                let path = format!("messages[{message_index}].tool_calls[{call_index}]");
                if !call.is_object() {
                    return Err(format!("{path} was not an object"));
                }
                if call.get("type") != Some(&Value::String("function".to_owned())) {
                    return Err(format!("{path}.type must be the string \"function\""));
                }
                let id = call
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{path} had no string id"))?;
                if id.trim().is_empty() {
                    return Err(format!("{path}.id was blank"));
                }
                let function = call
                    .get("function")
                    .filter(|function| function.is_object())
                    .ok_or_else(|| format!("{path}.function was not an object"))?;
                let name = function
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| format!("{path}.function had no string name"))?;
                if name.trim().is_empty() {
                    return Err(format!("{path}.function.name was blank"));
                }
                let raw = function
                    .get("arguments")
                    .and_then(Value::as_str)
                    .ok_or_else(|| {
                        format!("{path}.function.arguments was not a JSON-encoded string")
                    })?;
                let decoded = serde_json::from_str::<Value>(raw).map_err(|error| {
                    format!("{path}.function.arguments was not valid JSON: {error}")
                })?;
                if !decoded.is_object() {
                    return Err(format!(
                        "{path}.function.arguments did not decode to a JSON object"
                    ));
                }
            }
        }
        Ok(())
    }

    /// A user turn, the assistant turn that called `echo` with `call`, and
    /// the tool result answering it.
    fn replayed_exchange(call: Value) -> Vec<Message> {
        vec![
            Message::user("echo once"),
            message_from_validated_parts("assistant", Value::Null, None, Some(vec![call])),
            Message::tool("call_1", "echoed: one"),
        ]
    }

    /// A well-formed OpenAI function call to `echo`.
    fn echo_call() -> Value {
        json!({
            "id": "call_1",
            "type": "function",
            "function": { "name": "echo", "arguments": "{\"value\":\"one\"}" },
        })
    }

    #[test]
    fn a_replayed_tool_call_turn_serializes_in_the_openai_function_shape() {
        let body = build_request_body(
            &replayed_exchange(echo_call()),
            None,
            &CompletionOptions::new("m"),
        );
        assert_eq!(check_openai_tool_calls(&body), Ok(()), "{body}");
        let call = &body["messages"][1]["tool_calls"][0];
        assert_eq!(call["id"], "call_1");
        assert_eq!(call["type"], "function");
        assert_eq!(call["function"]["name"], "echo");
        assert_eq!(call["function"]["arguments"], "{\"value\":\"one\"}");
        assert_eq!(body["messages"][2]["role"], "tool");
        assert_eq!(body["messages"][2]["tool_call_id"], "call_1");
        assert_eq!(body["messages"][2]["content"], "echoed: one");
    }

    #[test]
    fn the_openai_check_names_each_way_a_replayed_call_breaks_the_schema() {
        let cases = [
            (
                json!({ "id": "call_1", "function": { "name": "echo", "arguments": "{}" } }),
                ".type must be the string \"function\"",
            ),
            (
                json!({ "id": " ", "type": "function", "function": { "name": "echo", "arguments": "{}" } }),
                ".id was blank",
            ),
            (
                json!({ "type": "function", "function": { "name": "echo", "arguments": "{}" } }),
                " had no string id",
            ),
            (
                json!({ "id": "call_1", "type": "function", "name": "echo", "arguments": "{}" }),
                ".function was not an object",
            ),
            (
                json!({ "id": "call_1", "type": "function", "function": { "name": "", "arguments": "{}" } }),
                ".function.name was blank",
            ),
            (
                json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": { "value": "one" } } }),
                ".function.arguments was not a JSON-encoded string",
            ),
            (
                json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": "{oops" } }),
                ".function.arguments was not valid JSON",
            ),
            (
                json!({ "id": "call_1", "type": "function", "function": { "name": "echo", "arguments": "[1]" } }),
                ".function.arguments did not decode to a JSON object",
            ),
        ];
        for (call, expected) in cases {
            let body =
                build_request_body(&replayed_exchange(call), None, &CompletionOptions::new("m"));
            let violation = check_openai_tool_calls(&body).expect_err("the call breaks the schema");
            assert!(
                violation.starts_with("messages[1].tool_calls[0]") && violation.contains(expected),
                "expected {expected:?}, got {violation:?}"
            );
        }
    }

    #[test]
    fn messages_go_out_verbatim_in_order() {
        let messages = [
            message_from_validated_parts("system", json!("be brief"), None, None),
            Message::user("draft text"),
            Message::assistant("done"),
        ];
        let body = build_request_body(&messages, None, &CompletionOptions::new("m"));
        assert_eq!(
            body["messages"],
            json!([
                { "role": "system", "content": "be brief" },
                { "role": "user", "content": "draft text" },
                { "role": "assistant", "content": "done" },
            ])
        );
    }

    #[test]
    fn each_tool_is_wrapped_as_a_function_with_its_schema_in_order() {
        let parameters = json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"],
        });
        let tools = [
            tool_schema_new("echo", "Echo a value.", parameters.clone()).expect("a valid schema"),
            tool_schema_new("grab", "Grab a value", json!({ "type": "object" }))
                .expect("a valid schema"),
        ];
        let body = build_request_body(
            &[Message::user("hi")],
            Some(&tools),
            &CompletionOptions::new("m"),
        );
        assert_eq!(
            body["tools"],
            json!([
                {
                    "type": "function",
                    "function": {
                        "name": "echo",
                        "description": "Echo a value.",
                        "parameters": parameters,
                    },
                },
                {
                    "type": "function",
                    "function": {
                        "name": "grab",
                        "description": "Grab a value",
                        "parameters": { "type": "object" },
                    },
                },
            ])
        );
    }

    #[test]
    fn unset_options_send_no_option_fields() {
        let body = build_request_body(
            &[Message::user("hi")],
            None,
            &CompletionOptions::new("analyst"),
        );
        assert_eq!(body["model"], "analyst");
        for field in ["temperature", "max_tokens", "chat_template_kwargs"] {
            assert!(body.get(field).is_none(), "{field} is absent: {body}");
        }
    }

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
        let schema = tool_schema_new("echo", "Echo.", serde_json::json!({ "type": "object" }))
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
