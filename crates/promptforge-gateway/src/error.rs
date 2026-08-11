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
///
/// Paths are kept as [`PathBuf`](std::path::PathBuf) and the include chain as a
/// `Vec<PathBuf>` (ERR-006); the TOML parse cause is preserved as a private
/// `#[source]` rather than flattened into a string (ERR-002).
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// The configuration file could not be read.
    #[non_exhaustive]
    #[error("read config {}", path.display())]
    Read {
        /// The path that could not be read.
        path: std::path::PathBuf,
        /// The underlying I/O error.
        #[source]
        source: std::io::Error,
    },

    /// The configuration was not valid TOML.
    #[non_exhaustive]
    #[error("parse config{}", parse_location(path.as_ref()))]
    Parse {
        /// The file the parse failure came from, when known.
        path: Option<std::path::PathBuf>,
        /// The underlying TOML deserialization error (boxed: it is large).
        #[source]
        source: Box<toml::de::Error>,
    },

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
    #[error("include cycle at {} (chain: {})", path.display(), render_chain(chain))]
    IncludeCycle {
        /// The path that closed the cycle.
        path: std::path::PathBuf,
        /// The include stack when the cycle was detected.
        chain: Vec<std::path::PathBuf>,
    },

    /// An `include` chain exceeded the maximum nesting depth.
    #[non_exhaustive]
    #[error("include depth exceeded {max} at {}", path.display())]
    IncludeDepth {
        /// The path that would have been loaded next.
        path: std::path::PathBuf,
        /// The configured maximum depth.
        max: usize,
    },
}

/// Renders the optional parse-failure path as a ` (path)` suffix or empty.
fn parse_location(path: Option<&std::path::PathBuf>) -> String {
    path.map(|p| format!(" ({})", p.display()))
        .unwrap_or_default()
}

/// Renders an include chain as `a -> b -> c`.
fn render_chain(chain: &[std::path::PathBuf]) -> String {
    chain
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(" -> ")
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
                GatewayError::QueueFull,
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "server_error",
                    "queue_full",
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
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn switch_failed_preserves_its_cause() {
        let error = GatewayError::switch_failed("load-profile", std::io::Error::other("disk"));
        assert!(error.source().is_some());
        assert!(error.to_string().contains("load-profile"));
        assert!(!error.to_string().contains("disk"));
    }

    #[test]
    fn admit_error_maps_both_variants_to_queue_full() {
        for admit in [
            crate::queue::AdmitError::QueueFull,
            crate::queue::AdmitError::Unavailable,
        ] {
            assert!(matches!(GatewayError::from(admit), GatewayError::QueueFull));
        }
    }
}
