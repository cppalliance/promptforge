//! The HTTP transport: the gateway client, request construction, bounded
//! SSE response reading, and environment loading.

use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use promptforge_core_support::events::ClientTiming;
use serde_json::Value;

use super::stream::{Applied, SseScanner, StreamAccumulator};
use super::{Completion, GatewayEndpoint, Message, SecretString, StreamDelta, ToolSchema};
use crate::model::{CompletionError, CompletionOptions};
use crate::{Error, Result};

/// A chat completions client bound to one gateway URL and, usually, the
/// gateway's shared bearer key.
///
/// The key is optional: a gateway on the same machine admits keyless
/// loopback callers by default, and a client built without a key
/// ([`GatewayClient::keyless`]) sends no `Authorization` header at all.
#[derive(Clone)]
#[non_exhaustive]
pub struct GatewayClient {
    transport: GatewayTransport,
    base_url: String,
    /// The bearer presented on every request, or `None` to present nothing.
    key: Option<SecretString>,
    /// Wall-clock cap applied to each completion request.
    request_timeout: Duration,
    /// Byte ceiling enforced on a response body before it is decoded.
    max_response_bytes: u64,
}

/// Default per-request timeout, matching the executor's run limits.
pub(crate) const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Default response-body ceiling, matching the executor's run limits.
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
enum GatewayTransport {
    Http(reqwest::Client),
    Disabled,
}

/// Builds the completion request body.
///
/// Every request streams: `stream` is always true and
/// `stream_options.include_usage` asks the backend for the final
/// empty-choices usage chunk, so token accounting survives the SSE path.
fn build_request_body(
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
    /// [`GatewayClient::from_env`]).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), promptforge_model_client::model::CompletionError> {
    /// use promptforge_model_client::client::{GatewayClient, GatewayEndpoint, Message, SecretString};
    /// use promptforge_model_client::model::CompletionOptions;
    ///
    /// let client = GatewayClient::new(
    ///     GatewayEndpoint::new("http://127.0.0.1:8081/v1")?,
    ///     SecretString::new("bearer-token")?,
    /// );
    /// let options = CompletionOptions::new("analyst");
    /// let completion = client
    ///     .complete(&[Message::user("hello")], None, &options, |_delta| {})
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
            key: Some(key),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Build a client that presents no bearer key.
    ///
    /// Every request goes out without an `Authorization` header. This fits a
    /// gateway on the same machine, which trusts keyless loopback callers by
    /// default (and, on a shared machine, every other OS account there)
    /// unless its operator set `trust_loopback = false`; against any other
    /// gateway the requests fail with a `Backend` 401. Nothing here checks
    /// the endpoint's host - the caller decides, and
    /// [`GatewayClient::from_env`] decides by [`GatewayEndpoint::is_loopback`].
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_model_client::client::{GatewayClient, GatewayEndpoint};
    ///
    /// let endpoint = GatewayEndpoint::new("http://127.0.0.1:8081/v1")?;
    /// let client = GatewayClient::keyless(endpoint);
    /// let _ = client;
    /// # Ok::<(), promptforge_model_client::model::CompletionError>(())
    /// ```
    #[must_use]
    pub fn keyless(endpoint: GatewayEndpoint) -> GatewayClient {
        GatewayClient {
            transport: GatewayTransport::Http(reqwest::Client::new()),
            base_url: endpoint.url,
            key: None,
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
    /// use promptforge_model_client::client::{GatewayClient, Message};
    /// use promptforge_model_client::model::{CompletionErrorKind, CompletionOptions};
    ///
    /// let client = GatewayClient::disabled();
    /// let options = CompletionOptions::new("m");
    /// let error = client
    ///     .complete(&[Message::user("hi")], None, &options, |_delta| {})
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
            key: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Whether this client presents a bearer key; a test seam for the
    /// environment constructor, which never exposes the key itself.
    #[cfg(test)]
    pub(crate) fn has_key(&self) -> bool {
        self.key.is_some()
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
    /// use promptforge_model_client::client::GatewayClient;
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

    /// Builds a client from the environment.
    ///
    /// - URL: `PROMPTFORGE_GATEWAY_URL`. Required.
    /// - Key: `PROMPTFORGE_GATEWAY_API_KEY`, the gateway's shared bearer.
    ///   Required unless the URL's host is loopback (`127.0.0.1`, `::1`,
    ///   `localhost`); a loopback gateway trusts keyless same-machine callers
    ///   by default, so the client is then built keyless and sends no
    ///   `Authorization` header. An empty value counts as unset. That trust
    ///   also admits every other OS account on a shared machine, so an
    ///   operator there sets `trust_loopback = false`; then set the key, or
    ///   a keyless client's requests fail with a `Backend` 401.
    ///
    /// # Errors
    /// Returns a [`CompletionError`] with `Config` kind when
    /// `PROMPTFORGE_GATEWAY_URL` is unset or invalid, when either variable is
    /// set to a non-Unicode value, or when the URL's host is not loopback (a
    /// LAN or remote gateway) and `PROMPTFORGE_GATEWAY_API_KEY` is unset or
    /// empty.
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

    /// Send a list of messages and return the model's accumulated outcome.
    ///
    /// The one completion method, always streaming: the request asks for SSE
    /// with `stream_options.include_usage`, deltas are accumulated into the
    /// buffered body shape, and `on_delta` is invoked live with each
    /// [`StreamDelta`] text or reasoning fragment (a caller with no use for
    /// deltas passes a no-op closure). The returned [`Completion`] carries
    /// the reassembled turn, the metadata parsed from the stream's summary
    /// chunk, and a [`ClientTiming`](crate::ClientTiming) measured on this
    /// client's own clock (TTFT, mean inter-token latency, end-to-end).
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
    /// - `Transport` on a transport-layer failure (connection, timeout) or
    ///   when the stream carries a mid-flight error envelope;
    /// - `Backend` when the gateway responds with a non-success status;
    /// - `MalformedResponse` when the stream exceeds the size cap, a chunk's
    ///   shape is unusable (the JSON decode failure is retained as a private
    ///   `#[source]`), the stream ends without the `[DONE]` sentinel, or a
    ///   tool-call batch is truncated by a `length`/`content_filter` finish
    ///   reason (partial arguments must not execute);
    /// - `EmptyReply` when the turn has neither non-empty tool calls nor
    ///   non-empty text.
    pub async fn complete(
        &self,
        messages: &[Message],
        tools: Option<&[ToolSchema]>,
        options: &CompletionOptions,
        on_delta: impl Fn(StreamDelta),
    ) -> std::result::Result<Completion, CompletionError> {
        let GatewayTransport::Http(http) = &self.transport else {
            return Err(CompletionError::from(Error::GatewayDisabled));
        };
        let request_body = build_request_body(messages, tools, options);

        let started = Instant::now();
        let mut request = http
            .post(format!("{}/chat/completions", self.base_url))
            // reqwest's whole-request timeout covers the body read, so the
            // run's wall-clock cap bounds the entire stream, not just the
            // connection.
            .timeout(self.request_timeout)
            .json(&request_body);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key.expose());
        }
        let mut response = request.send().await.map_err(Error::http)?;

        let status = response.status();
        if !status.is_success() {
            let raw_body = read_body_capped(response, self.max_response_bytes).await?;
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

        let mut scanner = SseScanner::new();
        let mut accumulator = StreamAccumulator::new();
        let mut received: u64 = 0;
        let mut first_delta: Option<Instant> = None;
        let mut last_delta: Option<Instant> = None;
        let mut delta_chunks: u32 = 0;
        let mut done = false;
        'read: while let Some(bytes) = response.chunk().await.map_err(Error::http)? {
            received += bytes.len() as u64;
            if received > self.max_response_bytes {
                return Err(CompletionError::from(Error::MalformedResponse(format!(
                    "response stream exceeds the {}-byte limit",
                    self.max_response_bytes
                ))));
            }
            scanner.extend(&bytes);
            while let Some(data) = scanner.next_data() {
                match accumulator.apply(&data, &on_delta)? {
                    Applied::Done => {
                        done = true;
                        break 'read;
                    }
                    Applied::Chunk { delta: true } => {
                        let now = Instant::now();
                        first_delta.get_or_insert(now);
                        last_delta = Some(now);
                        delta_chunks += 1;
                    }
                    Applied::Chunk { delta: false } => {}
                }
            }
        }
        // A stream that ends without the sentinel was cut off; its
        // accumulation may be missing the tail, so it must never pass for a
        // complete turn.
        if !done {
            return Err(CompletionError::from(Error::MalformedResponse(
                "completion stream ended without the [DONE] sentinel".into(),
            )));
        }

        // The truncation rule runs before normalization: a tool-call batch
        // cut short by `length` or `content_filter` may hold partial JSON
        // arguments, and partial arguments must not execute.
        if accumulator.has_tool_calls()
            && matches!(
                accumulator.finish_reason(),
                Some("length" | "content_filter")
            )
        {
            let reason = accumulator.finish_reason().unwrap_or_default().to_owned();
            return Err(CompletionError::from(Error::MalformedResponse(format!(
                "tool-call batch truncated by finish_reason {reason:?}: \
                 partial arguments must not execute"
            ))));
        }

        let client_timing = ClientTiming {
            ttft_ms: first_delta.map(|at| duration_ms(at.duration_since(started))),
            mean_itl_ms: match (first_delta, last_delta) {
                (Some(first), Some(last)) if delta_chunks >= 2 => {
                    Some(duration_ms(last.duration_since(first)) / f64::from(delta_chunks - 1))
                }
                _ => None,
            },
            e2e_ms: duration_ms(started.elapsed()),
        };

        let response_body = accumulator.into_body();
        let turn = crate::normalize::normalize(&response_body)?;
        let metadata = crate::normalize::response_metadata(&response_body);
        Ok(Completion {
            result: turn.outcome,
            finish_reason: turn.finish_reason,
            reasoning_content: turn.reasoning_content,
            model: metadata.model,
            usage: metadata.usage,
            llama_timings: metadata.llama_timings,
            vllm_metrics: metadata.vllm_metrics,
            client_timing: Some(client_timing),
            request_body,
            response_body,
        })
    }
}

/// A duration as fractional milliseconds.
fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
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

/// The environment-driven constructor behind [`GatewayClient::from_env`],
/// with the variable lookup injected so tests need not touch the process
/// environment.
///
/// The key is optional exactly when the URL's host is loopback; an empty key
/// counts as unset ([`SecretString::new`] refuses only an empty secret, and
/// `Result::ok` folds that refusal into `None`).
pub(crate) fn from_env_with(
    lookup: impl Fn(&str) -> std::result::Result<Option<String>, Error>,
) -> Result<GatewayClient> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url).map_err(Error::from)?;
    let key = lookup("PROMPTFORGE_GATEWAY_API_KEY")?
        .map(SecretString::new)
        .and_then(std::result::Result::ok);
    match key {
        Some(key) => Ok(GatewayClient::new(endpoint, key)),
        None if endpoint.is_loopback() => Ok(GatewayClient::keyless(endpoint)),
        None => Err(Error::MissingEnv("PROMPTFORGE_GATEWAY_API_KEY".into())),
    }
}
