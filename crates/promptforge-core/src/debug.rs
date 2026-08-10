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
///
/// # Sensitivity
/// A [`DebugEvent`] carries the verbatim request and response bodies, including
/// the full prompt, model output, tool arguments and results, and any store
/// contents that reached the turn. It is raw, unredacted capture: a host that
/// persists it owns treating it as sensitive. The bearer credential is never
/// part of a body (it rides an HTTP header the client never captures).
///
/// # Ordering and blocking
/// [`on_event`](Self::on_event) is called synchronously from the task driving
/// the run, in turn order, with the request delivered before its matching
/// response. Implementations must return promptly (copy into a queue rather than
/// blocking on I/O) and must not panic; a panic unwinds the run.
pub trait DebugCapture: Send + Sync {
    /// Receives one capture event for a model turn.
    ///
    /// `turn_index` is the 1-based model-turn number within the run. See the
    /// trait-level sensitivity, ordering, and non-blocking contract.
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
}

/// One owned capture payload for a model turn.
///
/// The `serde_json::Value` bodies are the intentional raw-capture wire contract:
/// a debug sink wants exactly what crossed the wire, not a re-typed view.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum DebugEvent {
    /// The JSON body sent to the gateway's chat-completions endpoint.
    #[non_exhaustive]
    Request {
        /// The serialized request body.
        body: Value,
    },
    /// The JSON body returned by the gateway, with parsed metadata.
    #[non_exhaustive]
    Response {
        /// The raw response body.
        body: Value,
        /// The choice's `finish_reason`, when the backend supplied one.
        finish_reason: Option<String>,
        /// The message's `reasoning_content`, when the backend supplied one.
        reasoning_content: Option<String>,
    },
}
