//! Agent-session input frames: the operator-input conversation.

use serde::{Deserialize, Serialize};

/// A pushed user-input lifecycle frame on an agent session's socket.
///
/// `{"type":"input_required","token":"..."}` announces an open wait: the
/// SPA pins its input box to the token and answers with an
/// `input_response` frame. `{"type":"input_cancelled","token":"..."}`
/// announces a wait that died unresolved, so the SPA never holds a
/// prompt against a dead token - cancellation is an outcome on the wire,
/// never silence.
///
/// Delivery: durable - the server's wait registry retains every
/// unresolved wait and the session resends it on reconnect, so a push
/// lost to a dead socket is repaired by the resent set: a live wait
/// reappears, and a cancelled one vanishes by its absence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "type")]
pub enum InputFrame {
    /// A wait opened: the session wants operator input for `token`.
    #[serde(rename = "input_required")]
    Required {
        /// The single-use wait token an `input_response` must echo.
        token: String,
    },
    /// A wait died unresolved: the prompt for `token` is stale.
    #[serde(rename = "input_cancelled")]
    Cancelled {
        /// The token whose wait is gone.
        token: String,
    },
}

/// The inbound answer to an [`InputFrame::Required`] prompt:
/// `{"type":"input_response","token":"...","text":"..."}`.
///
/// The session routes on the envelope's `type` and deserializes the body
/// with serde, which ignores the envelope tag itself. `text` is the
/// operator's input, byte-exact as typed. Like every inbound frame it
/// takes no delivery classification, because the server pushes none.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct InputResponse {
    /// The wait token this response answers.
    pub token: String,
    /// The operator's text, byte-exact as typed.
    pub text: String,
}
