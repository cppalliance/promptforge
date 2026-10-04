//! Gateway error types and their mapping to the OpenAI error envelope.

use axum::Json;
use axum::response::{IntoResponse, Response};
use gateway_config::ModelKind;
use gateway_protocol::ProtocolError;

mod classify;
mod wire;

pub(crate) use self::wire::{WireJson, WirePath, WireQuery};

/// A request-time failure, rendered to the client as an OpenAI error envelope.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum GatewayError {
    /// The bearer key was missing or did not match `server.key`.
    #[error("unauthorized")]
    Unauthorized,

    /// The request named a model with no `[[model]]` entry.
    #[non_exhaustive]
    #[error("unknown model {0}")]
    UnknownModel(String),

    /// The route's workload does not match the model's configured kind
    /// (e.g. an embedding model named on the chat route).
    #[non_exhaustive]
    #[error("model {model} is {actual}, not {expected}")]
    KindMismatch {
        /// The caller-facing model name.
        model: String,
        /// The workload the route serves.
        expected: ModelKind,
        /// The workload the model is configured for.
        actual: ModelKind,
    },

    /// A tool endpoint was reached but the tool is not configured.
    #[cfg(feature = "web-search")]
    #[non_exhaustive]
    #[error("tool not configured: {0}")]
    ToolNotConfigured(&'static str),

    /// The request body could not be understood.
    #[non_exhaustive]
    #[error("malformed request: {0}")]
    MalformedRequest(String),

    /// The speech request's `voice` is not one of the model's catalog
    /// voices. Checked at the route before queue admission, so the 400
    /// never burns a queue slot; the message names the valid voices.
    #[non_exhaustive]
    #[error("unknown voice {voice}; the model offers: {}", valid.join(", "))]
    InvalidVoice {
        /// The voice the request named.
        voice: String,
        /// The voices the model's catalog declares.
        valid: Vec<String>,
    },

    /// A transport- or protocol-level failure from the upstream seam. The
    /// variants are defined in [`ProtocolError`]; the gateway wraps them so
    /// a route handler deals with one error type.
    #[non_exhaustive]
    #[error(transparent)]
    Protocol(#[from] ProtocolError),

    /// The endpoint's waiting queue is full.
    #[error("queue full")]
    QueueFull,

    /// The queue's fail-fast `Reject` policy turned the request away at
    /// capacity. Maps to 429 so an OpenAI client surfaces a retryable
    /// rate-limit error rather than a server failure.
    #[error("queue rejected at capacity")]
    QueueRejected,

    /// The upstream provider rate-limited a speech request. Maps to 429 so
    /// an OpenAI client surfaces a retryable rate-limit error rather than
    /// a server failure. Speech-only: every other route keeps the shared
    /// [`ProtocolError`] mapping, so their envelopes stay bit-identical.
    #[error("upstream rate limited")]
    UpstreamRateLimited,

    /// The upstream provider was unavailable for a speech request. Maps to
    /// 503 so a client can retry, rather than the shared mapping's 502.
    /// Speech-only for the same reason as [`GatewayError::UpstreamRateLimited`].
    #[error("upstream unavailable")]
    UpstreamUnavailable,

    /// The named model is configured but not yet loaded, and a queue command
    /// (named in the message) is working on the routing table. Maps to 503
    /// so a client can retry once the active command completes.
    #[non_exhaustive]
    #[error("model provisioning in progress: {0}")]
    ModelProvisioning(String),

    /// The named local model belongs to the boot profile and its child is
    /// spawning: the remote models already serve, and this one follows
    /// once its child is ready. Maps to 503 with `Retry-After` so a client
    /// waits briefly instead of treating the model as missing.
    #[non_exhaustive]
    #[error("model is loading: {0}")]
    ModelLoading(String),

    /// A queued command was cancelled before it completed: by the user, by a
    /// newer command winning the debounce, or by process shutdown.
    #[non_exhaustive]
    #[error("command cancelled: {0}")]
    CommandCancelled(String),

    /// `POST /admin/switch-profile` named a profile the live catalog does
    /// not define; the message names the profiles it does.
    #[non_exhaustive]
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// A command (the boot load, an apply, an unload) failed at a named
    /// stage; the underlying cause is preserved via `source()` rather than
    /// flattened into a string.
    #[non_exhaustive]
    #[error("switch profile failed at {stage}")]
    SwitchFailed {
        /// The switch stage that failed (for diagnostics).
        stage: &'static str,
        /// The underlying cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Some target-profile local models started while others failed.
    #[cfg(feature = "local")]
    #[non_exhaustive]
    #[error("profile {profile} started partially; loaded: {loaded:?}; not started: {failed:?}")]
    PartialStart {
        /// Target profile now active in degraded mode.
        profile: String,
        /// Local model names that reached readiness and remain running.
        loaded: Vec<String>,
        /// Failed model names and their startup errors.
        failed: Vec<String>,
    },

    /// A blocking-pool task the route dispatched through [`blocking`] did
    /// not run to completion: it panicked, or the runtime is shutting down.
    /// Never the caller's fault, whatever the task was doing, so every
    /// route maps a join failure here instead of choosing a domain variant.
    #[non_exhaustive]
    #[error("blocking task failed")]
    BlockingTask(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A file-backed admin route was reached without a known config path.
    #[error("config path not configured")]
    ConfigPathUnavailable,

    /// A shadow-write route refused the payload: the body could not be
    /// rendered as TOML, a redacted secret had no existing value to
    /// preserve, or the merged pending configuration failed validation.
    /// The message includes the full cause chain so the UI can show why.
    #[non_exhaustive]
    #[error("config write rejected: {0}")]
    ConfigWriteRejected(String),

    /// A shadow file could not be written to disk after validation passed.
    #[non_exhaustive]
    #[error("config write failed")]
    ConfigWriteIo(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// `POST /admin/config-apply` ran its profile activation and it failed.
    /// A failure before the commit promoted nothing, so the pending changes
    /// stay staged and `GET /admin/config-dirty` still reports them; a
    /// `PartialStart` (a `local` build) lands after the commit, with the
    /// shadows promoted and the profile live minus the models that did not
    /// start. The message includes the activation failure's full cause
    /// chain; callers inspect status before retrying because it tells the
    /// two apart.
    #[non_exhaustive]
    #[error("profile activation failed ({0}); inspect gateway status before retrying Apply")]
    ApplyReloadFailed(String),

    /// `POST /admin/config-apply` was cancelled - by the user, by a revert,
    /// or by process shutdown - before its commit, so nothing was promoted
    /// and the pending changes stay staged for a retry.
    #[error("apply cancelled; the pending changes are still staged, retry Apply")]
    ApplyCancelled,

    /// A pending-state read could not resolve the shadow-overlaid
    /// configuration: a chain file or shadow is unreadable, unparsable, or
    /// the merged pending result fails validation. Saves validate before
    /// writing, so this means the on-disk pending state was corrupted out
    /// of band. The message includes the full cause chain.
    #[non_exhaustive]
    #[error("pending config unreadable: {0}")]
    PendingConfig(String),

    /// An `.env` file could not be read or parsed for `GET /admin/env`.
    #[non_exhaustive]
    #[error("env file unreadable")]
    EnvFile(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// `POST /admin/reveal` named a path that does not exist.
    #[non_exhaustive]
    #[error("reveal path not found: {0}")]
    RevealPathNotFound(String),

    /// The reveal's file manager could not be resolved or spawned after
    /// every refusal check passed.
    #[non_exhaustive]
    #[error("reveal failed")]
    RevealFailed(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// A `/v1/cache` route failed at the storage or transport layer before its
    /// response was committed (mid-stream failures are SSE error events, not
    /// this variant).
    #[cfg(feature = "local")]
    #[non_exhaustive]
    #[error("cache operation failed")]
    Cache(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// `DELETE /v1/cache/{sha256}` named a digest that matches no cache
    /// entry.
    #[cfg(feature = "local")]
    #[non_exhaustive]
    #[error("cache entry not found: {0}")]
    CacheEntryNotFound(String),

    /// `GET /admin/model-info` could not read or parse the named GGUF file.
    /// Maps to 422 so the UI's fallback (plain layer readout) triggers
    /// without looking like a server fault.
    #[cfg(feature = "local")]
    #[non_exhaustive]
    #[error("model info unavailable")]
    ModelInfo(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The cloud provider model sheet has not arrived: no usable cache
    /// existed at launch and the background download is still running.
    /// Maps to 503 so the config UI keeps polling until the sheet lands.
    #[non_exhaustive]
    #[error("cloud provider model sheet is still downloading")]
    CloudModelsLoading,

    /// The cloud provider model sheet download announced a body over the
    /// gateway's JSON body cap, so the read was refused before the body
    /// allocated. Maps to 502 like every other sheet download failure.
    #[non_exhaustive]
    #[error("cloud provider model sheet announced a {announced} byte body over the {cap} byte cap")]
    CloudModelsBodyTooLarge {
        /// The body size the response's `Content-Length` announced.
        announced: u64,
        /// The cap the download refused to exceed, in bytes.
        cap: usize,
    },

    /// The cloud provider model sheet parsed but declared a schema
    /// version this gateway does not accept; the message names both
    /// versions. Maps to 502 like every other sheet download failure.
    #[non_exhaustive]
    #[error(
        "cloud provider model sheet schema version {found} is not accepted; this gateway accepts version {accepted}"
    )]
    CloudModelsSchemaVersion {
        /// The schema version the sheet declared.
        found: u32,
        /// The schema version this gateway accepts.
        accepted: u32,
    },

    /// No cloud provider model sheet is available and the last download
    /// or cache write failed; the message names the failure.
    #[non_exhaustive]
    #[error("cloud provider model sheet unavailable: {0}")]
    CloudModelsUnavailable(String),
}

impl From<crate::queue::AdmitError> for GatewayError {
    fn from(value: crate::queue::AdmitError) -> Self {
        match value {
            // Both are "cannot admit now" from the client's perspective (503);
            // the queue layer keeps them distinct for diagnostics and tests.
            crate::queue::AdmitError::QueueFull | crate::queue::AdmitError::Unavailable => {
                GatewayError::QueueFull
            }
            // Fail-fast rejection is client-visible back-pressure (429), not
            // a server-side failure.
            crate::queue::AdmitError::Rejected => GatewayError::QueueRejected,
            // `AdmitError` is non-exhaustive across the crate boundary; any
            // future variant is a "cannot admit now" condition and maps to
            // the same 503 as a full queue.
            _ => GatewayError::QueueFull,
        }
    }
}

#[cfg(feature = "web-search")]
impl From<gateway_web_search::WebSearchError> for GatewayError {
    fn from(value: gateway_web_search::WebSearchError) -> Self {
        use gateway_web_search::WebSearchError;
        match value {
            WebSearchError::MalformedRequest(message) => GatewayError::MalformedRequest(message),
            WebSearchError::Protocol(error) => GatewayError::Protocol(error),
            // `WebSearchError` is non-exhaustive across the crate boundary; a
            // future variant renders as a malformed request rather than
            // failing to compile here.
            _ => GatewayError::MalformedRequest(value.to_string()),
        }
    }
}

/// The `Retry-After` a loading model's 503 sets, in seconds: long enough
/// that a polling client does not hammer the gateway while a child loads
/// its weights, short enough that it notices the model within one poll of
/// the spawn completing.
const MODEL_LOADING_RETRY_AFTER_SECONDS: u16 = 5;

impl GatewayError {
    /// The OpenAI error envelope body for this error, shared by the JSON
    /// error response and the mid-stream SSE error event.
    pub(crate) fn envelope(&self) -> serde_json::Value {
        let (_, kind, code) = self.classify();
        serde_json::json!({
            "error": { "message": self.to_string(), "type": kind, "code": code }
        })
    }

    /// The `Retry-After` header value, in seconds, for the errors that
    /// promise the condition clears on its own.
    fn retry_after_seconds(&self) -> Option<u16> {
        matches!(self, GatewayError::ModelLoading(_)).then_some(MODEL_LOADING_RETRY_AFTER_SECONDS)
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, ..) = self.classify();
        let retry_after = self.retry_after_seconds();
        let mut response = (status, Json(self.envelope())).into_response();
        if let Some(seconds) = retry_after {
            response
                .headers_mut()
                .insert(axum::http::header::RETRY_AFTER, seconds.into());
        }
        response
    }
}

/// Renders an error and every source beneath it as one `; `-joined line.
///
/// The gateway's wire messages are one line, so a variant that needs
/// the whole chain in its message flattens it here. The multi-line form
/// the binary writes to the log and to stderr is a different rendering
/// and stays in `main.rs`.
///
/// A cause that renders as nothing, and a cause whose text the
/// accumulated rendering already contains, are both skipped: some
/// variants copy their source's text into their own message, and
/// appending that cause again would print it twice. The check is a plain
/// substring test on the text rendered so far.
pub(crate) fn error_chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        let cause_text = cause.to_string();
        if !cause_text.is_empty() && !text.contains(&cause_text) {
            text.push_str("; ");
            text.push_str(&cause_text);
        }
        source = cause.source();
    }
    text
}

/// Maps a config-crate failure onto the wire: a failed disk write is a
/// server fault (500), everything else - validation, parse, unresolved
/// `${VAR}`, an unreadable chain file - rejects the payload (422) with the
/// full cause chain so the UI can show why the save failed.
pub(crate) fn config_write_error(error: gateway_config::ConfigError) -> GatewayError {
    if error.kind() == gateway_config::ConfigErrorKind::Write {
        GatewayError::ConfigWriteIo(Box::new(error))
    } else {
        GatewayError::ConfigWriteRejected(error_chain(&error))
    }
}

/// Maps a config-crate failure on a pending read: saves validate before
/// writing, so an unresolvable pending state is a server fault (500) with
/// the full cause chain in the message.
pub(crate) fn pending_read_error(error: &gateway_config::ConfigError) -> GatewayError {
    GatewayError::PendingConfig(error_chain(error))
}

/// Runs `work` on tokio's blocking pool and hands back its value.
///
/// Every route that touches the filesystem, a blocking client, or an OS
/// counter goes through here, so the one thing that can fail around the
/// work - the join, when the task panicked or the runtime is draining -
/// is mapped in one place to [`GatewayError::BlockingTask`]. A closure
/// that itself returns a `Result` composes as `blocking(..).await??` (or
/// `.await?.map_err(..)?`), keeping the domain error mapping beside the
/// domain code and the join mapping out of it.
pub(crate) async fn blocking<T, F>(work: F) -> Result<T, GatewayError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|join| GatewayError::BlockingTask(Box::new(join)))
}

#[cfg(test)]
mod tests;
