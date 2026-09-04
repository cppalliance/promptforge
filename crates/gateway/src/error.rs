//! Gateway error types and their mapping to the OpenAI error envelope.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use gateway_config::ModelKind;
use shared_protocol::ProtocolError;

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

    /// The uploaded transcription body exceeded the configured cap.
    #[cfg(feature = "stt")]
    #[error("audio file exceeds the 25 MiB limit")]
    AudioTooLarge,

    /// The active STT engine rejected an otherwise valid request.
    #[cfg(feature = "stt")]
    #[non_exhaustive]
    #[error("transcription failed")]
    Transcription(#[source] gateway_stt::TranscriptionError),

    /// A transport- or protocol-level failure from the upstream seam. The
    /// variants live in [`ProtocolError`]; the gateway wraps them so a route
    /// handler deals with one error type.
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

    /// A bounded profile-switch drain expired and cancelled the request.
    #[error("request cancelled for profile switch")]
    RequestCancelled,

    /// The named model is configured but not yet loaded, and a queue command
    /// (carried in the message) is working on the routing table. Maps to 503
    /// so a client can retry once the active command completes.
    #[non_exhaustive]
    #[error("model provisioning in progress: {0}")]
    ModelProvisioning(String),

    /// A queued command was cancelled before it completed: by the user, by a
    /// newer command winning the debounce, or by process shutdown.
    #[non_exhaustive]
    #[error("command cancelled: {0}")]
    CommandCancelled(String),

    /// `POST /admin/switch-profile` named a profile that is not on disk.
    #[non_exhaustive]
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// Profile reload failed at a named stage; the underlying cause is
    /// preserved via `source()` rather than flattened into a string.
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
    #[error("profile {profile} started partially; loaded: {loaded:?}; failed: {failed:?}")]
    PartialStart {
        /// Target profile now active in degraded mode.
        profile: String,
        /// Local model names that reached readiness and remain running.
        loaded: Vec<String>,
        /// Failed model names and their startup errors.
        failed: Vec<String>,
    },

    /// An operation required a selected profile, but none is active.
    #[error("active profile not configured")]
    ActiveProfileUnavailable,

    /// A file-backed admin route was reached without a known config path.
    #[error("config path not configured")]
    ConfigPathUnavailable,

    /// A shadow-write route refused the payload: the body could not be
    /// rendered as TOML, a redacted secret had no existing value to
    /// preserve, or the merged pending configuration failed validation.
    /// The message carries the full cause chain so the UI can show why.
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
    /// start. The message carries the activation failure's full cause
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
    /// of band. The message carries the full cause chain.
    #[non_exhaustive]
    #[error("pending config unreadable: {0}")]
    PendingConfig(String),

    /// An `.env` file could not be read or parsed for `GET /admin/env`.
    #[non_exhaustive]
    #[error("env file unreadable")]
    EnvFile(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The `GET /admin/system` sampling task did not run to completion.
    #[non_exhaustive]
    #[error("system metrics sampling failed")]
    SystemMetrics(#[source] Box<dyn std::error::Error + Send + Sync>),

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

    /// `DELETE /v1/cache/{sha256}` named a digest no cache entry carries.
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

#[cfg(feature = "stt")]
impl From<gateway_stt::TranscriptionError> for GatewayError {
    fn from(value: gateway_stt::TranscriptionError) -> Self {
        if let Some(model) = value.model_not_found() {
            return GatewayError::UnknownModel(model.to_owned());
        }
        if value.is_file_too_large() {
            return GatewayError::AudioTooLarge;
        }
        if value.is_inference() {
            GatewayError::Transcription(value)
        } else {
            GatewayError::MalformedRequest(value.to_string())
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

impl GatewayError {
    /// Wrap a body-decode failure as a protocol error (not a transport error),
    /// preserving the cause via `source()`. See [`ProtocolError::upstream_protocol`].
    #[must_use]
    pub(crate) fn upstream_protocol(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::Protocol(ProtocolError::upstream_protocol(source))
    }

    /// Wrap a profile-switch failure at `stage`, preserving the cause.
    #[must_use]
    pub(crate) fn switch_failed(
        stage: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::SwitchFailed {
            stage,
            source: Box::new(source),
        }
    }

    /// Wrap a cache-operation failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn cache(source: impl std::error::Error + Send + Sync + 'static) -> GatewayError {
        GatewayError::Cache(Box::new(source))
    }

    /// Wrap a model-info read or parse failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn model_info(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::ModelInfo(Box::new(source))
    }

    /// Wrap a system-metrics sampling failure, preserving the cause.
    #[must_use]
    pub(crate) fn system_metrics(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::SystemMetrics(Box::new(source))
    }

    /// The `(status, type, code)` triple for the OpenAI error envelope.
    #[expect(
        clippy::too_many_lines,
        reason = "a flat status table with one arm per error variant"
    )]
    fn classify(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            GatewayError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "unauthorized",
            ),
            GatewayError::UnknownModel(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "model_not_found",
            ),
            GatewayError::KindMismatch { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "kind_mismatch",
            ),
            #[cfg(feature = "web-search")]
            GatewayError::ToolNotConfigured(_) => {
                (StatusCode::NOT_FOUND, "invalid_request_error", "not_found")
            }
            GatewayError::MalformedRequest(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "malformed_request",
            ),
            #[cfg(feature = "stt")]
            GatewayError::AudioTooLarge => (
                StatusCode::PAYLOAD_TOO_LARGE,
                "invalid_request_error",
                "file_too_large",
            ),
            #[cfg(feature = "stt")]
            GatewayError::Transcription(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "transcription_error",
            ),
            GatewayError::Protocol(error) => error.classify(),
            GatewayError::QueueFull => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "queue_full",
            ),
            GatewayError::QueueRejected => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "queue_rejected",
            ),
            GatewayError::RequestCancelled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "profile_switch",
            ),
            GatewayError::ModelProvisioning(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_provisioning",
            ),
            GatewayError::CommandCancelled(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "command_cancelled",
            ),
            GatewayError::ProfileNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "profile_not_found",
            ),
            GatewayError::SwitchFailed { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "switch_failed",
            ),
            #[cfg(feature = "local")]
            GatewayError::PartialStart { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "partial_start",
            ),
            GatewayError::ActiveProfileUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "active_profile_unavailable",
            ),
            GatewayError::ConfigPathUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "config_path_unavailable",
            ),
            GatewayError::ConfigWriteRejected(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_error",
                "config_write_rejected",
            ),
            GatewayError::ConfigWriteIo(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "config_write_error",
            ),
            GatewayError::ApplyReloadFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "apply_reload_failed",
            ),
            GatewayError::ApplyCancelled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "apply_cancelled",
            ),
            GatewayError::PendingConfig(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "pending_config_error",
            ),
            GatewayError::EnvFile(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "env_file_error",
            ),
            GatewayError::SystemMetrics(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "system_metrics_error",
            ),
            GatewayError::RevealPathNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "reveal_path_not_found",
            ),
            GatewayError::RevealFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "reveal_error",
            ),
            #[cfg(feature = "local")]
            GatewayError::Cache(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "cache_error",
            ),
            #[cfg(feature = "local")]
            GatewayError::CacheEntryNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "cache_entry_not_found",
            ),
            #[cfg(feature = "local")]
            GatewayError::ModelInfo(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_error",
                "model_info_error",
            ),
        }
    }
}

impl GatewayError {
    /// The OpenAI error envelope body for this error, shared by the JSON
    /// error response and the mid-stream SSE error event.
    pub(crate) fn envelope(&self) -> serde_json::Value {
        let (_, kind, code) = self.classify();
        serde_json::json!({
            "error": { "message": self.to_string(), "type": kind, "code": code }
        })
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, ..) = self.classify();
        (status, Json(self.envelope())).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn gateway_error_classify_is_table_driven() {
        let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
            (
                GatewayError::Unauthorized,
                (
                    StatusCode::UNAUTHORIZED,
                    "authentication_error",
                    "unauthorized",
                ),
            ),
            (
                GatewayError::UnknownModel("m".to_owned()),
                (
                    StatusCode::NOT_FOUND,
                    "invalid_request_error",
                    "model_not_found",
                ),
            ),
            (
                GatewayError::KindMismatch {
                    model: "m".to_owned(),
                    expected: ModelKind::Chat,
                    actual: ModelKind::Embedding,
                },
                (
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "kind_mismatch",
                ),
            ),
            (
                GatewayError::QueueFull,
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "server_error",
                    "queue_full",
                ),
            ),
            (
                GatewayError::QueueRejected,
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit_error",
                    "queue_rejected",
                ),
            ),
            (
                GatewayError::switch_failed("build-routing", std::io::Error::other("x")),
                (
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "switch_failed",
                ),
            ),
            (
                GatewayError::system_metrics(std::io::Error::other("x")),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "system_metrics_error",
                ),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn shadow_route_errors_classify_is_table_driven() {
        let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
            (
                GatewayError::ConfigWriteRejected("invalid config: bad".to_owned()),
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "invalid_request_error",
                    "config_write_rejected",
                ),
            ),
            (
                GatewayError::ConfigWriteIo(Box::new(std::io::Error::other("disk"))),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "config_write_error",
                ),
            ),
            (
                GatewayError::ConfigPathUnavailable,
                (
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "config_path_unavailable",
                ),
            ),
            (
                GatewayError::EnvFile(Box::new(std::io::Error::other("bad line"))),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "env_file_error",
                ),
            ),
            (
                GatewayError::PendingConfig("corrupt shadow".to_owned()),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "pending_config_error",
                ),
            ),
            (
                GatewayError::ApplyReloadFailed("ghost endpoint".to_owned()),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "apply_reload_failed",
                ),
            ),
            (
                GatewayError::ApplyCancelled,
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "server_error",
                    "apply_cancelled",
                ),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn reveal_errors_classify_is_table_driven() {
        let cases: Vec<(GatewayError, (StatusCode, &str, &str))> = vec![
            (
                GatewayError::RevealPathNotFound("C:/ghost.gguf".to_owned()),
                (
                    StatusCode::NOT_FOUND,
                    "invalid_request_error",
                    "reveal_path_not_found",
                ),
            ),
            (
                GatewayError::RevealFailed(Box::new(std::io::Error::other("spawn"))),
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "reveal_error",
                ),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn protocol_error_delegates_classify_and_display() {
        // The protocol crate owns the transport/protocol variants and their
        // envelope mapping; the gateway wrapper delegates both and stays
        // transparent in the source chain.
        let error = GatewayError::upstream_protocol(std::io::Error::other("bad json"));
        assert!(matches!(error, GatewayError::Protocol(_)));
        assert_eq!(
            error.classify(),
            (StatusCode::BAD_GATEWAY, "server_error", "upstream_protocol")
        );
        assert_eq!(error.to_string(), "upstream protocol error");
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("bad json")
        );
    }

    #[cfg(feature = "stt")]
    #[test]
    fn unloaded_stt_model_maps_to_openai_model_not_found() {
        let error = GatewayError::from(gateway_stt::TranscriptionError::model_not_found_error(
            "ghost",
        ));
        assert!(matches!(error, GatewayError::UnknownModel(model) if model == "ghost"));
        let error = GatewayError::UnknownModel("ghost".to_owned());
        assert_eq!(
            error.classify(),
            (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "model_not_found"
            )
        );
    }

    #[test]
    fn switch_failed_preserves_its_cause() {
        let error = GatewayError::switch_failed("load-profile", std::io::Error::other("disk"));
        assert!(error.source().is_some());
        assert!(error.to_string().contains("load-profile"));
        assert!(!error.to_string().contains("disk"));
    }

    #[test]
    fn admit_error_maps_to_queue_errors() {
        for admit in [
            crate::queue::AdmitError::QueueFull,
            crate::queue::AdmitError::Unavailable,
        ] {
            assert!(matches!(GatewayError::from(admit), GatewayError::QueueFull));
        }
        assert!(matches!(
            GatewayError::from(crate::queue::AdmitError::Rejected),
            GatewayError::QueueRejected
        ));
    }

    #[cfg(feature = "web-search")]
    #[test]
    fn web_search_error_maps_to_gateway_error() {
        use gateway_web_search::WebSearchError;
        // The malformed-request arm preserves the message verbatim, so the
        // wire envelope is unchanged by the crate boundary.
        let err = GatewayError::from(WebSearchError::MalformedRequest(
            "web_search: empty query".to_string(),
        ));
        assert!(
            matches!(&err, GatewayError::MalformedRequest(m) if m == "web_search: empty query")
        );
        assert_eq!(
            err.classify(),
            (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "malformed_request"
            )
        );
        // The protocol arm is transparent: same variant, same display.
        let err = GatewayError::from(WebSearchError::from(ProtocolError::upstream_status(
            502,
            "bad gateway".to_string(),
        )));
        assert!(matches!(err, GatewayError::Protocol(_)));
        assert_eq!(err.to_string(), "upstream returned 502");
    }
}
