//! Gateway error types and their mapping to the OpenAI error envelope.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use promptforge_gateway_config::ModelKind;
use promptforge_gateway_protocol::ProtocolError;

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

    /// Admin profile routes were reached without a configured profiles directory.
    #[error("profiles directory not configured")]
    ProfilesUnavailable,

    /// A boot-config route was reached without a known boot config path
    /// (the gateway was assembled without one, as in embedded use).
    #[error("boot config path not configured")]
    BootConfigUnavailable,

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

    /// `PUT /admin/include/{path}` named a file absent from the profiles
    /// directory.
    #[non_exhaustive]
    #[error("include file not found: {0}")]
    IncludeNotFound(String),

    /// `POST /admin/config-apply` promoted every shadow but the profile
    /// reload failed: the gateway keeps running the previous configuration
    /// while the real files already carry the new one, which loads on the
    /// next restart. The message carries the reload failure's full cause
    /// chain.
    #[non_exhaustive]
    #[error(
        "config promoted to disk but the reload failed ({0}); the gateway keeps \
         running the previous configuration and the new config loads on the \
         next restart"
    )]
    ApplyReloadFailed(String),

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

#[cfg(feature = "web-search")]
impl From<promptforge_web_search_service::WebSearchError> for GatewayError {
    fn from(value: promptforge_web_search_service::WebSearchError) -> Self {
        use promptforge_web_search_service::WebSearchError;
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
            GatewayError::ProfilesUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "profiles_unavailable",
            ),
            GatewayError::BootConfigUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "boot_config_unavailable",
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
            GatewayError::IncludeNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "include_not_found",
            ),
            GatewayError::ApplyReloadFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "apply_reload_failed",
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
                GatewayError::IncludeNotFound("ghost.toml".to_owned()),
                (
                    StatusCode::NOT_FOUND,
                    "invalid_request_error",
                    "include_not_found",
                ),
            ),
            (
                GatewayError::BootConfigUnavailable,
                (
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "boot_config_unavailable",
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
        use promptforge_web_search_service::WebSearchError;
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
