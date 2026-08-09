//! An `OpenAI`-compatible chat completions client, pointed at the gateway.
//!
//! The client speaks the non-streaming `/chat/completions` shape: a list of
//! messages in, and either one text reply out or the tool calls the model
//! asked for. [`GatewayClient::complete`] sends a `tools` array when the caller
//! supplies one, so the executor's tool-call loop runs over this client.
//! Streaming is not supported. The client holds only the gateway's URL and the
//! shared key; the vendor credential lives in the gateway, so the executor
//! never sees it. Point `PROMPTFORGE_GATEWAY_URL` at a local server or another
//! gateway to retarget it.

use std::fmt;
use std::sync::Arc;

use serde_json::Value;

use crate::dialects::{DialectRequest, ToolDialectRegistry};
use crate::model::CompletionOptions;
use crate::normalize::{CompletionNormalizer, OpenAiChatNormalizer};
use crate::{Error, Result};

/// A single chat message.
///
/// A plain `user` message serializes to just `{"role":..,"content":..}`; the
/// optional `tool_call_id` and `tool_calls` fields are emitted only when set,
/// which keeps the wire shape of ordinary messages unchanged.
#[derive(Debug, Clone, serde::Serialize)]
#[non_exhaustive]
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

    /// Construct a plain `assistant` text turn (no `tool_calls` field).
    #[must_use]
    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: "assistant".into(),
            content: content.into(),
            tool_call_id: None,
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
#[non_exhaustive]
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
#[non_exhaustive]
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

/// A parsed chat-completions round trip, including metadata later steps need.
///
/// [`CompletionResult`] remains the decision the tool loop matches on.
/// `finish_reason` and `reasoning_content` ride beside it so observers can
/// report payload-free signals without reading the raw bodies. The request and
/// response bodies are `pub(crate)` for the opt-in [`crate::debug::DebugCapture`]
/// seam; they are not part of the public host API.
#[derive(Debug)]
#[non_exhaustive]
pub struct Completion {
    /// The text or tool-call outcome the tool loop consumes.
    pub result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    pub reasoning_content: Option<String>,
    /// The JSON body sent to the gateway.
    pub(crate) request_body: Value,
    /// The JSON body returned by the gateway.
    pub(crate) response_body: Value,
}

/// A chat completions client bound to one gateway URL and shared bearer key.
#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    key: String,
    normalizer: Arc<dyn CompletionNormalizer>,
    dialect_registry: Arc<ToolDialectRegistry>,
}

impl fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GatewayClient")
            .field("base_url", &self.base_url)
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Build a client from explicit parts (used by tests and by
    /// [`GatewayClient::from_env`]). A trailing slash on `base_url` is trimmed.
    /// The response normalizer defaults to [`OpenAiChatNormalizer`].
    #[must_use]
    pub fn new(base_url: &str, key: impl Into<String>) -> GatewayClient {
        GatewayClient {
            http: reqwest::Client::new(),
            base_url: base_url.trim_end_matches('/').to_string(),
            key: key.into(),
            normalizer: OpenAiChatNormalizer::shared(),
            dialect_registry: Arc::new(ToolDialectRegistry::builtin()),
        }
    }

    /// Replace the response normalizer used by [`GatewayClient::complete`].
    #[must_use]
    pub fn with_normalizer(mut self, normalizer: Arc<dyn CompletionNormalizer>) -> GatewayClient {
        self.normalizer = normalizer;
        self
    }

    /// Build a client from the environment.
    ///
    /// - URL: `PROMPTFORGE_GATEWAY_URL`. Required.
    /// - Key: `PROMPTFORGE_GATEWAY_KEY`, the gateway's shared bearer. Required.
    ///
    /// # Errors
    /// Returns [`Error::MissingEnv`] when either variable is not set.
    pub fn from_env() -> Result<GatewayClient> {
        from_env_with(|name| std::env::var(name).ok())
    }

    /// Send a list of messages and return the model's outcome.
    ///
    /// When `tools` is `Some` and non-empty, each schema is wrapped into the
    /// `OpenAI` function shape and sent as the request's `tools` array (with
    /// `tool_choice` set to `auto`); passing `None` or an empty slice sends no
    /// `tools` field, preserving the plain chat-completions behavior.
    ///
    /// `options.model` names the model on the wire. Optional `temperature`,
    /// `max_tokens`, and `thinking` extend the request when present.
    ///
    /// # Errors
    /// Returns [`Error::Http`] on a transport failure, [`Error::Backend`] when
    /// the gateway responds with a non-success status,
    /// [`Error::MalformedResponse`] when the response shape is unusable, and
    /// [`Error::EmptyModelReply`] when the turn has neither non-empty tool
    /// calls nor non-empty text.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        options: &CompletionOptions,
    ) -> Result<Completion> {
        let mut request_body = serde_json::json!({
            "model": options.model,
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
            request_body["tools"] = Value::Array(wrapped);
            request_body["tool_choice"] = Value::String("auto".into());
        }
        if let Some(temperature) = options.temperature {
            request_body["temperature"] = serde_json::json!(temperature);
        }
        if let Some(max_tokens) = options.max_tokens {
            request_body["max_tokens"] = serde_json::json!(max_tokens);
        }
        if let Some(thinking) = options.thinking {
            request_body["chat_template_kwargs"] = serde_json::json!({
                "enable_thinking": thinking,
            });
        }

        let dialect = self
            .dialect_registry
            .get(options.tool_dialect)
            .ok_or(Error::UnknownDialect(options.tool_dialect))?;
        let mut dr = DialectRequest {
            body: &mut request_body,
        };
        dialect.prepare_request(&mut dr)?;

        let response = self
            .http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(&self.key)
            .json(&request_body)
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

        let response_body: Value = response.json().await.map_err(Error::http)?;
        let turn = dialect.parse_turn(&response_body)?;
        Ok(Completion {
            result: turn.outcome,
            finish_reason: turn.finish_reason,
            reasoning_content: turn.reasoning_content,
            request_body,
            response_body,
        })
    }
}

fn from_env_with(lookup: impl Fn(&str) -> Option<String>) -> Result<GatewayClient> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let key = lookup("PROMPTFORGE_GATEWAY_KEY")
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY".into()))?;
    Ok(GatewayClient::new(&base_url, key))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn from_env_missing_gateway_url() {
        let err = from_env_with(lookup_from(&[("PROMPTFORGE_GATEWAY_KEY", "tok")]))
            .expect_err("missing URL must fail");
        assert!(matches!(
            err,
            Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_URL"
        ));
    }

    #[test]
    fn from_env_missing_gateway_key() {
        let err = from_env_with(lookup_from(&[(
            "PROMPTFORGE_GATEWAY_URL",
            "http://127.0.0.1:8081/v1",
        )]))
        .expect_err("missing key must fail");
        assert!(matches!(
            err,
            Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_KEY"
        ));
    }

    #[test]
    fn client_constructs_without_model() {
        let client = GatewayClient::new("http://127.0.0.1:8081/v1", "tok");
        assert_eq!(
            format!("{client:?}"),
            "GatewayClient { base_url: \"http://127.0.0.1:8081/v1\", key: \"tok\", .. }"
        );
    }

    #[tokio::test]
    async fn complete_sends_completion_options_model_on_the_wire() {
        use std::sync::{Arc, Mutex};

        use axum::Router;
        use axum::extract::Json;
        use axum::routing::post;
        use serde_json::{Value, json};
        use tokio::net::TcpListener;

        let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let slot = Arc::clone(&captured);
        let app = Router::new().route(
            "/v1/chat/completions",
            post(move |Json(body): Json<Value>| {
                let slot = Arc::clone(&slot);
                async move {
                    *slot.lock().expect("capture lock") = Some(body);
                    Json(json!({
                        "choices": [{
                            "message": { "role": "assistant", "content": "ok" },
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = GatewayClient::new(&format!("http://{addr}/v1"), "tok");
        let options = CompletionOptions {
            model: "analyst".into(),
            temperature: Some(0.0),
            max_tokens: Some(128),
            thinking: Some(false),
            tool_dialect: crate::dialects::ToolDialectId::OpenAi,
        };
        client
            .complete(&[Message::user("hi")], None, &options)
            .await
            .unwrap();
        let body = captured.lock().expect("capture lock").clone().unwrap();
        assert_eq!(body["model"], "analyst");
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["max_tokens"], 128);
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    }

    #[tokio::test]
    async fn complete_hard_fails_on_empty_model_reply() {
        use axum::Router;
        use axum::extract::Json;
        use axum::routing::post;
        use serde_json::{Value, json};
        use tokio::net::TcpListener;

        let app = Router::new().route(
            "/v1/chat/completions",
            post(|Json(_body): Json<Value>| async move {
                Json(json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "",
                            "reasoning_content": "ignored"
                        },
                        "finish_reason": "stop"
                    }]
                }))
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = GatewayClient::new(&format!("http://{addr}/v1"), "tok");
        let options = CompletionOptions {
            model: "m".into(),
            temperature: None,
            max_tokens: None,
            thinking: None,
            tool_dialect: crate::dialects::ToolDialectId::OpenAi,
        };
        let err = client
            .complete(&[Message::user("hi")], None, &options)
            .await
            .expect_err("empty product must fail");
        assert!(matches!(err, Error::EmptyModelReply { .. }));
    }
}
