//! Error shapes on the wire: the socket `error` frame and the HTTP error
//! envelope.

use serde::Serialize;

/// A failure report answered to one inbound frame - a malformed frame, a
/// refused menu event, or an agent-session failure:
/// `{"type":"error","message":"..."}` plus the echoed request `id`.
///
/// Delivery: durable on the workshop socket - a direct per-request reply
/// sent by the loop that owns the socket. The agent-session socket
/// additionally pushes id-less error frames for session-level failures;
/// that delivery is ephemeral and documented in the agent-session
/// section of the crate docs.
#[derive(Debug, Serialize)]
pub struct ErrorFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    message: String,
    /// The request's `id`, echoed verbatim when it carried one and omitted
    /// from the wire when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
}

impl ErrorFrame {
    /// Builds an error frame carrying `message`, echoing `id` when present.
    #[must_use]
    pub fn new(message: String, id: Option<&serde_json::Value>) -> Self {
        Self {
            kind: "error",
            message,
            id: id.cloned(),
        }
    }
}

/// The opaque wire error envelope every HTTP failure answers with:
/// `{"error":{"message":"...","code":"..."}}`.
///
/// The shell maps its per-crate error types onto status codes and renders
/// this envelope; the shape is pinned here so the wire contract lives in
/// one place. Failures rendered as plain text (the asset 404) never take
/// this shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorEnvelope {
    error: EnvelopeBody,
}

/// The envelope payload: the user-visible message and the
/// machine-readable code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EnvelopeBody {
    message: String,
    code: String,
}

impl ErrorEnvelope {
    /// Builds the envelope carrying `message` under the machine-readable
    /// `code`.
    #[must_use]
    pub fn new(message: impl Into<String>, code: impl Into<String>) -> Self {
        Self {
            error: EnvelopeBody {
                message: message.into(),
                code: code.into(),
            },
        }
    }
}
