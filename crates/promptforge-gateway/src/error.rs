//! Gateway error types and their mapping to the OpenAI error envelope.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use promptforge_gateway_config::ModelKind;

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

    /// The resolved model's upstream cannot serve the route's workload (for
    /// example a local chat server asked for embeddings). Distinct from
    /// [`GatewayError::KindMismatch`]: the kind matches, but the backing
    /// upstream has no implementation for it.
    #[non_exhaustive]
    #[error("model {0} is not available for this workload")]
    ModelUnavailable(String),

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

    /// The upstream returned a success status but a body that could not be
    /// decoded into the expected shape.
    ///
    /// Distinct from [`GatewayError::UpstreamTransport`] so a decode failure
    /// (a protocol problem) never masquerades as a transport death and triggers
    /// a spurious local `llama-server` respawn (UP-004, UPSTREAM-003). The cause
    /// is preserved via `source()`.
    #[non_exhaustive]
    #[error("upstream protocol error")]
    UpstreamProtocol(#[source] Box<dyn std::error::Error + Send + Sync>),

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
        }
    }
}

impl GatewayError {
    /// Wrap a transport error, hiding its concrete type from the public API.
    #[must_use]
    pub(crate) fn upstream_transport(source: reqwest::Error) -> GatewayError {
        GatewayError::UpstreamTransport(Box::new(source))
    }

    /// Wrap a body-decode failure as a protocol error (not a transport error),
    /// preserving the cause via `source()`.
    #[must_use]
    pub(crate) fn upstream_protocol(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> GatewayError {
        GatewayError::UpstreamProtocol(Box::new(source))
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
            GatewayError::KindMismatch { .. } => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "kind_mismatch",
            ),
            GatewayError::ModelUnavailable(_) => (
                StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "model_unavailable",
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
            GatewayError::UpstreamProtocol(_) => {
                (StatusCode::BAD_GATEWAY, "server_error", "upstream_protocol")
            }
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
                GatewayError::ModelUnavailable("m".to_owned()),
                (
                    StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "model_unavailable",
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
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn upstream_protocol_is_502_and_not_a_transport_error() {
        let error = GatewayError::upstream_protocol(std::io::Error::other("bad json"));
        assert_eq!(
            error.classify(),
            (StatusCode::BAD_GATEWAY, "server_error", "upstream_protocol")
        );
        // Must not be a transport error, so a decode failure never triggers a
        // local child respawn (UP-004, UPSTREAM-003).
        assert!(!matches!(error, GatewayError::UpstreamTransport(_)));
        assert!(error.source().is_some());
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
}
