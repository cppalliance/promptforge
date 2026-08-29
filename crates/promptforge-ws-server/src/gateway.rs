//! HTTP client for the PromptForge gateway's OpenAI-compatible API.
//!
//! [`GatewayClient`] wraps `reqwest` with bearer authentication and returns
//! responses as raw bytes so the workshop routes can relay them to the
//! caller byte-for-byte. A non-success status from the gateway is *not* an
//! error here: it is part of the relayed response. Streaming chat requests
//! are decoded from SSE into a [`SsePayloadStream`] of `data:` payloads.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::pin::Pin;
use std::time::Duration;

use futures_util::stream::{self, Stream, StreamExt};
use serde::Deserialize;

use crate::protocol::ChatRequest;

/// Default bound on a single `GET /health` probe: a gateway that accepts
/// the connection but never answers must still read as unreachable, and two
/// seconds keeps the probe well under the heartbeat interval it serves.
pub(crate) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// TCP connect timeout applied to every request. A gateway that is down or
/// unreachable should fail fast rather than hanging for the OS default (~21 s
/// on Linux, ~75 s on Windows).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default whole-request timeout for non-streaming operations: model
/// catalog fetch, buffered chat completions, and the initial cache API
/// handshake. Streaming responses (SSE chat, cache downloads, and profile
/// switches) can legitimately run for minutes, so the same bound covers
/// only their header phase (see `send_bounded`) and the body stream stays
/// open-ended.
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// A gateway HTTP response captured for verbatim relay.
#[derive(Debug)]
pub struct GatewayResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// A stream of SSE `data:` payloads from the gateway, in arrival order.
///
/// Each item is one event's data, verbatim; the OpenAI terminal sentinel
/// arrives as the payload `"[DONE]"`. A transport failure mid-stream yields
/// one error item and then ends the stream.
pub type SsePayloadStream = Pin<Box<dyn Stream<Item = Result<String, GatewayError>> + Send>>;

/// The outcome of a streaming chat request to the gateway.
#[non_exhaustive]
pub enum ChatStream {
    /// The gateway accepted the stream; payloads arrive in order.
    #[non_exhaustive]
    Stream {
        /// The gateway's success status, relayed unchanged.
        status: reqwest::StatusCode,
        /// The SSE payload stream, ending with the `"[DONE]"` payload.
        payloads: SsePayloadStream,
    },

    /// The gateway answered with an ordinary (non-SSE) response, buffered
    /// for verbatim relay; this is how a declined stream reports its error
    /// envelope.
    #[non_exhaustive]
    Relay(GatewayResponse),
}

// Manual because the boxed payload stream has no `Debug` impl.
impl std::fmt::Debug for ChatStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stream { status, .. } => f
                .debug_struct("ChatStream")
                .field("status", status)
                .finish_non_exhaustive(),
            Self::Relay(response) => f.debug_tuple("Relay").field(response).finish(),
        }
    }
}

/// The gateway's answer to a cache-ensure request, `POST /v1/cache`.
///
/// The gateway answers a cache hit with a buffered JSON `ready` event and a
/// miss with an SSE stream of `downloading` progress events terminated by a
/// `ready` or `error` event; both event shapes decode as [`CacheEvent`]. A
/// non-success status (a declined or failed request) is buffered rather
/// than reported as an error, matching the relay contract of the other
/// client methods.
#[non_exhaustive]
pub enum CacheResponse {
    /// The gateway is downloading the blob; `payloads` carries the SSE
    /// stream of [`CacheEvent`] JSON documents.
    #[non_exhaustive]
    Download {
        /// The gateway's success status.
        status: reqwest::StatusCode,
        /// The SSE payload stream, ending in a terminal `ready` or `error`
        /// event.
        payloads: SsePayloadStream,
    },

    /// Any other answer, buffered: a cache hit's `ready` JSON on a success
    /// status, or the gateway's error envelope on a failure status.
    #[non_exhaustive]
    Buffered(GatewayResponse),
}

// Manual because the boxed payload stream has no `Debug` impl.
impl std::fmt::Debug for CacheResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Download { status, .. } => f
                .debug_struct("CacheResponse::Download")
                .field("status", status)
                .finish_non_exhaustive(),
            Self::Buffered(response) => f
                .debug_tuple("CacheResponse::Buffered")
                .field(response)
                .finish(),
        }
    }
}

/// One event of the gateway cache API: a download progress sample, or the
/// terminal state of a cache-ensure call.
///
/// The `path` a `Ready` event carries names a file on the gateway host, so
/// the cache API is only meaningful to a workshop sharing the gateway's
/// filesystem - the standard local deployment, where both run on loopback.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheEvent {
    /// A progress sample from a running download.
    #[non_exhaustive]
    Downloading {
        /// Cumulative bytes downloaded so far.
        bytes: u64,
        /// Total bytes expected; null when the upstream server sent no
        /// Content-Length.
        total: Option<u64>,
    },

    /// The blob is cached and ready at `path`.
    #[non_exhaustive]
    Ready {
        /// Local path of the cached blob on the gateway host.
        path: PathBuf,
    },

    /// The download failed.
    #[non_exhaustive]
    Error {
        /// The gateway's description of the failure.
        message: String,
    },
}

/// The gateway's answer to a profile switch, `POST /admin/switch-profile`.
///
/// An accepted switch answers `text/event-stream`: stage markers as the
/// switch proceeds, then exactly one terminal `ready` or `error` event, all
/// decoding as [`SwitchEvent`]. A refusal before the switch starts (bad
/// auth, a malformed name, no profiles directory) is buffered rather than
/// reported as an error, matching the relay contract of the other client
/// methods.
#[non_exhaustive]
pub enum SwitchResponse {
    /// The gateway accepted the switch and is streaming its progress.
    #[non_exhaustive]
    Switching {
        /// The gateway's success status.
        status: reqwest::StatusCode,
        /// The SSE payload stream of [`SwitchEvent`] JSON documents, ending
        /// in a terminal `ready` or `error` event.
        payloads: SsePayloadStream,
    },

    /// A refusal, buffered: the gateway's error envelope.
    #[non_exhaustive]
    Buffered(GatewayResponse),
}

// Manual because the boxed payload stream has no `Debug` impl.
impl std::fmt::Debug for SwitchResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Switching { status, .. } => f
                .debug_struct("SwitchResponse::Switching")
                .field("status", status)
                .finish_non_exhaustive(),
            Self::Buffered(response) => f
                .debug_tuple("SwitchResponse::Buffered")
                .field(response)
                .finish(),
        }
    }
}

/// One event of the gateway's switch-profile stream: a stage marker as the
/// switch proceeds, then exactly one terminal event.
///
/// Stage markers arrive in execution order - `loading-profile`,
/// `stopping-models`, `starting-models` (the long pole: weights loading
/// into VRAM). The stage stays a string so a gateway that grows a new
/// stage never breaks the decode.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
#[non_exhaustive]
pub enum SwitchEvent {
    /// A phase of the switch is beginning.
    #[non_exhaustive]
    Stage {
        /// The gateway's name for the phase, e.g. `starting-models`.
        stage: String,
    },

    /// Terminal: the switch committed and `profile` is live.
    #[non_exhaustive]
    Ready {
        /// The now-active profile name.
        profile: String,
    },

    /// Terminal: the switch failed. The previous profile stays
    /// authenticated and remote-routable, but its local children may
    /// already be gone (the gateway's documented degraded state).
    #[non_exhaustive]
    Error {
        /// The gateway's description of the failure.
        message: String,
    },
}

/// A stream of decoded [`SwitchEvent`]s, as produced by [`switch_events`].
pub type SwitchEventStream = Pin<Box<dyn Stream<Item = Result<SwitchEvent, GatewayError>> + Send>>;

/// Decodes a switch-profile payload stream into typed [`SwitchEvent`]s.
///
/// A payload that does not parse as a switch event is logged and skipped -
/// a malformed line from the gateway degrades one progress update, never
/// the switch - and the stream continues to its terminal event. A transport
/// failure passes through and ends the stream.
#[must_use]
pub fn switch_events(payloads: SsePayloadStream) -> SwitchEventStream {
    Box::pin(payloads.filter_map(|item| async move {
        match item {
            Ok(payload) => match serde_json::from_str::<SwitchEvent>(&payload) {
                Ok(event) => Some(Ok(event)),
                Err(error) => {
                    tracing::warn!(%error, payload, "skipping a malformed switch-profile event");
                    None
                }
            },
            Err(error) => Some(Err(error)),
        }
    }))
}

/// A gateway request failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayError {
    /// The HTTP client could not be built.
    #[non_exhaustive]
    #[error("build gateway http client")]
    Build(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The request could not be sent or no response arrived (connect
    /// refused, DNS, TLS, timeout).
    #[non_exhaustive]
    #[error("gateway transport error")]
    Transport(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The response body could not be read to completion.
    #[non_exhaustive]
    #[error("read gateway response body")]
    ReadBody(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The chat request could not be serialized to JSON.
    #[non_exhaustive]
    #[error("serialize chat request")]
    Serialize(#[source] Box<dyn std::error::Error + Send + Sync>),
}

/// Bearer-authenticated client for the gateway's OpenAI-compatible
/// endpoints. An empty API key sends no `Authorization` header at all, for
/// gateways running with authentication disabled.
#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    base_url: String,
    api_key: String,
    /// Whole-request bound for buffered calls; header-phase bound for
    /// streaming calls.
    request_timeout: Duration,
    /// Whole-request bound for the health probe.
    health_timeout: Duration,
}

// Manual so the bearer key is never written to logs.
impl std::fmt::Debug for GatewayClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GatewayClient")
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish_non_exhaustive()
    }
}

impl GatewayClient {
    /// Builds a client for `base_url` authenticating with `api_key`.
    ///
    /// A trailing slash on `base_url` is trimmed so route joins stay clean.
    /// An empty `api_key` disables authentication: requests then carry no
    /// `Authorization` header.
    ///
    /// # Errors
    /// Returns [`GatewayError::Build`] if the TLS backend cannot initialize.
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, GatewayError> {
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .build()
            .map_err(|source| GatewayError::Build(Box::new(source)))?;
        Ok(Self {
            http,
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            request_timeout: REQUEST_TIMEOUT,
            health_timeout: HEALTH_PROBE_TIMEOUT,
        })
    }

    /// Overrides the request and probe bounds, so tests can trip them
    /// without waiting out the production values.
    #[cfg(test)]
    pub(crate) fn with_timeouts_for_test(mut self, request: Duration, health: Duration) -> Self {
        self.request_timeout = request;
        self.health_timeout = health;
        self
    }

    /// Sends `request`, bounding the wait for the response headers on the
    /// client's request timeout.
    ///
    /// The streaming calls use this instead of a whole-request timeout: a
    /// gateway that accepts the connection and then stalls must fail the
    /// call rather than hang its caller, but an accepted stream may
    /// legitimately run for minutes, so only the header phase is bounded
    /// and the body stream stays open-ended.
    async fn send_bounded(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, GatewayError> {
        match tokio::time::timeout(self.request_timeout, request.send()).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(source)) => Err(GatewayError::Transport(Box::new(source))),
            Err(elapsed) => Err(GatewayError::Transport(Box::new(elapsed))),
        }
    }

    /// Applies bearer authentication to `request`, unless the client was
    /// built with an empty API key, in which case the request goes out with
    /// no `Authorization` header.
    fn authorize(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if self.api_key.is_empty() {
            request
        } else {
            request.bearer_auth(&self.api_key)
        }
    }

    /// Probes the gateway's liveness endpoint, `GET /health`.
    ///
    /// Returns `true` only when the gateway answers with a success status:
    /// a transport failure, a probe timeout, or a non-success answer all
    /// read as unreachable. The request never carries the client's API key
    /// (the endpoint is unauthenticated by design) and is capped at the
    /// probe bound (`HEALTH_PROBE_TIMEOUT` by default).
    pub async fn health(&self) -> bool {
        let probe = self
            .http
            .get(format!("{}/health", self.base_url))
            .timeout(self.health_timeout);
        match probe.send().await {
            Ok(response) => response.status().is_success(),
            Err(_) => false,
        }
    }

    /// Fetches the gateway's model catalog from `GET /v1/models`.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn list_models(&self) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .authorize(self.http.get(format!("{}/v1/models", self.base_url)))
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Fetches the gateway's profile list from `GET /admin/profiles`.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn list_profiles(&self) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .authorize(self.http.get(format!("{}/admin/profiles", self.base_url)))
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Fetches the gateway's live status from `GET /admin/status`, which
    /// carries the active profile's name.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn profile_status(&self) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .authorize(self.http.get(format!("{}/admin/status", self.base_url)))
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Posts a profile switch to `POST /admin/switch-profile`.
    ///
    /// An accepted switch answers `text/event-stream` and returns
    /// [`SwitchResponse::Switching`], whose payload stream carries stage
    /// markers and then a terminal `ready` or `error` event (decode it with
    /// [`switch_events`]). Only the wait for the response headers is
    /// bounded: loading model weights into VRAM legitimately runs for
    /// minutes and the stream reports progress the whole way, so the
    /// stream itself carries no deadline. A non-success or non-streaming
    /// answer is buffered and returned, not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed (the header bound elapsing included) and
    /// [`GatewayError::ReadBody`] if a buffered answer's body cannot be
    /// read.
    pub async fn switch_profile(&self, name: &str) -> Result<SwitchResponse, GatewayError> {
        let request = self
            .authorize(
                self.http
                    .post(format!("{}/admin/switch-profile", self.base_url)),
            )
            .json(&serde_json::json!({ "name": name }));
        let response = self.send_bounded(request).await?;
        let status = response.status();
        if status.is_success() && is_event_stream(&response) {
            return Ok(SwitchResponse::Switching {
                status,
                payloads: payload_stream(response),
            });
        }
        read(response).await.map(SwitchResponse::Buffered)
    }

    /// Posts a non-streaming chat completion to
    /// `POST /v1/chat/completions`.
    ///
    /// A non-success status is relayed in the returned
    /// [`GatewayResponse`], not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed and [`GatewayError::ReadBody`] if the response body cannot
    /// be read.
    pub async fn chat_completion(
        &self,
        request: &ChatRequest,
    ) -> Result<GatewayResponse, GatewayError> {
        let response = self
            .authorize(
                self.http
                    .post(format!("{}/v1/chat/completions", self.base_url)),
            )
            .json(request)
            .timeout(self.request_timeout)
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        read(response).await
    }

    /// Posts a streaming chat completion to `POST /v1/chat/completions`
    /// with `"stream": true` added to the request body.
    ///
    /// A success status yields [`ChatStream::Stream`] carrying the SSE
    /// payload stream; any other status is buffered and returned as
    /// [`ChatStream::Relay`] so the caller can relay the gateway's error
    /// envelope verbatim. Only the wait for the response headers is
    /// bounded; the accepted stream itself carries no deadline.
    ///
    /// # Errors
    /// Returns [`GatewayError::Serialize`] if the request cannot be
    /// serialized, [`GatewayError::Transport`] if the request cannot be
    /// completed (the header bound elapsing included), and
    /// [`GatewayError::ReadBody`] if a declined stream's error body cannot
    /// be read.
    pub async fn chat_completion_stream(
        &self,
        request: &ChatRequest,
    ) -> Result<ChatStream, GatewayError> {
        let mut body = serde_json::to_value(request)
            .map_err(|source| GatewayError::Serialize(Box::new(source)))?;
        if let Some(object) = body.as_object_mut() {
            object.insert("stream".to_string(), serde_json::Value::Bool(true));
        }
        let request = self
            .authorize(
                self.http
                    .post(format!("{}/v1/chat/completions", self.base_url)),
            )
            .json(&body);
        let response = self.send_bounded(request).await?;
        let status = response.status();
        if !status.is_success() {
            return read(response).await.map(ChatStream::Relay);
        }
        Ok(ChatStream::Stream {
            status,
            payloads: payload_stream(response),
        })
    }

    /// Posts a cache-ensure request to `POST /v1/cache`, asking the gateway
    /// to make the blob at `source` available locally.
    ///
    /// A cache hit answers a buffered JSON `ready` event
    /// ([`CacheResponse::Buffered`] on a success status); a miss answers
    /// `text/event-stream` and returns [`CacheResponse::Download`], whose
    /// payload stream ends in a terminal `ready` or `error` event. Only
    /// the wait for the response headers is bounded; a download stream
    /// itself carries no deadline. A non-success status is buffered and
    /// returned, not reported as an error.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed (the header bound elapsing included) and
    /// [`GatewayError::ReadBody`] if a buffered answer's body cannot be
    /// read.
    pub async fn cache_ensure(&self, source: &str) -> Result<CacheResponse, GatewayError> {
        let request = self
            .authorize(self.http.post(format!("{}/v1/cache", self.base_url)))
            .json(&serde_json::json!({ "source": source }));
        let response = self.send_bounded(request).await?;
        let status = response.status();
        if status.is_success() && is_event_stream(&response) {
            return Ok(CacheResponse::Download {
                status,
                payloads: payload_stream(response),
            });
        }
        read(response).await.map(CacheResponse::Buffered)
    }
}

/// Whether the gateway answered with an SSE body.
fn is_event_stream(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("text/event-stream"))
}

/// Captures the status and raw body of a gateway response.
async fn read(response: reqwest::Response) -> Result<GatewayResponse, GatewayError> {
    let status = response.status();
    let body = response
        .bytes()
        .await
        .map_err(|source| GatewayError::ReadBody(Box::new(source)))?
        .to_vec();
    Ok(GatewayResponse { status, body })
}

/// Decodes a gateway SSE response body into its `data:` payload stream.
///
/// Byte chunks arrive on arbitrary TCP boundaries, so the decoder buffers
/// partial lines; a mid-stream transport failure surfaces as one error item
/// that ends the stream.
fn payload_stream(response: reqwest::Response) -> SsePayloadStream {
    let state = (response.bytes_stream(), SseDecoder::default(), false);
    let payloads = stream::try_unfold(state, |(mut bytes, mut decoder, mut eof)| async move {
        loop {
            if let Some(payload) = decoder.pop() {
                return Ok(Some((payload, (bytes, decoder, eof))));
            }
            if eof {
                return Ok(None);
            }
            match bytes.next().await {
                Some(Ok(chunk)) => decoder.feed(&chunk),
                Some(Err(source)) => return Err(GatewayError::ReadBody(Box::new(source))),
                None => {
                    decoder.finish();
                    eof = true;
                }
            }
        }
    });
    Box::pin(payloads)
}

/// Incremental SSE decoder: turns arbitrary byte chunks into `data:`
/// payloads, one per event, in arrival order.
///
/// Only `data:` fields are collected; `event:`, `id:`, `retry:`, and
/// comments are dropped, matching what an OpenAI-compatible stream carries.
/// Multiple `data:` lines in one event are joined with `\n` per the SSE
/// specification.
#[derive(Debug, Default)]
struct SseDecoder {
    /// Bytes received but not yet terminated by `\n`.
    partial: Vec<u8>,
    /// Joined `data:` lines of the event currently being accumulated.
    data: String,
    /// Whether the current event carries at least one `data:` line.
    has_data: bool,
    /// Completed payloads awaiting pickup.
    out: VecDeque<String>,
}

impl SseDecoder {
    /// Feeds one byte chunk, completing every event it terminates.
    fn feed(&mut self, chunk: &[u8]) {
        self.partial.extend_from_slice(chunk);
        while let Some(end) = self.partial.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.partial.drain(..=end).collect();
            self.line(&line[..line.len() - 1]);
        }
    }

    /// Flushes a trailing unterminated line and any pending event at EOF.
    fn finish(&mut self) {
        if !self.partial.is_empty() {
            let line = std::mem::take(&mut self.partial);
            self.line(&line);
        }
        self.dispatch();
    }

    /// Takes the oldest completed payload, if any.
    fn pop(&mut self) -> Option<String> {
        self.out.pop_front()
    }

    /// Handles one line without its `\n`; a blank line ends the event.
    fn line(&mut self, raw: &[u8]) {
        let line = raw.strip_suffix(b"\r").unwrap_or(raw);
        if line.is_empty() {
            self.dispatch();
            return;
        }
        if let Some(value) = line.strip_prefix(b"data:") {
            let value = value.strip_prefix(b" ").unwrap_or(value);
            if self.has_data {
                self.data.push('\n');
            }
            self.data.push_str(&String::from_utf8_lossy(value));
            self.has_data = true;
        }
    }

    /// Queues the accumulated event, dropping events with no `data:` line.
    fn dispatch(&mut self) {
        if self.has_data {
            self.out.push_back(std::mem::take(&mut self.data));
            self.has_data = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::response::IntoResponse;

    use crate::backoff::xorshift;

    #[test]
    fn trailing_slash_is_trimmed_from_base_url() {
        let client = GatewayClient::new("http://127.0.0.1:8081/", "k").expect("client builds");
        assert_eq!(client.base_url, "http://127.0.0.1:8081");
    }

    #[test]
    fn debug_redacts_the_api_key() {
        let client = GatewayClient::new("http://127.0.0.1:8081", "secret-key").expect("client");
        let rendered = format!("{client:?}");
        assert!(!rendered.contains("secret-key"), "key leaked: {rendered}");
    }

    fn drain(decoder: &mut SseDecoder) -> Vec<String> {
        let mut payloads = Vec::new();
        while let Some(payload) = decoder.pop() {
            payloads.push(payload);
        }
        payloads
    }

    #[test]
    fn decoder_emits_the_same_events_regardless_of_chunk_boundaries() {
        let wire = "data: {\"a\":1}\n\ndata: [DONE]\n\n";
        let mut whole = SseDecoder::default();
        whole.feed(wire.as_bytes());
        whole.finish();

        let mut drip = SseDecoder::default();
        for byte in wire.as_bytes() {
            drip.feed(std::slice::from_ref(byte));
        }
        drip.finish();

        let whole_out = drain(&mut whole);
        assert_eq!(drain(&mut drip), whole_out, "chunking must not matter");
        assert_eq!(
            whole_out,
            ["{\"a\":1}".to_string(), "[DONE]".to_string()],
            "payloads arrive verbatim, including the terminal sentinel"
        );
    }

    #[test]
    fn decoder_joins_multi_line_data_and_ignores_other_fields() {
        let wire = ": comment\nevent: message\nid: 7\ndata: first\ndata: second\n\n";
        let mut decoder = SseDecoder::default();
        decoder.feed(wire.as_bytes());
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("first\nsecond"));
        assert!(decoder.pop().is_none(), "one event, one payload");
    }

    #[test]
    fn decoder_accepts_crlf_line_endings() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"data: one\r\n\r\n");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("one"));
    }

    #[test]
    fn decoder_flushes_an_unterminated_final_line_at_eof() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"data: tail");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("tail"));
    }

    #[test]
    fn decoder_drops_events_without_data() {
        let mut decoder = SseDecoder::default();
        decoder.feed(b"event: ping\n\ndata: kept\n\n");
        decoder.finish();
        assert_eq!(decoder.pop().as_deref(), Some("kept"));
        assert!(decoder.pop().is_none());
    }

    /// A realistic delta stream whose payloads carry multi-byte UTF-8
    /// (2-, 3-, and 4-byte codepoints), so a byte split can land inside a
    /// codepoint; mixed CRLF/LF endings and a multi-line event ride along.
    const MULTIBYTE_WIRE: &str = concat!(
        "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"h\u{e9}llo \u{1f914} w\u{f6}rld\"}}]}\r\n\r\n",
        "data: {\"choices\":[{\"delta\":{\"content\":\"\u{65e5}\u{672c}\u{8a9e}\"}}]}\n\n",
        "data: first\ndata: \u{1f40d} second\n\n",
        "data: [DONE]\n\n",
    );

    /// The payloads [`MULTIBYTE_WIRE`] must always decode to, regardless
    /// of how the bytes are chunked.
    fn multibyte_payloads() -> Vec<String> {
        vec![
            "{\"choices\":[{\"delta\":{\"reasoning_content\":\"h\u{e9}llo \u{1f914} w\u{f6}rld\"}}]}".to_string(),
            "{\"choices\":[{\"delta\":{\"content\":\"\u{65e5}\u{672c}\u{8a9e}\"}}]}".to_string(),
            "first\n\u{1f40d} second".to_string(),
            "[DONE]".to_string(),
        ]
    }

    /// Feeds `wire` split at the ascending byte offsets in `cuts`, then
    /// returns everything the decoder produced.
    fn decode_in_fragments(wire: &[u8], cuts: &[usize]) -> Vec<String> {
        let mut decoder = SseDecoder::default();
        let mut start = 0;
        for &cut in cuts {
            decoder.feed(&wire[start..cut]);
            start = cut;
        }
        decoder.feed(&wire[start..]);
        decoder.finish();
        drain(&mut decoder)
    }

    #[test]
    fn decoder_survives_every_split_point_including_mid_utf8() {
        let wire = MULTIBYTE_WIRE.as_bytes();
        let expected = multibyte_payloads();
        assert_eq!(decode_in_fragments(wire, &[]), expected, "unsplit feed");
        // Every split point, so every mid-codepoint boundary is exercised.
        for cut in 1..wire.len() {
            assert_eq!(
                decode_in_fragments(wire, &[cut]),
                expected,
                "split at byte {cut} changed the decode"
            );
        }
    }

    #[test]
    fn decoder_is_chunking_invariant_under_random_splits() {
        let wire = MULTIBYTE_WIRE.as_bytes();
        let expected = multibyte_payloads();
        for seed in [0x9E37_79B9_7F4A_7C15_u64, 42, 7_777_777] {
            let mut state = seed;
            let cuts: Vec<usize> = (1..wire.len())
                .filter(|_| xorshift(&mut state).is_multiple_of(4))
                .collect();
            assert_eq!(
                decode_in_fragments(wire, &cuts),
                expected,
                "seed {seed}: fragmenting at {cuts:?} changed the decode"
            );
        }
    }

    #[test]
    fn cache_event_decodes_each_wire_shape() {
        let downloading: CacheEvent =
            serde_json::from_str(r#"{"status":"downloading","bytes":5,"total":10}"#)
                .expect("downloading decodes");
        assert_eq!(
            downloading,
            CacheEvent::Downloading {
                bytes: 5,
                total: Some(10)
            }
        );
        let unknown_total: CacheEvent =
            serde_json::from_str(r#"{"status":"downloading","bytes":5,"total":null}"#)
                .expect("a null total decodes");
        assert_eq!(
            unknown_total,
            CacheEvent::Downloading {
                bytes: 5,
                total: None
            }
        );
        let ready: CacheEvent =
            serde_json::from_str(r#"{"status":"ready","path":"/cache/ggml.bin"}"#)
                .expect("ready decodes");
        assert_eq!(
            ready,
            CacheEvent::Ready {
                path: PathBuf::from("/cache/ggml.bin")
            }
        );
        let error: CacheEvent =
            serde_json::from_str(r#"{"status":"error","message":"boom"}"#).expect("error decodes");
        assert_eq!(
            error,
            CacheEvent::Error {
                message: "boom".to_string()
            }
        );
    }

    /// Mock cache route state: the last request's auth header and body,
    /// captured so tests can assert what the client sent.
    #[derive(Clone, Default)]
    struct CacheProbe {
        authorized: std::sync::Arc<std::sync::atomic::AtomicBool>,
        sources: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    }

    impl CacheProbe {
        fn sources(&self) -> Vec<String> {
            self.sources
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    /// Binds `app` on a free loopback port and returns its base URL.
    async fn serve(app: axum::Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock gateway");
        let addr = listener.local_addr().expect("mock gateway address");
        tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("mock gateway serves");
        });
        format!("http://{addr}")
    }

    /// Binds a stub that completes TCP handshakes and then never answers,
    /// modeling a gateway that is up but wedged.
    async fn spawn_stalled_gateway() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind stalled stub");
        let addr = listener.local_addr().expect("stalled stub address");
        tokio::spawn(async move {
            // Sockets are held open and never answered until the test's
            // runtime tears the task down.
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                held.push(socket);
            }
        });
        format!("http://{addr}")
    }

    /// A client against `base_url` whose bounds are tight enough to trip
    /// inside a test.
    fn impatient_client(base_url: &str) -> GatewayClient {
        GatewayClient::new(base_url, "")
            .expect("client builds in tests")
            .with_timeouts_for_test(Duration::from_millis(100), Duration::from_millis(100))
    }

    #[tokio::test]
    async fn a_stalled_gateway_trips_the_request_timeout() {
        let base_url = spawn_stalled_gateway().await;
        let error = impatient_client(&base_url)
            .list_models()
            .await
            .expect_err("a gateway that never answers must trip the request timeout");
        assert!(
            matches!(error, GatewayError::Transport(_)),
            "expected Transport, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_stalled_gateway_trips_the_buffered_chat_timeout() {
        let base_url = spawn_stalled_gateway().await;
        let request = ChatRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
            stream: false,
            rest: serde_json::Map::new(),
        };
        let error = impatient_client(&base_url)
            .chat_completion(&request)
            .await
            .expect_err("a gateway that never answers must trip the request timeout");
        assert!(
            matches!(error, GatewayError::Transport(_)),
            "expected Transport, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_stalled_gateway_trips_the_stream_header_bound() {
        let base_url = spawn_stalled_gateway().await;
        let request = ChatRequest {
            model: "test-model".to_string(),
            messages: vec![serde_json::json!({"role": "user", "content": "hi"})],
            stream: false,
            rest: serde_json::Map::new(),
        };
        let error = impatient_client(&base_url)
            .chat_completion_stream(&request)
            .await
            .expect_err("a gateway that never sends headers must trip the header bound");
        assert!(
            matches!(error, GatewayError::Transport(_)),
            "expected Transport, got {error:?}"
        );
    }

    #[tokio::test]
    async fn a_stalled_gateway_probe_reads_unreachable() {
        let base_url = spawn_stalled_gateway().await;
        assert!(
            !impatient_client(&base_url).health().await,
            "a gateway that accepts but never answers must read unreachable"
        );
    }

    #[tokio::test]
    async fn a_cache_hit_answers_a_buffered_ready_event() {
        let probe = CacheProbe::default();
        let seen = probe.clone();
        let app = axum::Router::new().route(
            "/v1/cache",
            axum::routing::post(
                move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                    let seen = seen.clone();
                    async move {
                        seen.authorized.store(
                            headers
                                .get(axum::http::header::AUTHORIZATION)
                                .and_then(|value| value.to_str().ok())
                                == Some("Bearer test-key"),
                            std::sync::atomic::Ordering::Relaxed,
                        );
                        seen.sources
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .push(
                                body["source"]
                                    .as_str()
                                    .expect("source is a string")
                                    .to_string(),
                            );
                        axum::Json(serde_json::json!({
                            "path": "/cache/ggml-large-v3-turbo.bin",
                            "status": "ready",
                        }))
                        .into_response()
                    }
                },
            ),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "test-key").expect("client builds in tests");
        let response = client
            .cache_ensure("https://example.com/models/ggml-large-v3-turbo.bin")
            .await
            .expect("the request completes");
        let CacheResponse::Buffered(answer) = response else {
            panic!("a cache hit is buffered, got {response:?}");
        };
        assert!(answer.status.is_success());
        let event: CacheEvent =
            serde_json::from_slice(&answer.body).expect("the hit body is a ready event");
        assert_eq!(
            event,
            CacheEvent::Ready {
                path: PathBuf::from("/cache/ggml-large-v3-turbo.bin")
            }
        );
        assert!(probe.authorized.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(
            probe.sources(),
            ["https://example.com/models/ggml-large-v3-turbo.bin"]
        );
    }

    #[tokio::test]
    async fn a_cache_miss_answers_a_download_stream() {
        let app = axum::Router::new().route(
            "/v1/cache",
            axum::routing::post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"status\":\"downloading\",\"bytes\":5,\"total\":null}\n\n",
                        "data: {\"status\":\"downloading\",\"bytes\":10,\"total\":12}\n\n",
                        "data: {\"status\":\"ready\",\"path\":\"/cache/ggml.bin\"}\n\n",
                    ),
                )
            }),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let response = client
            .cache_ensure("https://example.com/models/ggml.bin")
            .await
            .expect("the request completes");
        let CacheResponse::Download { mut payloads, .. } = response else {
            panic!("a cache miss streams, got {response:?}");
        };
        let mut events = Vec::new();
        while let Some(item) = payloads.next().await {
            let payload = item.expect("the stream is clean");
            events.push(
                serde_json::from_str::<CacheEvent>(&payload)
                    .expect("each payload is a cache event"),
            );
        }
        assert_eq!(
            events,
            [
                CacheEvent::Downloading {
                    bytes: 5,
                    total: None
                },
                CacheEvent::Downloading {
                    bytes: 10,
                    total: Some(12)
                },
                CacheEvent::Ready {
                    path: PathBuf::from("/cache/ggml.bin")
                },
            ],
            "the stream carries progress samples then the terminal ready"
        );
    }

    #[test]
    fn switch_event_decodes_each_wire_shape() {
        let stage: SwitchEvent =
            serde_json::from_str(r#"{"stage":"stopping-models"}"#).expect("a stage marker decodes");
        assert_eq!(
            stage,
            SwitchEvent::Stage {
                stage: "stopping-models".to_string()
            }
        );
        let ready: SwitchEvent =
            serde_json::from_str(r#"{"status":"ready","profile":"beta"}"#).expect("ready decodes");
        assert_eq!(
            ready,
            SwitchEvent::Ready {
                profile: "beta".to_string()
            }
        );
        let error: SwitchEvent =
            serde_json::from_str(r#"{"status":"error","message":"boom"}"#).expect("error decodes");
        assert_eq!(
            error,
            SwitchEvent::Error {
                message: "boom".to_string()
            }
        );
    }

    /// Collects the typed events of a switch stream, panicking on a
    /// transport error item.
    async fn collect_switch_events(payloads: SsePayloadStream) -> Vec<SwitchEvent> {
        let mut events = Vec::new();
        let mut typed = switch_events(payloads);
        while let Some(item) = typed.next().await {
            events.push(item.expect("the stream is clean"));
        }
        events
    }

    #[tokio::test]
    async fn an_accepted_switch_streams_stages_then_the_terminal_event() {
        let app = axum::Router::new().route(
            "/admin/switch-profile",
            axum::routing::post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"stage\":\"loading-profile\"}\n\n",
                        "data: {\"stage\":\"stopping-models\"}\n\n",
                        "data: {\"stage\":\"starting-models\"}\n\n",
                        "data: {\"status\":\"ready\",\"profile\":\"beta\"}\n\n",
                    ),
                )
            }),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let response = client
            .switch_profile("beta")
            .await
            .expect("the request completes");
        let SwitchResponse::Switching { payloads, .. } = response else {
            panic!("an accepted switch streams, got {response:?}");
        };
        assert_eq!(
            collect_switch_events(payloads).await,
            [
                SwitchEvent::Stage {
                    stage: "loading-profile".to_string()
                },
                SwitchEvent::Stage {
                    stage: "stopping-models".to_string()
                },
                SwitchEvent::Stage {
                    stage: "starting-models".to_string()
                },
                SwitchEvent::Ready {
                    profile: "beta".to_string()
                },
            ],
            "the stream carries stage markers in order then the terminal ready"
        );
    }

    #[tokio::test]
    async fn a_malformed_switch_event_is_skipped_and_the_stream_continues() {
        let app = axum::Router::new().route(
            "/admin/switch-profile",
            axum::routing::post(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    concat!(
                        "data: {\"stage\":\"loading-profile\"}\n\n",
                        "data: this is not json\n\n",
                        "data: {\"unrelated\":true}\n\n",
                        "data: {\"status\":\"error\",\"message\":\"start-local failed\"}\n\n",
                    ),
                )
            }),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let response = client
            .switch_profile("beta")
            .await
            .expect("the request completes");
        let SwitchResponse::Switching { payloads, .. } = response else {
            panic!("an accepted switch streams, got {response:?}");
        };
        assert_eq!(
            collect_switch_events(payloads).await,
            [
                SwitchEvent::Stage {
                    stage: "loading-profile".to_string()
                },
                SwitchEvent::Error {
                    message: "start-local failed".to_string()
                },
            ],
            "malformed payloads are skipped; the terminal event still arrives"
        );
    }

    #[tokio::test]
    async fn a_declined_switch_is_buffered_not_an_error() {
        let app = axum::Router::new().route(
            "/admin/switch-profile",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "error": {"message": "bad name", "code": "switch_failed"}
                    })),
                )
            }),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let response = client
            .switch_profile("../escape")
            .await
            .expect("a declined request still completes");
        let SwitchResponse::Buffered(answer) = response else {
            panic!("a declined switch is buffered, got {response:?}");
        };
        assert_eq!(answer.status, reqwest::StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn a_declined_cache_request_is_buffered_not_an_error() {
        let app = axum::Router::new().route(
            "/v1/cache",
            axum::routing::post(|| async {
                (
                    axum::http::StatusCode::BAD_REQUEST,
                    axum::Json(serde_json::json!({
                        "error": {"message": "bad source", "code": "malformed_request"}
                    })),
                )
            }),
        );
        let base_url = serve(app).await;
        let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
        let response = client
            .cache_ensure("not-a-url")
            .await
            .expect("a declined request still completes");
        let CacheResponse::Buffered(answer) = response else {
            panic!("a declined request is buffered, got {response:?}");
        };
        assert_eq!(answer.status, reqwest::StatusCode::BAD_REQUEST);
    }
}
