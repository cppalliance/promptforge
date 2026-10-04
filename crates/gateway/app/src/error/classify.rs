//! The [`GatewayError`] constructors and the status table behind its
//! OpenAI error envelope.

use axum::http::StatusCode;
use gateway_protocol::ProtocolError;

use super::GatewayError;

impl GatewayError {
    /// Wraps a body-decode failure as a protocol error (not a transport error),
    /// preserving the cause via `source()`. See [`ProtocolError::upstream_protocol`].
    #[must_use]
    pub(crate) fn upstream_protocol(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::Protocol(ProtocolError::upstream_protocol(source))
    }

    /// Wraps a command failure (the boot load, an apply, an unload) at
    /// `stage`, preserving the cause.
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

    /// Wraps a cache-operation failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn cache(source: impl std::error::Error + Send + Sync + 'static) -> GatewayError {
        GatewayError::Cache(Box::new(source))
    }

    /// Wraps a model-info read or parse failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn model_info(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::ModelInfo(Box::new(source))
    }

    /// The `(status, type, code)` triple for the OpenAI error envelope.
    #[expect(
        clippy::too_many_lines,
        reason = "a flat status table with one arm per error variant"
    )]
    pub(super) fn classify(&self) -> (StatusCode, &'static str, &'static str) {
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
            GatewayError::InvalidVoice { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "invalid_voice",
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
            GatewayError::UpstreamRateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "upstream_rate_limited",
            ),
            GatewayError::UpstreamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "upstream_unavailable",
            ),
            GatewayError::ModelProvisioning(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_provisioning",
            ),
            GatewayError::ModelLoading(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_loading",
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
            GatewayError::BlockingTask(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "blocking_task_failed",
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
            GatewayError::CloudModelsLoading => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "cloud_models_loading",
            ),
            GatewayError::CloudModelsBodyTooLarge { .. } => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_body_too_large",
            ),
            GatewayError::CloudModelsSchemaVersion { .. } => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_schema_version",
            ),
            GatewayError::CloudModelsUnavailable(_) => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_unavailable",
            ),
        }
    }
}
