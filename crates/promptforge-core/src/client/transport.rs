//! The HTTP transport: the gateway client, request construction, bounded
//! response reading, and environment loading.

use std::fmt;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use super::{Completion, GatewayEndpoint, Message, SecretString, ToolSchema};
use crate::dialects::{DialectRequest, ToolDialectRegistry};
use crate::model::{CompletionError, CompletionOptions};
use crate::{Error, Result};

/// A chat completions client bound to one gateway URL and shared bearer key.
#[derive(Clone)]
#[non_exhaustive]
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
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
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
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), promptforge_core::model::CompletionError> {
    /// use promptforge_core::client::{GatewayClient, GatewayEndpoint, Message, SecretString};
    /// use promptforge_core::model::CompletionOptions;
    /// use promptforge_core::dialects::ToolDialectId;
    ///
    /// let client = GatewayClient::new(
    ///     GatewayEndpoint::new("http://127.0.0.1:8081/v1")?,
    ///     SecretString::new("bearer-token")?,
    /// );
    /// let options = CompletionOptions::new("analyst", ToolDialectId::OpenAi);
    /// let completion = client
    ///     .complete(&[Message::user("hello")], None, &options)
    ///     .await?;
    /// let _ = completion.result();
    /// # Ok(())
    /// # }
    /// ```
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
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn run() {
    /// use promptforge_core::client::{GatewayClient, Message};
    /// use promptforge_core::model::{CompletionErrorKind, CompletionOptions};
    /// use promptforge_core::dialects::ToolDialectId;
    ///
    /// let client = GatewayClient::disabled();
    /// let options = CompletionOptions::new("m", ToolDialectId::OpenAi);
    /// let error = client
    ///     .complete(&[Message::user("hi")], None, &options)
    ///     .await
    ///     .expect_err("a disabled client cannot complete");
    /// assert_eq!(error.kind(), CompletionErrorKind::Disabled);
    /// # }
    /// ```
    #[must_use]
    pub fn disabled() -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Disabled,
            base_url: String::new(),
            key: SecretString::disabled_placeholder(),
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
    ///
    /// # Examples
    ///
    /// ```
    /// use std::num::NonZeroU64;
    /// use std::time::Duration;
    ///
    /// use promptforge_core::client::GatewayClient;
    ///
    /// let cap = NonZeroU64::new(1024 * 1024).ok_or("cap is non-zero")?;
    /// let client = GatewayClient::disabled().with_request_limits(Duration::from_secs(30), cap);
    /// let _ = client;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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
    /// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
    /// (F11 - the full reachable set):
    /// - `Disabled` when this client was built with [`GatewayClient::disabled`];
    /// - `Config` when the selected tool dialect is unknown, or dialect request
    ///   preparation fails;
    /// - `Transport` on a transport-layer failure (connection, timeout);
    /// - `Backend` when the gateway responds with a non-success status;
    /// - `MalformedResponse` when the body exceeds the size cap or its shape is
    ///   unusable (the JSON decode failure is retained as a private `#[source]`);
    /// - `EmptyReply` when the turn has neither non-empty tool calls nor
    ///   non-empty text.
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
            request_body["temperature"] = serde_json::json!(temperature.get());
        }
        if let Some(max_tokens) = options.max_tokens {
            request_body["max_tokens"] = serde_json::json!(max_tokens.get());
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
        let mut dr = DialectRequest::new(&mut request_body);
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
            // F5: bound the body, then escape control characters so a hostile
            // payload cannot forge log lines. The escaped body is kept only for
            // the opt-in `CompletionError::backend_body` accessor, never the
            // public `Display`.
            let body = String::from_utf8_lossy(&raw_body);
            let body = escape_controls(&body, 2000);
            return Err(CompletionError::from(Error::Backend {
                status: status.as_u16(),
                body,
            }));
        }

        let response_body: Value = serde_json::from_slice(&raw_body).map_err(|error| {
            // F11 / MODEL-009: retain the decode failure as a private `#[source]`
            // cause rather than flattening it into the message string.
            Error::MalformedResponseSource {
                message: "completion response was not valid JSON".to_owned(),
                source: Box::new(error),
            }
        })?;
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

/// Escapes control characters in a diagnostic body and bounds it to `max` chars.
///
/// Control characters (including newlines and carriage returns) are rendered in
/// their `\u{..}`/`\n` escaped form so a backend body cannot forge log lines or
/// smuggle terminal control sequences into a diagnostic (F5). An empty body is
/// reported as a fixed marker.
pub(crate) fn escape_controls(body: &str, max: usize) -> String {
    if body.is_empty() {
        return "(empty body)".to_owned();
    }
    let mut escaped = String::with_capacity(body.len());
    for ch in body.chars().take(max) {
        if ch.is_control() {
            for part in ch.escape_default() {
                escaped.push(part);
            }
        } else {
            escaped.push(ch);
        }
    }
    escaped
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

pub(crate) fn from_env_with(
    lookup: impl Fn(&str) -> std::result::Result<Option<String>, Error>,
) -> Result<GatewayClient> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let key = lookup("PROMPTFORGE_GATEWAY_KEY")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url).map_err(Error::from)?;
    let key = SecretString::new(key)
        .map_err(|_| Error::MissingEnv("PROMPTFORGE_GATEWAY_KEY must not be empty".into()))?;
    Ok(GatewayClient::new(endpoint, key))
}
