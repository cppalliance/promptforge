//! The gateway client's wire types: the buffered relay response, the
//! forwarded config-panel response, the SSE payload stream, the typed
//! cache events decoded from it, and the profile-selection outcome.

use std::path::PathBuf;
use std::pin::Pin;

use futures_util::stream::Stream;
use serde::Deserialize;

use super::GatewayError;

/// A gateway HTTP response captured for verbatim relay.
#[derive(Debug)]
pub struct GatewayResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// A gateway response captured for the config-panel proxy: the relay
/// keeps the content type alongside the status and body, because the
/// config UI distinguishes a buffered JSON answer from an SSE stream by
/// it.
#[derive(Debug)]
pub struct ForwardedResponse {
    /// The gateway's status code, relayed unchanged.
    pub status: reqwest::StatusCode,
    /// The gateway's `Content-Type`, when it sent one.
    pub content_type: Option<String>,
    /// The gateway's response body, relayed byte-for-byte.
    pub body: Vec<u8>,
}

/// A stream of SSE `data:` payloads from the gateway, in arrival order.
///
/// Each item is one event's data, verbatim. A transport failure mid-stream
/// yields one error item and then ends the stream.
pub type SsePayloadStream = Pin<Box<dyn Stream<Item = Result<String, GatewayError>> + Send>>;

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
    /// The gateway is downloading the blob; `payloads` is the SSE stream
    /// of [`CacheEvent`] JSON documents.
    Download {
        /// The gateway's success status.
        status: reqwest::StatusCode,
        /// The SSE payload stream, ending in a terminal `ready` or `error`
        /// event.
        payloads: SsePayloadStream,
    },

    /// Any other answer, buffered: a cache hit's `ready` JSON on a success
    /// status, or the gateway's error envelope on a failure status.
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
/// The `path` in a `Ready` event names a file on the gateway host, so
/// the cache API is only meaningful to a workshop sharing the gateway's
/// filesystem - the standard local deployment, where both run on loopback.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
#[non_exhaustive]
pub enum CacheEvent {
    /// A progress sample from a running download.
    Downloading {
        /// Cumulative bytes downloaded so far.
        bytes: u64,
        /// Total bytes expected; null when the upstream server sent no
        /// Content-Length.
        total: Option<u64>,
    },

    /// The blob is cached and ready at `path`.
    Ready {
        /// Local path of the cached blob on the gateway host.
        path: PathBuf,
    },

    /// The download failed.
    Error {
        /// The gateway's description of the failure.
        message: String,
    },
}

/// The gateway's answer to a profile selection, `POST /admin/switch-profile`.
///
/// An accepted selection answers one JSON document decoding as
/// [`SwitchProfileBody`]: the gateway persisted the selection and reports
/// whether it must restart to load it. A refusal (bad auth, a malformed or
/// undefined name) is buffered rather than reported as an error, matching
/// the relay contract of the other client methods.
#[derive(Debug)]
#[non_exhaustive]
pub enum SwitchResponse {
    /// The gateway accepted and persisted the selection.
    Selected(SwitchProfileBody),

    /// A refusal, buffered: the gateway's error envelope.
    Buffered(GatewayResponse),
}

/// The body of an accepted profile selection.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SwitchProfileBody {
    /// The selection now persisted: a profile name, or `None` for no
    /// profile.
    #[serde(default)]
    pub profile: Option<String>,
    /// Whether the gateway must restart before the selection is served;
    /// `false` when the selection already matches the running profile.
    pub restart_required: bool,
}
