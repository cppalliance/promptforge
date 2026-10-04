//! The HTTP transport: the gateway client, bounded SSE response reading,
//! and environment loading.
//!
//! The request body, the stream reassembly, and the read loop that applies
//! the byte cap and measures the timing are the shared wire code in
//! `wire`; this file owns only what touches the wire: sending, the
//! per-receive timeout, the response as a chunk source, and the clock the
//! read loop is handed.

use std::fmt;
use std::num::NonZeroU64;
use std::time::{Duration, Instant};

use promptforge::model::{Completion, CompletionError, CompletionOptions, Message, ToolSchema};

use crate::config::{GatewayConfigError, GatewayEndpoint, SecretString};
use crate::failure::{elapsed, transport_failure, unavailable};
use crate::wire::classify::classify_http_failure;
use crate::wire::delta::StreamDelta;
use crate::wire::read::{ChunkSource, read_body_capped, read_completion_stream};
use crate::wire::request::build_request_body;
use crate::wire::stream::escape_controls;

/// A client that sends chat completion requests to one gateway URL.
///
/// The client usually presents the gateway's shared bearer key on every
/// request. The key is optional: by default, a gateway on the same machine
/// admits loopback callers that present no key. A client built without a
/// key ([`GatewayChat::keyless`]) omits the `Authorization` header
/// entirely.
#[derive(Clone)]
#[non_exhaustive]
pub struct GatewayChat {
    transport: GatewayTransport,
    base_url: String,
    /// The bearer presented on every request, or `None` to present nothing.
    key: Option<SecretString>,
    /// Longest wait for the response headers, and then for each next body
    /// chunk; a stream that keeps arriving is never cut off.
    request_timeout: Duration,
    /// Byte ceiling enforced on a response body before it is decoded.
    max_response_bytes: u64,
}

/// Default longest wait for the next receive, matching the executor's run
/// limits.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
/// Default response-body ceiling, matching the executor's run limits.
const DEFAULT_MAX_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
enum GatewayTransport {
    Http(reqwest::Client),
    Disabled,
}

/// A [`reqwest::Response`] body as the reassembly's chunk source, with each
/// receive bounded by the client's timeout.
struct ResponseChunks {
    response: reqwest::Response,
    timeout: Duration,
}

impl ChunkSource for ResponseChunks {
    type Chunk = bytes::Bytes;

    async fn next_chunk(&mut self) -> Result<Option<Self::Chunk>, CompletionError> {
        tokio::time::timeout(self.timeout, self.response.chunk())
            .await
            .map_err(elapsed)?
            .map_err(transport_failure)
    }
}

impl fmt::Debug for GatewayChat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The bearer key is a credential and must never appear in Debug output,
        // logs, or panic messages. It is redacted to a fixed marker regardless of
        // whether one is set, so no length or presence signal leaks either.
        f.debug_struct("GatewayChat")
            .field("base_url", &self.base_url)
            .field("key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayChat {
    /// Builds a client that sends requests to `endpoint` with `key` as the
    /// bearer key.
    ///
    /// The endpoint is a validated [`GatewayEndpoint`], and the key is a
    /// [`SecretString`], which keeps it redacted. [`GatewayChat::from_env`]
    /// builds its client this way when a key is set.
    #[must_use]
    pub fn new(endpoint: GatewayEndpoint, key: SecretString) -> GatewayChat {
        GatewayChat {
            transport: GatewayTransport::Http(reqwest::Client::new()),
            base_url: endpoint.url,
            key: Some(key),
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Builds a client that presents no bearer key.
    ///
    /// Every request goes out without an `Authorization` header. This suits
    /// a gateway on the same machine, because such a gateway trusts loopback
    /// callers that present no key, unless its operator set
    /// `trust_loopback = false`. On a shared machine, that trust also covers
    /// every other OS account there. Against any other gateway, the requests
    /// fail with an `Unavailable`-kind error when the gateway answers 401.
    ///
    /// This constructor does not check the endpoint's host, so the caller
    /// decides when a keyless client is appropriate.
    /// [`GatewayChat::from_env`] decides by [`GatewayEndpoint::is_loopback`].
    #[must_use]
    pub fn keyless(endpoint: GatewayEndpoint) -> GatewayChat {
        GatewayChat {
            transport: GatewayTransport::Http(reqwest::Client::new()),
            base_url: endpoint.url,
            key: None,
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
            max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
        }
    }

    /// Builds a client that never contacts a gateway, for execution paths
    /// that must stay hermetic.
    ///
    /// Any attempted model call fails with an `Unavailable`-kind
    /// [`CompletionError`]. The client reads no gateway configuration and
    /// sends no HTTP request.
    #[must_use]
    pub fn disabled() -> GatewayChat {
        GatewayChat {
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
    fn has_key(&self) -> bool {
        self.key.is_some()
    }

    /// Applies the run's HTTP limits to this client: a timeout for each
    /// receive and a cap on the response size.
    ///
    /// `request_timeout` is the longest a completion request waits for its
    /// response headers, and then for each next body chunk. Every chunk that
    /// arrives restarts the wait. A long stream that keeps arriving
    /// completes, and a stream that stalls fails as a timeout.
    ///
    /// The client refuses the response body as soon as it would exceed
    /// `max_response_bytes`, before any UTF-8 or JSON decoding runs.
    #[must_use]
    pub fn with_request_limits(
        mut self,
        request_timeout: Duration,
        max_response_bytes: NonZeroU64,
    ) -> GatewayChat {
        self.request_timeout = request_timeout;
        self.max_response_bytes = max_response_bytes.get();
        self
    }

    /// Builds a client from environment variables.
    ///
    /// - `PROMPTFORGE_GATEWAY_URL` holds the gateway URL. It is required.
    /// - `PROMPTFORGE_GATEWAY_API_KEY` holds the gateway's shared bearer key.
    ///   An empty value counts as unset. The key is required unless the
    ///   URL's host is loopback (`127.0.0.1`, `::1`, `localhost`). By
    ///   default, a loopback gateway trusts callers on the same machine that
    ///   present no key, so without a key the client is built keyless.
    ///
    /// That trust also admits every other OS account on a shared machine, so
    /// the gateway's operator there sets `trust_loopback = false`. Then set
    /// the key, or a keyless client's requests fail with an
    /// `Unavailable`-kind error when the gateway answers 401.
    ///
    /// # Errors
    /// Returns a [`GatewayConfigError`] when `PROMPTFORGE_GATEWAY_URL` is
    /// unset or invalid, when either variable is set to a value that is not
    /// valid Unicode, or when the URL's host is not loopback (a LAN or
    /// remote gateway) and `PROMPTFORGE_GATEWAY_API_KEY` is unset or empty.
    pub fn from_env() -> Result<GatewayChat, GatewayConfigError> {
        from_env_with(|name| match std::env::var(name) {
            Ok(value) => Ok(Some(value)),
            Err(std::env::VarError::NotPresent) => Ok(None),
            // A set-but-non-Unicode value is a real misconfiguration, surfaced
            // explicitly instead of being silently treated as "not set".
            Err(std::env::VarError::NotUnicode(_)) => {
                Err(GatewayConfigError::InvalidEnv(name.to_owned()))
            }
        })
    }

    /// Sends a list of messages to the gateway and returns the model's reply.
    ///
    /// The request always streams. It asks for server-sent events (SSE) and
    /// sets `stream_options.include_usage`, so the stream ends with a
    /// summary chunk that reports token usage. The client reassembles the
    /// streamed fragments into the body a non-streaming chat completion
    /// response would carry. It calls `on_delta` with each text or reasoning
    /// fragment, as a [`StreamDelta`], as soon as the fragment arrives. A
    /// caller with no use for the fragments passes a closure that does
    /// nothing.
    ///
    /// The returned [`Completion`] holds the reassembled turn, the metadata
    /// parsed from the stream's summary chunk, and a
    /// [`ClientTiming`](promptforge::metrics::ClientTiming) measured on this
    /// client's own clock: time to first token, mean inter-token latency,
    /// and end-to-end time.
    ///
    /// When `tools` is `Some` and non-empty, the request carries a `tools`
    /// array with one `OpenAI` function tool per schema: an object whose
    /// `type` is `function` and whose `function` holds the schema's name,
    /// description, and parameters. The request also sets `tool_choice` to
    /// `auto`. Passing `None` or an empty slice omits the `tools` field, so
    /// the request is a plain chat completion.
    ///
    /// The request names the model that `options` names, and the returned
    /// completion is labeled with that name, whatever name the response
    /// gave. The optional `temperature`, `max_tokens`, and `thinking`
    /// settings in `options` extend the request when present.
    ///
    /// # Errors
    /// Returns a [`CompletionError`] whose [`kind`](CompletionError::kind) is
    /// one of the following, and never any other kind:
    /// - `Unavailable` when this client was built with [`GatewayChat::disabled`],
    ///   or when the gateway answers 401 or 403.
    /// - `Timeout` when the response headers or the next body chunk do not
    ///   arrive within the request timeout.
    /// - `Transport` on any other transport failure, such as a failed
    ///   connection, or when the stream carries a mid-stream error envelope
    ///   that names no known cause.
    /// - `ContextOverflow`, `RateLimited`, `QuotaExhausted`, `Overloaded`,
    ///   `Refused`, `ServerError`, or `Rejected` when the gateway answers
    ///   with a non-success status, as [`classify_http_failure`] reads it.
    /// - `MalformedResponse` when the stream exceeds the response size cap,
    ///   a chunk is not usable JSON (a JSON decode error is kept as the
    ///   error's source), or the stream ends without the `[DONE]` sentinel.
    ///   It is also the kind when a `length` or `content_filter` finish
    ///   reason cuts off a batch of tool calls, because partial arguments
    ///   must not run.
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
            return Err(unavailable());
        };
        let request_body = build_request_body(messages, tools, options);

        let started = Instant::now();
        // No reqwest `.timeout`: it caps the whole request including the
        // body, which would cut off a long stream that is still arriving.
        // The timeout bounds the headers here and each receive in
        // `ResponseChunks`.
        let mut request = http
            .post(format!("{}/chat/completions", self.base_url))
            .json(&request_body);
        if let Some(key) = &self.key {
            request = request.bearer_auth(key.expose());
        }
        let response = tokio::time::timeout(self.request_timeout, request.send())
            .await
            .map_err(elapsed)?
            .map_err(transport_failure)?;

        let status = response.status();
        let content_length = response.content_length();
        let mut chunks = ResponseChunks {
            response,
            timeout: self.request_timeout,
        };
        if !status.is_success() {
            let raw_body =
                read_body_capped(&mut chunks, content_length, self.max_response_bytes).await?;
            // F5: bound the body, then escape control characters so a hostile
            // payload cannot forge log lines. The classifier keeps the escaped
            // body only as the opt-in `CompletionError::detail`, never in the
            // public `Display`.
            let body = String::from_utf8_lossy(&raw_body);
            let body = escape_controls(&body, 2000);
            return Err(classify_http_failure(status.as_u16(), &body));
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

/// The environment-driven constructor behind [`GatewayChat::from_env`],
/// with the variable lookup injected so tests need not touch the process
/// environment.
///
/// The key is optional exactly when the URL's host is loopback; an empty key
/// counts as unset ([`SecretString::new`] refuses only an empty secret, and
/// `Result::ok` folds that refusal into `None`).
fn from_env_with(
    lookup: impl Fn(&str) -> Result<Option<String>, GatewayConfigError>,
) -> Result<GatewayChat, GatewayConfigError> {
    let base_url = lookup("PROMPTFORGE_GATEWAY_URL")?
        .ok_or_else(|| GatewayConfigError::MissingEnv("PROMPTFORGE_GATEWAY_URL".into()))?;
    let endpoint = GatewayEndpoint::new(&base_url)?;
    let key = lookup("PROMPTFORGE_GATEWAY_API_KEY")?
        .map(SecretString::new)
        .and_then(Result::ok);
    match key {
        Some(key) => Ok(GatewayChat::new(endpoint, key)),
        None if endpoint.is_loopback() => Ok(GatewayChat::keyless(endpoint)),
        None => Err(GatewayConfigError::MissingEnv(
            "PROMPTFORGE_GATEWAY_API_KEY".into(),
        )),
    }
}

#[cfg(test)]
pub(crate) mod tests;
