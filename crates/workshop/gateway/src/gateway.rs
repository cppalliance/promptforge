//! HTTP client for the PromptForge gateway's OpenAI-compatible API.
//!
//! [`GatewayClient`] wraps `reqwest` with bearer authentication and returns
//! responses as raw bytes so the workshop routes can relay them to the
//! caller byte-for-byte. A non-success status from the gateway is *not* an
//! error here: it is part of the relayed response. A cache download is
//! decoded from SSE into a [`SsePayloadStream`] of `data:` payloads, and
//! the progress subscription into a [`ProgressStream`] of decoded
//! snapshots.

use std::time::Duration;

mod events;
pub(crate) mod progress;
mod sse;

pub mod socket;

pub use events::{
    CacheEvent, CacheResponse, ForwardedResponse, GatewayResponse, SsePayloadStream,
    SwitchProfileBody, SwitchResponse,
};
pub use progress::ProgressStream;
pub use socket::GatewayRealtimeSocket;
use sse::{is_event_stream, payload_stream, read};

/// Default bound on a single `GET /health` probe: a gateway that accepts
/// the connection but never answers must still read as unreachable, and two
/// seconds keeps the probe well under the heartbeat interval it serves.
pub(crate) const HEALTH_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// TCP connect timeout applied to every request. A gateway that is down or
/// unreachable should fail fast rather than hanging for the OS default (~21 s
/// on Linux, ~75 s on Windows).
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Default whole-request timeout for non-streaming operations: the model
/// catalog fetch, the profile selection, and the initial cache API
/// handshake. Streaming responses (cache downloads and the progress
/// subscription) can legitimately run for minutes, so the same bound
/// covers only their header phase (see `send_bounded`) and the body
/// stream stays open-ended.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

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

    /// The gateway answered a streaming-only request with a non-success
    /// status (for example 401 on a rejected token). The body is bounded
    /// and control-escaped.
    #[non_exhaustive]
    #[error("gateway answered status {status}: {body}")]
    Status {
        /// The gateway's status code.
        status: reqwest::StatusCode,
        /// The gateway's error body, bounded and control-escaped.
        body: String,
    },

    /// A gateway event stream contained a block that could not be decoded,
    /// one that grew past its size bound without terminating, or a
    /// success answer whose body was not the documented JSON shape.
    #[non_exhaustive]
    #[error("malformed gateway answer: {message}")]
    Malformed {
        /// What was wrong with the block or body.
        message: String,
        /// The decode failure, when the block or body was undecodable.
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
}

impl GatewayError {
    /// A transport failure manufactured by a test, for the server's
    /// error-mapping fixtures.
    #[cfg(feature = "test-fixtures")]
    #[must_use]
    pub fn transport_for_test(source: Box<dyn std::error::Error + Send + Sync>) -> Self {
        Self::Transport(source)
    }
}

/// Bearer-authenticated client for the gateway's OpenAI-compatible
/// endpoints. An empty API key omits the `Authorization` header entirely,
/// for gateways running with authentication disabled.
#[derive(Clone)]
pub struct GatewayClient {
    http: reqwest::Client,
    pub(crate) base_url: String,
    pub(crate) api_key: String,
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
    /// An empty `api_key` disables authentication: requests then omit the
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

    /// The gateway's base URL as configured, trailing slash trimmed -
    /// also the origin the config-panel iframe loads from.
    #[must_use]
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Forwards one request to the gateway: `method` on
    /// `path_and_query`, with an optional JSON `body`, authenticated
    /// with the client's bearer key. Only the wait for the response
    /// headers is bounded - a forwarded cache download or config apply
    /// legitimately runs for minutes - and the whole body is buffered
    /// for relay. A non-success status is relayed in the returned
    /// [`ForwardedResponse`].
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed (the header bound elapsing included) and
    /// [`GatewayError::ReadBody`] if the response body cannot be read.
    pub async fn forward(
        &self,
        method: reqwest::Method,
        path_and_query: &str,
        body: Option<Vec<u8>>,
    ) -> Result<ForwardedResponse, GatewayError> {
        let mut request = self.authorize(
            self.http
                .request(method, format!("{}{}", self.base_url, path_and_query)),
        );
        if let Some(bytes) = body {
            request = request
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(bytes);
        }
        let response = self.send_bounded(request).await?;
        let status = response.status();
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body = response
            .bytes()
            .await
            .map_err(|source| GatewayError::ReadBody(Box::new(source)))?
            .to_vec();
        Ok(ForwardedResponse {
            status,
            content_type,
            body,
        })
    }

    /// Probes the gateway's liveness endpoint, `GET /health`.
    ///
    /// Returns `true` only when the gateway answers with a success status:
    /// a transport failure, a probe timeout, or a non-success answer all
    /// read as unreachable. The request never includes the client's API key
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
    /// A non-success status is relayed in the returned [`GatewayResponse`].
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
    /// A non-success status is relayed in the returned [`GatewayResponse`].
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
    /// reports the active profile's name.
    ///
    /// A non-success status is relayed in the returned [`GatewayResponse`].
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

    /// Posts a profile selection to `POST /admin/switch-profile`: `name`
    /// selects a defined profile, `None` selects no profile (`null` on
    /// the wire).
    ///
    /// The gateway persists the selection and answers one JSON document,
    /// returned as [`SwitchResponse::Selected`], reporting whether it
    /// must restart to serve the selection; the caller decides how that
    /// restart happens. A non-success answer is buffered and returned.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed, [`GatewayError::ReadBody`] if the answer's body cannot
    /// be read, and [`GatewayError::Malformed`] when a success answer is
    /// not the documented outcome shape.
    pub async fn switch_profile(&self, name: Option<&str>) -> Result<SwitchResponse, GatewayError> {
        let response = self
            .authorize(
                self.http
                    .post(format!("{}/admin/switch-profile", self.base_url)),
            )
            .timeout(self.request_timeout)
            .json(&serde_json::json!({ "name": name }))
            .send()
            .await
            .map_err(|source| GatewayError::Transport(Box::new(source)))?;
        let answer = read(response).await?;
        if !answer.status.is_success() {
            return Ok(SwitchResponse::Buffered(answer));
        }
        serde_json::from_slice::<SwitchProfileBody>(&answer.body)
            .map(SwitchResponse::Selected)
            .map_err(|source| GatewayError::Malformed {
                message: "the switch-profile answer is not the outcome document".to_owned(),
                source: Some(Box::new(source)),
            })
    }

    /// Posts a cache-ensure request to `POST /v1/cache`, asking the gateway
    /// to make the blob at `source` available locally.
    ///
    /// A cache hit answers a buffered JSON `ready` event
    /// ([`CacheResponse::Buffered`] on a success status); a miss answers
    /// `text/event-stream` and returns [`CacheResponse::Download`], whose
    /// payload stream ends in a terminal `ready` or `error` event. Only
    /// the wait for the response headers is bounded, so a download stream
    /// runs without a deadline. A non-success status is buffered and
    /// returned.
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

    /// Subscribes to the gateway's `GET /admin/progress` event stream.
    ///
    /// The returned stream yields every progress event the gateway
    /// sends, beginning with the snapshot replay of the operations live
    /// at connect time. Heartbeat comment lines and other non-`data:`
    /// lines are skipped. Intermediate events are lossy at the source,
    /// so the stream can drop samples; detect completion only from
    /// `Finished` events, never from a fraction reaching 1.0. Only the
    /// wait for the response headers is bounded: the subscription is
    /// long-lived by design, so the stream itself runs without a deadline.
    /// The stream ends when the gateway closes the body; whether to
    /// resubscribe is the caller's decision.
    ///
    /// The endpoint answers only an event stream on success, so a
    /// non-success status (for example 401 on a rejected token) is a
    /// [`GatewayError::Status`], not a relayed response. Decode failures
    /// surface as per-item errors instead.
    ///
    /// # Errors
    /// Returns [`GatewayError::Transport`] if the request cannot be
    /// completed (the header bound elapsing included),
    /// [`GatewayError::Status`] on a non-success status, and
    /// [`GatewayError::ReadBody`] when that answer's body cannot be
    /// read.
    pub async fn subscribe_progress(&self) -> Result<ProgressStream, GatewayError> {
        let request = self.authorize(self.http.get(format!("{}/admin/progress", self.base_url)));
        let response = self.send_bounded(request).await?;
        progress::subscribe(response).await
    }
}

#[cfg(test)]
mod tests;
