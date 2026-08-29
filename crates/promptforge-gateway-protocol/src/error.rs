//! Protocol-level error types: upstream transport/protocol failures and
//! explicit teardown failures.
//!
//! [`ProtocolError`] is the error every [`crate::upstream::Upstream`] method
//! returns; the gateway wraps it in its own route-level error type and renders
//! both through the same OpenAI error envelope mapping ([`ProtocolError::classify`],
//! [`ProtocolError::envelope`]).

/// A failure at the upstream seam: reaching a backend, decoding its reply, or
/// declining a workload the upstream cannot serve.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    /// The upstream backend could not be reached after the request may
    /// have left the gateway (a mid-flight read or timeout failure). The
    /// provider may have received and billed it, so it is not safe to
    /// retry blindly.
    #[error("upstream transport error")]
    UpstreamTransport(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The connection to the upstream backend itself failed (refused,
    /// DNS, TLS handshake): the request never left the gateway, nothing
    /// was billed, and a retry is safe.
    ///
    /// Distinct from [`ProtocolError::UpstreamTransport`], where the
    /// request may have reached the provider. A timeout is never connect:
    /// it may have reached the provider.
    #[error("upstream connect error")]
    UpstreamConnect(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The upstream returned a success status but a body that could not be
    /// decoded into the expected shape.
    ///
    /// Distinct from [`ProtocolError::UpstreamTransport`] so a decode failure
    /// (a protocol problem) never masquerades as a transport death and triggers
    /// a spurious local `llama-server` respawn (UP-004, UPSTREAM-003). The cause
    /// is preserved via `source()`.
    #[error("upstream protocol error")]
    UpstreamProtocol(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The upstream backend returned a non-success status.
    #[error("upstream returned {status}")]
    UpstreamStatus {
        /// The status code the backend returned.
        status: u16,
        /// The (truncated) upstream body, for diagnostics.
        body: String,
    },

    /// The resolved model's upstream cannot serve the route's workload (for
    /// example a local chat server asked for embeddings). The kind matches,
    /// but the backing upstream has no implementation for it.
    #[error("model {0} is not available for this workload")]
    ModelUnavailable(String),
}

impl ProtocolError {
    /// Wrap a reqwest failure, classifying it by where the request died.
    ///
    /// A connect failure (`err.is_connect()`) means the request never left
    /// the gateway and is classified [`ProtocolError::UpstreamConnect`];
    /// anything else - including every timeout, which may have reached the
    /// provider - stays [`ProtocolError::UpstreamTransport`].
    #[must_use]
    pub fn upstream_transport(source: reqwest::Error) -> ProtocolError {
        if source.is_connect() {
            ProtocolError::UpstreamConnect(Box::new(source))
        } else {
            ProtocolError::UpstreamTransport(Box::new(source))
        }
    }

    /// Wrap an already-classified mid-flight transport failure, preserving
    /// the cause via `source()`.
    ///
    /// The caller asserts the request may have reached the provider; for a
    /// reqwest failure whose class is unknown, use
    /// [`ProtocolError::upstream_transport`] instead.
    #[must_use]
    pub fn transport(source: impl std::error::Error + Send + Sync + 'static) -> ProtocolError {
        ProtocolError::UpstreamTransport(Box::new(source))
    }

    /// Wrap an already-classified connect failure, preserving the cause via
    /// `source()`.
    #[must_use]
    pub fn connect(source: impl std::error::Error + Send + Sync + 'static) -> ProtocolError {
        ProtocolError::UpstreamConnect(Box::new(source))
    }

    /// Wrap a body-decode failure as a protocol error (not a transport error),
    /// preserving the cause via `source()`.
    #[must_use]
    pub fn upstream_protocol(
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> ProtocolError {
        ProtocolError::UpstreamProtocol(Box::new(source))
    }

    /// Build a non-success-status failure from the upstream's status and
    /// truncated body.
    #[must_use]
    pub fn upstream_status(status: u16, body: String) -> ProtocolError {
        ProtocolError::UpstreamStatus { status, body }
    }

    /// The `(status, type, code)` triple for the OpenAI error envelope.
    #[must_use]
    pub fn classify(&self) -> (reqwest::StatusCode, &'static str, &'static str) {
        match self {
            ProtocolError::UpstreamTransport(_) => (
                reqwest::StatusCode::BAD_GATEWAY,
                "server_error",
                "upstream_transport",
            ),
            ProtocolError::UpstreamConnect(_) => (
                reqwest::StatusCode::BAD_GATEWAY,
                "server_error",
                "upstream_connect",
            ),
            ProtocolError::UpstreamProtocol(_) => (
                reqwest::StatusCode::BAD_GATEWAY,
                "server_error",
                "upstream_protocol",
            ),
            ProtocolError::UpstreamStatus { status, .. } => {
                let code = reqwest::StatusCode::from_u16(*status)
                    .unwrap_or(reqwest::StatusCode::BAD_GATEWAY);
                if code.is_client_error() {
                    (code, "invalid_request_error", "upstream_client_error")
                } else {
                    (
                        reqwest::StatusCode::BAD_GATEWAY,
                        "server_error",
                        "upstream_error",
                    )
                }
            }
            ProtocolError::ModelUnavailable(_) => (
                reqwest::StatusCode::BAD_REQUEST,
                "invalid_request_error",
                "model_unavailable",
            ),
        }
    }

    /// The OpenAI error envelope body for this error, shared by the JSON
    /// error response and the mid-stream SSE error event.
    #[must_use]
    pub fn envelope(&self) -> serde_json::Value {
        let (_, kind, code) = self.classify();
        serde_json::json!({
            "error": { "message": self.to_string(), "type": kind, "code": code }
        })
    }
}

/// A failure while explicitly releasing an upstream's owned resources.
///
/// Returned by [`crate::upstream::Upstream::shutdown`] when a child kill/reap
/// or capture-reader teardown fails, so a caller can refuse to proceed rather
/// than start replacements while an old child may survive.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ShutdownError {
    /// The upstream's teardown failed; the cause is preserved via `source()`.
    #[error("upstream teardown failed")]
    Teardown(#[source] Box<dyn std::error::Error + Send + Sync>),
}

impl ShutdownError {
    /// Wrap a teardown failure, preserving the cause via `source()`.
    #[must_use]
    pub fn teardown(source: impl std::error::Error + Send + Sync + 'static) -> ShutdownError {
        ShutdownError::Teardown(Box::new(source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::error::Error as _;

    #[test]
    fn protocol_error_classify_is_table_driven() {
        let cases: Vec<(ProtocolError, (reqwest::StatusCode, &str, &str))> = vec![
            (
                ProtocolError::ModelUnavailable("m".to_owned()),
                (
                    reqwest::StatusCode::BAD_REQUEST,
                    "invalid_request_error",
                    "model_unavailable",
                ),
            ),
            (
                ProtocolError::connect(std::io::Error::other("refused")),
                (
                    reqwest::StatusCode::BAD_GATEWAY,
                    "server_error",
                    "upstream_connect",
                ),
            ),
            (
                ProtocolError::transport(std::io::Error::other("reset")),
                (
                    reqwest::StatusCode::BAD_GATEWAY,
                    "server_error",
                    "upstream_transport",
                ),
            ),
            (
                ProtocolError::upstream_protocol(std::io::Error::other("bad json")),
                (
                    reqwest::StatusCode::BAD_GATEWAY,
                    "server_error",
                    "upstream_protocol",
                ),
            ),
            (
                ProtocolError::upstream_status(429, "rate limited".to_owned()),
                (
                    reqwest::StatusCode::TOO_MANY_REQUESTS,
                    "invalid_request_error",
                    "upstream_client_error",
                ),
            ),
            (
                ProtocolError::upstream_status(500, "exploded".to_owned()),
                (
                    reqwest::StatusCode::BAD_GATEWAY,
                    "server_error",
                    "upstream_error",
                ),
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(error.classify(), expected);
        }
    }

    #[test]
    fn upstream_protocol_is_502_and_not_a_transport_error() {
        let error = ProtocolError::upstream_protocol(std::io::Error::other("bad json"));
        assert_eq!(
            error.classify(),
            (
                reqwest::StatusCode::BAD_GATEWAY,
                "server_error",
                "upstream_protocol"
            )
        );
        // Must not be a transport error, so a decode failure never triggers a
        // local child respawn (UP-004, UPSTREAM-003).
        assert!(!matches!(error, ProtocolError::UpstreamTransport(_)));
        assert!(error.source().is_some());
    }

    #[test]
    fn envelope_carries_message_type_and_code() {
        let error = ProtocolError::upstream_status(503, "busy".to_owned());
        let envelope = error.envelope();
        assert_eq!(envelope["error"]["message"], "upstream returned 503");
        assert_eq!(envelope["error"]["type"], "server_error");
        assert_eq!(envelope["error"]["code"], "upstream_error");
    }

    #[test]
    fn shutdown_error_preserves_its_cause() {
        let error = ShutdownError::teardown(std::io::Error::other("kill failed"));
        assert_eq!(error.to_string(), "upstream teardown failed");
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("kill failed")
        );
    }
}
