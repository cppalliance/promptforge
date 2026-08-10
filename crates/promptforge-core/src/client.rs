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
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::dialects::{DialectRequest, ToolDialectRegistry};
use crate::model::{CompletionError, CompletionOptions};
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
    pub(crate) role: String,
    /// The message text.
    pub(crate) content: String,
    /// For a `tool` message, the id of the tool call this result answers.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    /// For an `assistant` turn that requested tools, the raw `tool_calls` array
    /// as received from the backend, echoed back verbatim.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_calls: Option<Vec<Value>>,
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
    pub(crate) fn assistant_tool_calls(raw_tool_calls: Vec<Value>) -> Message {
        Message {
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            tool_calls: Some(raw_tool_calls),
        }
    }

    /// Returns the message role (`system`, `user`, `assistant`, or `tool`).
    #[must_use]
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Returns the message text.
    #[must_use]
    pub fn content(&self) -> &str {
        &self.content
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
    pub(crate) name: String,
    /// A one-sentence description shown to the model.
    pub(crate) description: String,
    /// The JSON Schema for the tool's parameters.
    pub(crate) parameters: Value,
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
    pub(crate) id: String,
    /// The name of the tool to invoke.
    pub(crate) name: String,
    /// The parsed arguments for the call.
    pub(crate) arguments: Value,
}

impl ToolCall {
    /// Returns the id the model assigned to this call.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the name of the tool to invoke.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the parsed arguments for the call.
    #[must_use]
    pub fn arguments(&self) -> &Value {
        &self.arguments
    }
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
    pub(crate) result: CompletionResult,
    /// The choice's `finish_reason`, when the backend supplied one.
    pub(crate) finish_reason: Option<String>,
    /// The message's reasoning side channel, when the backend supplied one.
    pub(crate) reasoning_content: Option<String>,
    /// The JSON body sent to the gateway.
    pub(crate) request_body: Value,
    /// The JSON body returned by the gateway.
    pub(crate) response_body: Value,
}

impl Completion {
    /// Returns the text or tool-call outcome the tool loop consumes.
    #[must_use]
    pub fn result(&self) -> &CompletionResult {
        &self.result
    }

    /// Returns the choice's `finish_reason`, when the backend supplied one.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Returns the reasoning side channel, when the backend supplied one. It is
    /// never promoted into the answer.
    #[must_use]
    pub fn reasoning_content(&self) -> Option<&str> {
        self.reasoning_content.as_deref()
    }
}

/// A bearer credential whose contents never appear in `Debug`, `Display`, or
/// logs.
///
/// Wrap any secret (the gateway bearer key) in a `SecretString` at the boundary
/// so an accidental `{:?}` or log line cannot leak it; only crate-internal
/// transport code reads the exposed value to set the `Authorization` header.
#[derive(Clone)]
pub struct SecretString(String);

impl SecretString {
    /// Wraps a secret so it is redacted everywhere it is formatted.
    #[must_use]
    pub fn new(secret: impl Into<String>) -> SecretString {
        SecretString(secret.into())
    }

    /// Borrows the raw secret. Crate-internal so no downstream code can read a
    /// credential back out of the type.
    pub(crate) fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretString(<redacted>)")
    }
}

impl fmt::Display for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<redacted>")
    }
}

/// A validated gateway API base URL (the OpenAI-shaped `/v1` root).
///
/// Construction rejects a URL without an `http`/`https` scheme or host, so a
/// client can never be pointed at an unusable endpoint. A trailing slash is
/// trimmed so request paths join cleanly.
#[non_exhaustive]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayEndpoint {
    url: String,
}

impl GatewayEndpoint {
    /// Validates and normalizes a gateway base URL.
    ///
    /// # Errors
    /// Returns a `Config`-kind [`CompletionError`] when `url` is empty, lacks an
    /// `http`/`https` scheme, or names no host.
    pub fn new(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        let trimmed = url.trim();
        let rest = trimmed
            .strip_prefix("https://")
            .or_else(|| trimmed.strip_prefix("http://"));
        let Some(after_scheme) = rest else {
            return Err(CompletionError::from(Error::MissingEnv(format!(
                "gateway URL must begin with http:// or https://: {trimmed:?}"
            ))));
        };
        let host = after_scheme.split(['/', '?', '#']).next().unwrap_or("");
        if host.is_empty() {
            return Err(CompletionError::from(Error::MissingEnv(format!(
                "gateway URL names no host: {trimmed:?}"
            ))));
        }
        Ok(GatewayEndpoint {
            url: trimmed.trim_end_matches('/').to_string(),
        })
    }

    /// Returns the normalized base URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }
}

impl TryFrom<&str> for GatewayEndpoint {
    type Error = CompletionError;

    fn try_from(url: &str) -> std::result::Result<GatewayEndpoint, CompletionError> {
        GatewayEndpoint::new(url)
    }
}

/// A chat completions client bound to one gateway URL and shared bearer key.
#[derive(Clone)]
pub struct GatewayClient {
    transport: GatewayTransport,
    base_url: String,
    key: SecretString,
    dialect_registry: Arc<ToolDialectRegistry>,
    /// Wall-clock cap applied to each completion request.
    request_timeout: Duration,
    /// Byte ceiling enforced on a response body before it is decoded.
    max_response_bytes: u64,
}

/// Default per-request timeout, matching [`crate::execute::RunLimits`].
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Default response-body ceiling, matching [`crate::execute::RunLimits`].
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
enum GatewayTransport {
    Http(reqwest::Client),
    Disabled,
}

impl fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer key is a credential and must never appear in Debug output,
        // logs, or panic messages. It is redacted to a fixed marker regardless of
        // whether one is set, so no length or presence signal leaks either.
        f.debug_struct("GatewayClient")
            .field("base_url", &self.base_url)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Build a client from a validated [`GatewayEndpoint`] and a redacted
    /// [`SecretString`] bearer key (used by tests and by
    /// [`GatewayClient::from_env`]). Responses are parsed through the resolved
    /// tool dialect.
    #[must_use]
    pub fn new(endpoint: GatewayEndpoint, key: SecretString) -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Http(reqwest::Client::new()),
            base_url: endpoint.url,
            key,
            dialect_registry: Arc::new(ToolDialectRegistry::builtin()),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Build a client that cannot read gateway configuration or send HTTP.
    ///
    /// Hosts use this explicit sentinel for hermetic execution paths. Any
    /// attempted model call fails with a `Disabled`-kind [`CompletionError`].
    #[must_use]
    pub fn disabled() -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Disabled,
            base_url: String::new(),
            key: SecretString::new(String::new()),
            dialect_registry: Arc::new(ToolDialectRegistry::builtin()),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Applies the run's HTTP limits to this client.
    ///
    /// Each completion request is bounded by `request_timeout`, and the response
    /// body is refused once it would exceed `max_response_bytes` before any
    /// UTF-8 or JSON decoding runs.
    #[must_use]
    pub fn with_request_limits(
        mut self,
        request_timeout: Duration,
        max_response_bytes: NonZeroU64,
    ) -> GatewayClient {
        self.request_timeout = request_timeout;
        self.max_response_bytes = max_response_bytes.get();
        self
    }

    /// Build a client from the environment.
    ///
    /// - URL: `PROMPTFORGE_GATEWAY_URL`. Required.
    /// - Key: `PROMPTFORGE_GATEWAY_KEY`, the gateway's shared bearer. Required.
    ///
    /// # Errors
    /// Returns a [`CompletionError`] with `Config` kind when either variable is
    /// not set.
    pub fn from_env() -> std::result::Result<GatewayClient, CompletionError> {
        from_env_with(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            // A set-but-non-Unicode value is a real misconfiguration, surfaced
            // explicitly instead of being silently treated as "not set".
            Err(std::env::VarError::NotUnicode(_)) => Err(Error::InvalidEnv(name.to_owned())),
        })
        .map_err(CompletionError::from)
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
    /// Returns a [`CompletionError`] classified `Transport` on a transport
    /// failure, `Backend` when the gateway responds with a non-success status,
    /// `MalformedResponse` when the response shape is unusable, and `EmptyReply`
    /// when the turn has neither non-empty tool calls nor non-empty text.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        options: &CompletionOptions,
    ) -> std::result::Result<Completion, CompletionError> {
        let GatewayTransport::Http(http) = &self.transport else {
            return Err(CompletionError::from(Error::GatewayDisabled));
        };
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

        let response = http
            .post(format!("{}/chat/completions", self.base_url))
            .bearer_auth(self.key.expose())
            .timeout(self.request_timeout)
            .json(&request_body)
            .send()
            .await
            .map_err(Error::http)?;

        let status = response.status();
        let raw_body = read_body_capped(response, self.max_response_bytes).await?;
        if !status.is_success() {
            let body = String::from_utf8_lossy(&raw_body);
            let body: String = body.chars().take(2000).collect();
            let body = if body.is_empty() {
                "(empty body)".to_string()
            } else {
                body
            };
            return Err(CompletionError::from(Error::Backend {
                status: status.as_u16(),
                body,
            }));
        }

        let response_body: Value = serde_json::from_slice(&raw_body)
            .map_err(|error| Error::MalformedResponse(error.to_string()))?;
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

/// Reads a response body, refusing it once it would exceed `cap` bytes.
///
/// The advertised `Content-Length` short-circuits an oversize body, and the
/// streamed chunks are bounded so a gateway that omits or lies about the length
/// still cannot force an unbounded allocation before decoding.
async fn read_body_capped(mut response: reqwest::Response, cap: u64) -> Result<Vec<u8>> {
    if let Some(len) = response.content_length()
        && len > cap
    {
        return Err(Error::MalformedResponse(format!(
            "response body of {len} bytes exceeds the {cap}-byte limit"
        )));
    }
    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(Error::http)? {
        if body.len() as u64 + chunk.len() as u64 > cap {
            return Err(Error::MalformedResponse(format!(
                "response body exceeds the {cap}-byte limit"
            )));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn from_env_with(
    lookup: impl Fn(&str) -> std::result::Result<Option<String>, Error>,
) -> Result<GatewayClient> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let key = lookup("PROMPTFORGE_GATEWAY_KEY")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url).map_err(Error::from)?;
    Ok(GatewayClient::new(endpoint, SecretString::new(key)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lookup_from<'a>(
        pairs: &'a [(&'a str, &'a str)],
    ) -> impl Fn(&str) -> std::result::Result<Option<String>, Error> + 'a {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name| {
            Ok(pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone()))
        }
    }

    #[test]
    fn from_env_surfaces_non_unicode_value_instead_of_dropping_it() {
        let err = from_env_with(|name| {
            if name == "PROMPTFORGE_GATEWAY_URL" {
                Err(Error::InvalidEnv(name.to_owned()))
            } else {
                Ok(Some("tok".to_owned()))
            }
        })
        .expect_err("a non-Unicode variable must be surfaced, not treated as missing");
        assert!(
            matches!(err, Error::InvalidEnv(ref name) if name == "PROMPTFORGE_GATEWAY_URL"),
            "expected an explicit InvalidEnv error, got {err:?}"
        );
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
    fn debug_redacts_the_bearer_key_and_never_leaks_it() {
        let client = GatewayClient::new(
            GatewayEndpoint::new("http://127.0.0.1:8081/v1").expect("valid test endpoint"),
            SecretString::new("super-secret-token"),
        );
        let rendered = format!("{client:?}");
        assert!(
            !rendered.contains("super-secret-token"),
            "the bearer key must never appear in Debug output, got: {rendered}"
        );
        assert!(
            rendered.contains("<redacted>"),
            "the key field must be redacted, got: {rendered}"
        );
        assert!(
            rendered.contains("http://127.0.0.1:8081/v1"),
            "the base URL is not a secret and should still appear, got: {rendered}"
        );
    }

    #[test]
    fn secret_string_never_prints_its_contents() {
        let secret = SecretString::new("super-secret-token");
        assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
        assert_eq!(format!("{secret}"), "<redacted>");
        assert_eq!(secret.expose(), "super-secret-token");
    }

    #[test]
    fn gateway_endpoint_rejects_non_http_schemes_and_missing_host() {
        assert!(GatewayEndpoint::new("ftp://example.com/v1").is_err());
        assert!(GatewayEndpoint::new("not-a-url").is_err());
        assert!(GatewayEndpoint::new("http:///v1").is_err());
        assert!(GatewayEndpoint::new("").is_err());
    }

    #[test]
    fn gateway_endpoint_trims_trailing_slash_and_keeps_valid_urls() {
        let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")
            .expect("a well-formed https URL is accepted");
        assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
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

        let client = GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
            SecretString::new("tok"),
        );
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

        let client = GatewayClient::new(
            GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
            SecretString::new("tok"),
        );
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
        assert!(matches!(Error::from(err), Error::EmptyModelReply { .. }));
    }
}
