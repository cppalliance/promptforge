//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! Tranche 1 speaks the plain `/chat/completions` shape: a list of messages in,
//! one text reply out. No tools, no streaming. The client holds only the
//! gateway's URL and the shared token; the vendor credential lives in the
//! gateway, so the executor never sees it. Point `PROMPTFORGE_BASE_URL` at a
//! local server or another gateway to retarget it.

use serde_json::Value;

use crate::{Error, Result};

/// Default backend base URL (the local development gateway).
const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8081/v1";
/// Default model used when `PROMPTFORGE_MODEL` is unset.
const DEFAULT_MODEL: &str = "claude-sonnet-4-6";

/// A single chat message.
///
/// A plain `user` message serializes to just `{"role":..,"content":..}`; the
/// optional `tool_call_id` and `tool_calls` fields are emitted only when set,
/// which keeps the wire shape of ordinary messages unchanged.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Message {
    /// The role: `system`, `user`, `assistant`, or `tool`.
    pub role: String,
    /// The message text.
    pub content: String,
    /// For a `tool` message, the id of the tool call this result answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// For an `assistant` turn that requested tools, the raw `tool_calls` array
    /// as received from the backend, echoed back verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<Value>>,
}

impl Message {
    /// Construct a `user` message.
    #[must_use]
    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: "user".into(),
            content: content.into(),
            tool_call_id: None,
            tool_calls: None,
        }
    }

    /// Construct a `tool` message carrying the result of a tool call.
    ///
    /// `tool_call_id` must match the `id` of the [`ToolCall`] this answers.
    #[must_use]
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Message {
        Message {
            role: "tool".into(),
            content: content.into(),
            tool_call_id: Some(tool_call_id.into()),
            tool_calls: None,
        }
    }

    /// Construct the `assistant` turn that requested tool calls.
    ///
    /// `raw_tool_calls` is the backend's `tool_calls` array echoed back
    /// verbatim so the conversation history matches what the model emitted.
    #[must_use]
    pub fn assistant_tool_calls(raw_tool_calls: Vec<Value>) -> Message {
        Message {
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(raw_tool_calls),
        }
    }
}

/// A tool advertised to the model, in the `OpenAI` function-calling shape.
///
/// When serialized into a request the wrapping code turns this into
/// `{"type":"function","function":{"name":..,"description":..,"parameters":..}}`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ToolSchema {
    /// The tool's wire name.
    pub name: String,
    /// A one-sentence description shown to the model.
    pub description: String,
    /// The JSON Schema for the tool's parameters.
    pub parameters: Value,
}

/// A tool invocation requested by the model.
///
/// `OpenAI` returns tool calls with `function.arguments` as a JSON-encoded
/// string; this type holds that string parsed into a [`Value`] (falling back to
/// a string `Value` if it is not valid JSON).
#[derive(Debug, Clone)]
pub struct ToolCall {
    /// The id the model assigned to this call, echoed back with its result.
    pub id: String,
    /// The name of the tool to invoke.
    pub name: String,
    /// The parsed arguments for the call.
    pub arguments: Value,
}

/// The outcome of a completion round trip.
#[derive(Debug)]
#[non_exhaustive]
pub enum CompletionResult {
    /// The model returned a final text reply.
    Text(String),
    /// The model asked to call one or more tools.
    ToolCalls(Vec<ToolCall>),
}

/// A chat completions client bound to one gateway, token, and model.
#[derive(Debug, Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    token: String,
    model: String,
}

impl GatewayClient {
    /// Build a client from explicit parts (used by tests and by
    /// [`GatewayClient::from_env`]). A trailing slash on `base_url` is trimmed.
    #[must_use]
    pub fn new(
        base_url: &str,
        token: impl Into<String>,
        model: impl Into<String>,
    ) -> GatewayClient {
        GatewayClient {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.into(),
            model: model.into(),
        }
    }

    /// Build a client from the environment.
    ///
    /// - Base URL: `PROMPTFORGE_BASE_URL`, else the local gateway.
    /// - Model: `PROMPTFORGE_MODEL`, else a sane default.
    /// - Token: `PROMPTFORGE_TOKEN`, the gateway's shared bearer. Required.
    ///
    /// # Errors
    /// Returns [`Error::MissingEnv`] when `PROMPTFORGE_TOKEN` is not set.
    pub fn from_env() -> Result<GatewayClient> {
        let token = std::env::var("PROMPTFORGE_TOKEN")
            .map_err(|_| Error::MissingEnv("PROMPTFORGE_TOKEN".into()))?;
        let base_url =
            std::env::var("PROMPTFORGE_BASE_URL").unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let model = std::env::var("PROMPTFORGE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.into());
        Ok(GatewayClient::new(&base_url, token, model))
    }

    /// The model this client will call.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Send a list of messages and return the model's outcome.
    ///
    /// When `tools` is `Some` and non-empty, each schema is wrapped into the
    /// `OpenAI` function shape and sent as the request's `tools` array (with
    /// `tool_choice` set to `auto`); passing `None` or an empty slice sends no
    /// `tools` field, preserving the plain chat-completions behavior.
    ///
    /// # Errors
    /// Returns [`Error::Http`] on a transport failure, [`Error::Backend`] when
    /// the gateway responds with a non-success status, and
    /// [`Error::MalformedResponse`] when the response carries no usable content.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
    ) -> Result<CompletionResult> {
        let mut body = serde_json::json!({
            "model": self.model,
            "messages": messages,
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

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.token)
            .json(&body)
            .send()
            .await
            .map_err(Error::http)?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let body: String = body.chars().take(2000).collect();
            let body = if body.is_empty() {
                "(empty body)".to_string()
            } else {
                body
            };
            return Err(Error::Backend {
                status: status.as_u16(),
                body,
            });
        }

        let parsed: Value = response.json().await.map_err(Error::http)?;
        parse_completion(&parsed)
    }
}

/// Parse a chat-completions response body into a [`CompletionResult`].
///
/// Prefers a non-empty `tool_calls` array on the first choice's message; each
/// entry's `function.arguments` (a JSON-encoded string) is parsed into a
/// [`Value`], falling back to a string `Value` when it is not valid JSON. When
/// there are no tool calls, the message `content` is returned as text.
///
/// # Errors
/// Returns [`Error::MalformedResponse`] when there are no choices, or when the
/// chosen message has neither `content` nor `tool_calls`.
fn parse_completion(response_json: &Value) -> Result<CompletionResult> {
    let choice = response_json
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
        .ok_or_else(|| Error::MalformedResponse("no choices in response".into()))?;
    let message = choice
        .get("message")
        .ok_or_else(|| Error::MalformedResponse("choice had no message".into()))?;

    if let Some(raw_calls) = message
        .get("tool_calls")
        .and_then(Value::as_array)
        .filter(|calls| !calls.is_empty())
    {
        let mut calls = Vec::with_capacity(raw_calls.len());
        for raw in raw_calls {
            let id = raw
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::MalformedResponse("tool call had no id".into()))?
                .to_string();
            let function = raw
                .get("function")
                .ok_or_else(|| Error::MalformedResponse("tool call had no function".into()))?;
            let name = function
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| Error::MalformedResponse("tool call had no name".into()))?
                .to_string();
            let arguments = match function.get("arguments").and_then(Value::as_str) {
                Some(raw_args) => serde_json::from_str::<Value>(raw_args)
                    .unwrap_or_else(|_| Value::String(raw_args.to_string())),
                None => Value::Null,
            };
            calls.push(ToolCall {
                id,
                name,
                arguments,
            });
        }
        return Ok(CompletionResult::ToolCalls(calls));
    }

    if let Some(content) = message.get("content").and_then(Value::as_str) {
        return Ok(CompletionResult::Text(content.to_string()));
    }

    Err(Error::MalformedResponse(
        "choice had neither content nor tool_calls".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tool_calls_from_response() {
        let response = serde_json::json!({
            "choices": [{
                "index": 0,
                "message": {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "web_search",
                            "arguments": "{\"query\":\"rust\",\"count\":3}"
                        }
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        });

        let result = parse_completion(&response).unwrap();
        match result {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].name, "web_search");
                assert_eq!(
                    calls[0].arguments,
                    serde_json::json!({ "query": "rust", "count": 3 })
                );
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn falls_back_to_string_for_unparseable_arguments() {
        let response = serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_2",
                        "type": "function",
                        "function": { "name": "web_fetch", "arguments": "not json" }
                    }]
                }
            }]
        });

        let result = parse_completion(&response).unwrap();
        match result {
            CompletionResult::ToolCalls(calls) => {
                assert_eq!(calls[0].arguments, Value::String("not json".into()));
            }
            CompletionResult::Text(text) => panic!("expected tool calls, got text: {text}"),
        }
    }

    #[test]
    fn parses_text_when_no_tool_calls() {
        let response = serde_json::json!({
            "choices": [{
                "message": { "role": "assistant", "content": "hello" }
            }]
        });

        let result = parse_completion(&response).unwrap();
        match result {
            CompletionResult::Text(text) => assert_eq!(text, "hello"),
            CompletionResult::ToolCalls(_) => panic!("expected text, got tool calls"),
        }
    }

    #[test]
    fn errors_when_neither_content_nor_tool_calls() {
        let response = serde_json::json!({
            "choices": [{ "message": { "role": "assistant" } }]
        });

        assert!(matches!(
            parse_completion(&response),
            Err(Error::MalformedResponse(_))
        ));
    }
}
