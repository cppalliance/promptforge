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
    ) -> Self {
        Self::Protocol(ProtocolError::upstream_protocol(source))
    }

    /// Wraps a command failure (the boot load, an apply, an unload) at
    /// `stage`, preserving the cause.
    #[must_use]
    pub(crate) fn switch_failed(
        stage: &'static str,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> Self {
        Self::SwitchFailed {
            stage,
            source: Box::new(source),
        }
    }

    /// Wraps a cache-operation failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn cache(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Cache(Box::new(source))
    }

    /// Wraps a model-info read or parse failure, preserving the cause.
    #[cfg(feature = "local")]
    #[must_use]
    pub(crate) fn model_info(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::ModelInfo(Box::new(source))
    }

    /// The `(status, type, code)` triple for the OpenAI error envelope.
    #[expect(
        clippy::too_many_lines,
        reason = "a flat status table with one arm per error variant"
    )]
    pub(super) fn classify(&self) -> (StatusCode, &'static str, &'static str) {
        match self {
            Self::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "unauthorized",
            ),
            Self::UnknownModel(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "model_not_found",
            ),
            Self::KindMismatch { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "kind_mismatch",
            ),
            #[cfg(feature = "web-search")]
            Self::ToolNotConfigured(_) => {
                (StatusCode::NOT_FOUND, "invalid_request_error", "not_found")
            }
            Self::MalformedRequest(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "malformed_request",
            ),
            Self::InvalidVoice { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "invalid_voice",
            ),
            Self::Protocol(error) => error.classify(),
            Self::QueueFull => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "queue_full",
            ),
            Self::QueueRejected => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "queue_rejected",
            ),
            Self::UpstreamRateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limit_error",
                "upstream_rate_limited",
            ),
            Self::UpstreamUnavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "upstream_unavailable",
            ),
            Self::ModelProvisioning(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_provisioning",
            ),
            Self::ModelLoading(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "model_loading",
            ),
            Self::CommandCancelled(_) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "command_cancelled",
            ),
            Self::ProfileNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "profile_not_found",
            ),
            Self::SwitchFailed { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "switch_failed",
            ),
            #[cfg(feature = "local")]
            Self::PartialStart { .. } => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "partial_start",
            ),
            Self::BlockingTask(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "blocking_task_failed",
            ),
            Self::ConfigPathUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "config_path_unavailable",
            ),
            Self::ConfigWriteRejected(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_error",
                "config_write_rejected",
            ),
            Self::ConfigWriteIo(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "config_write_error",
            ),
            Self::ApplyReloadFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "apply_reload_failed",
            ),
            Self::ApplyCancelled => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "apply_cancelled",
            ),
            Self::RestartUnavailable => (
                StatusCode::CONFLICT,
                "invalid_request_error",
                "restart_unavailable",
            ),
            Self::PendingConfig(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "pending_config_error",
            ),
            Self::EnvFile(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "env_file_error",
            ),
            Self::RevealPathNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "reveal_path_not_found",
            ),
            Self::RevealFailed(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "reveal_error",
            ),
            #[cfg(feature = "local")]
            Self::Cache(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "cache_error",
            ),
            #[cfg(feature = "local")]
            Self::CacheEntryNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "cache_entry_not_found",
            ),
            #[cfg(feature = "local")]
            Self::ModelInfo(_) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "invalid_request_error",
                "model_info_error",
            ),
            Self::CloudModelsLoading => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "cloud_models_loading",
            ),
            Self::CloudModelsBodyTooLarge { .. } => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_body_too_large",
            ),
            Self::CloudModelsSchemaVersion { .. } => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_schema_version",
            ),
            Self::CloudModelsUnavailable(_) => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "cloud_models_unavailable",
            ),
        }
    }
}
