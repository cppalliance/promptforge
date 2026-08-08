//! Opt-in raw model-turn capture.
//!
//! [`DebugCapture`] receives owned request and response payloads for a host
//! that wants them on disk or in a debugger. It is a separate seam from
//! [`crate::observe::Observer`]: observations stay payload-free, and production
//! hosts leave [`crate::execute::RunOptions::debug`] as `None` so they pay
//! nothing for this path.

use serde_json::Value;

/// An opt-in sink for raw model-turn payloads.
///
/// Implementations own any synchronization they need. The runtime never consults
/// a capture for a decision; dropping every event cannot change the result.
pub trait DebugCapture: Send + Sync {
    /// Receives one capture event for a model turn.
    ///
    /// `turn_index` is the 1-based model-turn number within the run (the same
    /// counter advanced for [`crate::observe::detail::MODEL_TURN_COMPLETED`]).
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
}

/// One owned capture payload for a model turn.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DebugEvent {
    /// The JSON body sent to the gateway's chat-completions endpoint.
    Request {
        /// The serialized request body.
        body: Value,
    },
    /// The JSON body returned by the gateway, with parsed metadata.
    Response {
        /// The raw response body.
        body: Value,
        /// The choice's `finish_reason`, when the backend supplied one.
        finish_reason: Option<String>,
        /// The message's `reasoning_content`, when the backend supplied one.
        reasoning_content: Option<String>,
    },
}
