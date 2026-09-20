//! The HTTP transport: the gateway client, bounded SSE response reading,
//! and environment loading.
//!
//! The request body, the stream reassembly, and the read loop that applies
//! the byte cap and measures the timing are the engine's shared protocol
//! seams (`promptforge_api_runtime::model`); this file owns only what
//! touches the wire: sending, the request timeout, the response as a chunk
//! source, and the clock the read loop is handed.

use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use promptforge_api_runtime::model::{
    ChunkSource, ClientError as Error, ClientTimeout, Completion, CompletionError,
    CompletionOptions, Message, StreamDelta, ToolSchema, build_request_body, escape_controls,
    read_body_capped, read_completion_stream,
};

use crate::config::{GatewayEndpoint, SecretString};

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

/// Wraps a transport-layer failure into the client substrate, marking a
/// timeout so [`CompletionError::is_timeout`] holds through the type
/// erasure.
pub(crate) fn http(error: reqwest::Error) -> Error {
    Error::Http(transport_source(error))
}

/// Boxes a transport-layer failure as an error-chain source, wrapped in
/// the vocabulary's timeout marker when it was one.
///
/// Every substrate variant that erases a `reqwest::Error` (`Http`,
/// `BackendBodyRead`) boxes it through here, so `is_timeout` holds under
/// each of them and the marker cannot be forgotten on one path.
pub(crate) fn transport_source(error: reqwest::Error) -> Box<dyn std::error::Error + Send + Sync> {
    if error.is_timeout() {
        return Box::new(ClientTimeout(Box::new(error)));
    }
    Box::new(error)
}

/// A [`reqwest::Response`] body as the reassembly's chunk source.
struct ResponseChunks(reqwest::Response);

impl ChunkSource for ResponseChunks {
    type Chunk = bytes::Bytes;

    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, CompletionError> {
        self.0
            .chunk()
            .await
            .map_err(|error| CompletionError::from(http(error)))
    }
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
    /// Builds a client from a validated [`GatewayEndpoint`] and a redacted
    /// [`SecretString`] bearer key (used by tests and by
    /// [`GatewayClient::from_env`]).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn run() -> Result<(), harness_models::CompletionError> {
    /// use harness_models::{GatewayClient, GatewayEndpoint, SecretString};
    /// use promptforge_api_runtime::model::{CompletionOptions, Message};
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

    /// Builds a client that presents no bearer key.
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
    /// use harness_models::{GatewayClient, GatewayEndpoint};
    ///
    /// let endpoint = GatewayEndpoint::new("http://127.0.0.1:8081/v1")?;
    /// let client = GatewayClient::keyless(endpoint);
    /// let _ = client;
    /// # Ok::<(), harness_models::CompletionError>(())
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

    /// Builds a client that cannot read gateway configuration or send HTTP.
    ///
    /// Hosts use this explicit sentinel for hermetic execution paths. Any
    /// attempted model call fails with a `Disabled`-kind [`CompletionError`].
    ///
    /// # Examples
    ///
    /// ```
    /// # async fn run() {
    /// use harness_models::{CompletionErrorKind, GatewayClient};
    /// use promptforge_api_runtime::model::{CompletionOptions, Message};
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
    /// use harness_models::GatewayClient;
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
    pub fn from_env() -> Result<GatewayClient, CompletionError> {
        from_env_with(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            // A set-but-non-Unicode value is a real misconfiguration, surfaced
            // explicitly instead of being silently treated as "not set".
            Err(std::env::VarError::NotUnicode(_)) => Err(Error::InvalidEnv(name.to_owned())),
        })
        .map_err(CompletionError::from)
    }

    /// Sends a list of messages and returns the model's accumulated outcome.
    ///
    /// The one completion method, always streaming: the request asks for SSE
    /// with `stream_options.include_usage`, deltas are accumulated into the
    /// buffered body shape, and `on_delta` is invoked live with each
    /// [`StreamDelta`] text or reasoning fragment (a caller with no use for
    /// deltas passes a no-op closure). The returned [`Completion`] carries
    /// the reassembled turn, the metadata parsed from the stream's summary
    /// chunk, and a [`ClientTiming`](promptforge_api_types::metrics::ClientTiming)
    /// measured on this client's own clock
    /// (TTFT, mean inter-token latency, end-to-end).
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
    ) -> Result<Completion, CompletionError> {
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
        let response = request.send().await.map_err(self::http)?;

        let status = response.status();
        let content_length = response.content_length();
        let mut chunks = ResponseChunks(response);
        if !status.is_success() {
            let raw_body =
                read_body_capped(&mut chunks, content_length, self.max_response_bytes).await?;
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

        // The byte cap, the `[DONE]` rule, the truncation rule, the strict
        // turn normalizer, and the timing arithmetic all run inside the
        // shared read loop: one rule set for every transport. This client
        // contributes the chunks and the clock.
        read_completion_stream(
            &mut chunks,
            request_body,
            self.max_response_bytes,
            on_delta,
            started,
            Instant::now,
        )
        .await
    }
}

/// The environment-driven constructor behind [`GatewayClient::from_env`],
/// with the variable lookup injected so tests need not touch the process
/// environment.
///
/// The key is optional exactly when the URL's host is loopback; an empty key
/// counts as unset ([`SecretString::new`] refuses only an empty secret, and
/// `Result::ok` folds that refusal into `None`).
pub(crate) fn from_env_with(
    lookup: impl Fn(&str) -> Result<Option<String>, Error>,
) -> Result<GatewayClient, Error> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| Error::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url).map_err(Error::from)?;
    let key = lookup("PROMPTFORGE_GATEWAY_API_KEY")?
        .map(SecretString::new)
        .and_then(Result::ok);
    match key {
        Some(key) => Ok(GatewayClient::new(endpoint, key)),
        None if endpoint.is_loopback() => Ok(GatewayClient::keyless(endpoint)),
        None => Err(Error::MissingEnv("PROMPTFORGE_GATEWAY_API_KEY".into())),
    }
}

#[cfg(test)]
pub(crate) mod tests;
