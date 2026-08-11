//! Gateway error types and their mapping to the OpenAI error envelope.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

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

    /// A tool endpoint was reached but the tool is not configured.
    #[non_exhaustive]
    #[error("tool not configured: {0}")]
    ToolNotConfigured(&'static str),

    /// The request body could not be understood.
    #[non_exhaustive]
    #[error("malformed request: {0}")]
    MalformedRequest(String),

    /// The upstream backend could not be reached (transport-layer failure).
    #[non_exhaustive]
    #[error("upstream transport error")]
    UpstreamTransport(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The upstream backend returned a non-success status.
    #[non_exhaustive]
    #[error("upstream returned {status}")]
    UpstreamStatus {
        /// The status code the backend returned.
        status: u16,
        /// The (truncated) upstream body, for diagnostics.
        body: String,
    },

    /// The endpoint's waiting queue is full.
    #[error("queue full")]
    QueueFull,

    /// `POST /admin/switch-profile` named a profile that is not on disk.
    #[non_exhaustive]
    #[error("profile not found: {0}")]
    ProfileNotFound(String),

    /// Profile reload failed (bad TOML, include error, or local spawn failure).
    #[non_exhaustive]
    #[error("switch profile failed: {0}")]
    SwitchFailed(String),

    /// Admin profile routes were reached without a configured profiles directory.
    #[error("profiles directory not configured")]
    ProfilesUnavailable,
}

impl From<crate::queue::AdmitError> for GatewayError {
    fn from(value: crate::queue::AdmitError) -> Self {
        match value {
            // Both are "cannot admit now" from the client's perspective (503);
            // the queue layer keeps them distinct for diagnostics and tests.
            crate::queue::AdmitError::QueueFull | crate::queue::AdmitError::Unavailable => {
                GatewayError::QueueFull
            }
        }
    }
}

impl GatewayError {
    /// Wrap a transport error, hiding its concrete type from the public API.
    #[must_use]
    pub(crate) fn upstream_transport(source: reqwest::Error) -> GatewayError {
        GatewayError::UpstreamTransport(Box::new(source))
    }

    /// The `(status, type, code)` triple for the OpenAI error envelope.
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
            GatewayError::ToolNotConfigured(_) => {
                (StatusCode::NOT_FOUND, "invalid_request_error", "not_found")
            }
            GatewayError::MalformedRequest(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "malformed_request",
            ),
            GatewayError::UpstreamTransport(_) => (
                StatusCode::BAD_GATEWAY,
                "server_error",
                "upstream_transport",
            ),
            GatewayError::UpstreamStatus { status, .. } => {
                let code = StatusCode::from_u16(*status).unwrap_or(StatusCode::BAD_GATEWAY);
                if code.is_client_error() {
                    (code, "invalid_request_error", "upstream_client_error")
                } else {
                    (StatusCode::BAD_GATEWAY, "server_error", "upstream_error")
                }
            }
            GatewayError::QueueFull => (
                StatusCode::SERVICE_UNAVAILABLE,
                "server_error",
                "queue_full",
            ),
            GatewayError::ProfileNotFound(_) => (
                StatusCode::NOT_FOUND,
                "invalid_request_error",
                "profile_not_found",
            ),
            GatewayError::SwitchFailed(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "switch_failed",
            ),
            GatewayError::ProfilesUnavailable => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "profiles_unavailable",
            ),
        }
    }
}

impl IntoResponse for GatewayError {
    fn into_response(self) -> Response {
        let (status, kind, code) = self.classify();
        let body = Json(serde_json::json!({
            "error": { "message": self.to_string(), "type": kind, "code": code }
        }));
        (status, body).into_response()
    }
}

/// A configuration load or validation failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {path}")]
    Read {
        /// The path that could not be read.
        path: String,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML.
    #[non_exhaustive]
    #[error("parse config: {0}")]
    Parse(String),

    /// A `${VAR}` referenced an environment variable that was not set.
    #[non_exhaustive]
    #[error("unresolved environment variable {0}")]
    UnresolvedVar(String),

    /// A `${...}` interpolation was malformed (for example, unclosed).
    #[non_exhaustive]
    #[error("interpolation: {0}")]
    Interpolation(String),

    /// The configuration parsed but failed a semantic check.
    #[non_exhaustive]
    #[error("invalid config: {0}")]
    Validation(String),

    /// An `include` chain revisited a file already being resolved.
    #[non_exhaustive]
    #[error("include cycle at {path} (chain: {chain})")]
    IncludeCycle {
        /// The path that closed the cycle.
        path: String,
        /// The include stack when the cycle was detected.
        chain: String,
    },

    /// An `include` chain exceeded the maximum nesting depth.
    #[non_exhaustive]
    #[error("include depth exceeded {max} at {path}")]
    IncludeDepth {
        /// The path that would have been loaded next.
        path: String,
        /// The configured maximum depth.
        max: usize,
    },
}
