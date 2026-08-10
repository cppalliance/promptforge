//! Opt-in raw model-turn capture.
//!
//! [`DebugCapture`] receives owned request and response payloads for a host
//! that wants them on disk or in a debugger. It is a separate seam from
//! [`crate::observe::Observer`]: observations stay payload-free, and production
//! hosts leave [`crate::execute::RunConfig::debug`] unset so they pay
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
/// # Ordering and delivery
/// Both events for a turn are delivered only after the model round trip
/// succeeds. [`on_event`](Self::on_event) is then called synchronously from the
/// task driving the run, in turn order, with the [`DebugEvent::Request`]
/// delivered before its matching [`DebugEvent::Response`]. A turn whose round
/// trip does not complete - a transport error, cancellation, or a response the
/// client rejects - emits neither event, so a capture records only completed
/// turns and never a lone request.
///
/// Implementations must return promptly (copy into a queue rather than blocking
/// on I/O) and must not panic; a panic unwinds the run.
///
/// # Examples
/// A nonblocking capture copies each event into an in-memory queue and handles
/// events forward-compatibly. [`DebugEvent`] and its variants are
/// `#[non_exhaustive]`, so a wildcard arm is required:
///
/// ```
/// use std::sync::Mutex;
/// use promptforge_core::debug::{DebugCapture, DebugEvent};
///
/// #[derive(Default)]
/// struct QueueCapture {
///     turns: Mutex<Vec<(String, u32)>>,
/// }
///
/// impl DebugCapture for QueueCapture {
///     fn on_event(&self, _execution: &str, section: &str, turn_index: u32, event: DebugEvent) {
///         // Copy into an in-memory queue; never block on I/O on this path.
///         let kind = match event {
///             DebugEvent::Request { .. } => "request",
///             DebugEvent::Response { .. } => "response",
///             _ => "other",
///         };
///         self.turns.lock().unwrap().push((format!("{section}:{kind}"), turn_index));
///     }
/// }
///
/// let capture = QueueCapture::default();
/// capture.on_event("run", "Say hi", 1, DebugEvent::request(serde_json::Value::Null));
/// capture.on_event("run", "Say hi", 1, DebugEvent::response(serde_json::Value::Null, None, None));
/// let turns = capture.turns.lock().unwrap();
/// assert_eq!(turns.as_slice(), &[("Say hi:request".to_owned(), 1), ("Say hi:response".to_owned(), 1)]);
/// ```
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

impl DebugEvent {
    /// Builds a [`DebugEvent::Request`] from a serialized request `body`.
    ///
    /// The variants are `#[non_exhaustive]` so fields can be added compatibly;
    /// these constructors are the stable way for a host (or its tests) to build
    /// an event without depending on the variant's exact field set.
    #[must_use]
    pub fn request(body: Value) -> DebugEvent {
        DebugEvent::Request { body }
    }

    /// Builds a [`DebugEvent::Response`] from a response `body` and its parsed
    /// `finish_reason`/`reasoning_content` metadata.
    #[must_use]
    pub fn response(
        body: Value,
        finish_reason: Option<String>,
        reasoning_content: Option<String>,
    ) -> DebugEvent {
        DebugEvent::Response {
            body,
            finish_reason,
            reasoning_content,
        }
    }
}
