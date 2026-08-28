//! The wire protocol of the workshop sockets: every JSON frame the server
//! exchanges with the UI, typed in one place, with zero I/O.
//!
//! Frames are grouped by direction - inbound (client to server) first,
//! outbound (server to client) second. Nothing here touches a socket, a
//! task, or a clock, so every wire shape is pinned by a plain unit test
//! below. The TypeScript half of this contract is
//! `ui/src/services/protocol.ts`; the two files cross-cite each other so a
//! shape change touches both or neither. The wire shapes are additionally
//! frozen end to end by the characterization tests in `tests/it`.
//!
//! # Delivery contract
//!
//! Every frame the server pushes carries exactly one of two delivery
//! semantics. The session loops are built on this classification, so no
//! pushed frame type ships unclassified.
//!
//! **Durable** frames are delivered exactly, and coalesce. Where the
//! data is shared fan-out state, the producer records it and wakes each
//! connection loop through a `Notify`; the loop compares the shared
//! revision against its own per-client cursor and sends everything past
//! the cursor, so a missed wakeup is harmless because the next one
//! delivers everything past the cursor. A durable frame that answers
//! the connection's own request (the chat reply stream relayed from the
//! gateway, the voice announcements and stop replies) is sent directly
//! by the loop that owns the socket, which delivers exactly without any
//! cursor - no shared state exists for a cursor to index.
//!
//! **Ephemeral** frames may drop under lag. They ride bounded channels
//! (a broadcast where the state fans out); a client too slow to drain
//! its channel lags out and its connection may drop. The drop is
//! harmless because every ephemeral frame has a repair path that owes
//! nothing to its predecessors: status and catalog are complete
//! snapshots resent on reconnect, and a voice interim is superseded by
//! the next interim of its take.
//!
//! ## Classification
//!
//! Chat socket (`/ws`):
//!
//! - [`DeltaFrame`] - durable. One chunk of a chat reply in flight; a
//!   dropped chunk is a hole in the transcript no later frame repairs.
//! - [`ReasoningFrame`] - durable. The same transcript stream on the
//!   reasoning side channel; chunks are append-only and irreplaceable.
//! - [`DoneFrame`] - durable. The stream's terminal marker; dropping it
//!   leaves the client's chat in flight forever.
//! - [`ErrorFrame`] - durable. A terminal transcript outcome like
//!   `done`; dropping it leaves the chat unresolved.
//! - [`StatusFrame`] - ephemeral. Every update is a complete snapshot of
//!   the bar, so a lagging client loses nothing by skipping
//!   intermediates, and the current status is resent on reconnect.
//! - [`CatalogFrame`] - ephemeral. Each push carries the whole catalog
//!   verbatim; the newest push supersedes every older one and the
//!   catalog is resent on reconnect.
//! - [`WorkbenchFrame`] - ephemeral. Every push is a complete snapshot
//!   of the server-owned Model-menu state, retained and resent on
//!   reconnect, exactly like the catalog frame.
//!
//! Voice socket (`/voice`):
//!
//! - [`StreamFrame`] - durable. The single per-take generation
//!   announcement; every interim and final frame that follows refers to
//!   it and nothing resends it.
//! - [`InterimFrame`] - ephemeral. Each interim supersedes the previous
//!   one - the committed prefix plus a fresh tentative decode - so a
//!   dropped interim is overwritten by the next.
//! - [`FinalFrame`] - durable. The take's single stop reply carrying the
//!   assembled transcript; it has no successor and is never resent.

use serde::{Deserialize, Serialize};

// --- Inbound: client to server -------------------------------------------

/// A non-streaming chat completion request forwarded to the gateway.
///
/// This is the body accepted by the workshop's `POST /chat` and sent
/// upstream to `POST /v1/chat/completions`; on `/ws` the same fields
/// arrive inside a `{"type":"chat",...}` frame.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChatRequest {
    /// The model name from the gateway catalog.
    pub model: String,
    /// OpenAI chat messages, relayed without inspecting their shape.
    pub messages: Vec<serde_json::Value>,
}

/// The `/voice` control message that begins a take.
pub(crate) const VOICE_START: &str = "start";

/// The `/voice` control message that ends a take and requests its final
/// transcript.
pub(crate) const VOICE_STOP: &str = "stop";

// --- Outbound: server to client ------------------------------------------

/// One status bar update: what the bar should show right now.
///
/// Every update is a complete snapshot, so a lagging receiver loses nothing
/// by skipping intermediates. `label` is the short text rendered in the
/// status bar; `description` is the longer tooltip shown on hover.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct StatusBarUpdate {
    /// Short text rendered in the status bar.
    pub(crate) label: String,
    /// Longer text shown as the bar's tooltip.
    pub(crate) description: String,
    /// Determinate progress, when the activity can report it.
    pub(crate) progress: Option<Progress>,
    /// How loudly the update speaks; the UI ignores `Debug` updates.
    pub(crate) severity: Severity,
    /// Which subsystem is active, driving the bar's activity indicator.
    pub(crate) activity: Activity,
}

impl StatusBarUpdate {
    /// The update as a wire frame: its own fields plus `"type": "status"`.
    pub(crate) fn frame(&self) -> StatusFrame<'_> {
        StatusFrame {
            kind: "status",
            update: self,
        }
    }
}

/// A determinate progress report for the status bar's progress slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct Progress {
    /// Units completed so far.
    pub(crate) current: u64,
    /// Units expected in total.
    pub(crate) total: u64,
}

/// How loudly a status update speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Severity {
    /// User-visible status text.
    Info,
    /// Internal instrumentation; the UI ignores it for display.
    Debug,
    /// A failure the user should see.
    Error,
}

/// The subsystem an update belongs to, driving the activity indicator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Activity {
    /// No specific subsystem; the activity LED stays dark.
    General,
    /// A model turn in flight: amber on the activity LED.
    Thinking,
    /// Output tokens arriving: green on the activity LED.
    Generating,
}

/// The serialized shape of one update on the socket: the update's fields
/// flattened beside `"type": "status"`, matching the chat protocol's frame
/// taxonomy.
///
/// Delivery: ephemeral - every update is a complete snapshot, so a
/// lagging client skips intermediates and the current status is resent
/// on reconnect.
#[derive(Debug, Serialize)]
pub(crate) struct StatusFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    #[serde(flatten)]
    update: &'a StatusBarUpdate,
}

/// One pushed model catalog.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CatalogPush {
    /// The gateway's `/v1/models` `data` array, verbatim.
    pub(crate) models: Vec<serde_json::Value>,
}

impl CatalogPush {
    /// The push as a wire frame: `"type": "models"` beside the array.
    pub(crate) fn frame(&self) -> CatalogFrame<'_> {
        CatalogFrame {
            kind: "models",
            models: &self.models,
        }
    }
}

/// The serialized shape of a catalog push on the socket, matching the chat
/// protocol's frame taxonomy.
///
/// Delivery: ephemeral - the newest push carries the whole catalog and
/// supersedes every older one; the catalog is resent on reconnect.
#[derive(Debug, Serialize)]
pub(crate) struct CatalogFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    models: &'a [serde_json::Value],
}

/// One pushed workbench snapshot: the server-owned Model-menu state.
///
/// The server computes `chat_ready` - catalog non-empty, a model
/// selected, no switch in flight, gateway reachable - and the UI never
/// derives it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct WorkbenchSnapshot {
    /// Every gateway profile name, in gateway order.
    pub(crate) profiles: Vec<String>,
    /// The profile the gateway is serving, once known.
    pub(crate) active: Option<String>,
    /// The profile a switch is loading, while one is in flight.
    pub(crate) switching: Option<String>,
    /// The model chat requests go to, once one is selected.
    pub(crate) selected_model: Option<String>,
    /// Whether a chat can be submitted right now.
    pub(crate) chat_ready: bool,
}

impl WorkbenchSnapshot {
    /// The snapshot as a wire frame: `"type": "workbench"` beside the
    /// fields, with `selected_model` shortened to `selected` on the wire.
    // An `allow` rather than an `expect`: the wire-shape test below uses
    // this in test builds, so an expectation would be unfulfilled there
    // and fail the -D warnings gate.
    #[allow(
        dead_code,
        reason = "the /ws session loop serializes the frame in a later step"
    )]
    pub(crate) fn frame(&self) -> WorkbenchFrame<'_> {
        WorkbenchFrame {
            kind: "workbench",
            profiles: &self.profiles,
            active: self.active.as_deref(),
            switching: self.switching.as_deref(),
            selected: self.selected_model.as_deref(),
            chat_ready: self.chat_ready,
        }
    }
}

/// The serialized shape of a workbench push on the socket, matching the
/// chat protocol's frame taxonomy. Absent options serialize as `null`,
/// never as omitted keys: every push is the complete menu state.
///
/// Delivery: ephemeral - every push is a complete snapshot of the menu
/// state, retained and resent on reconnect, exactly like the catalog
/// frame.
#[derive(Debug, Serialize)]
pub(crate) struct WorkbenchFrame<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    profiles: &'a [String],
    active: Option<&'a str>,
    switching: Option<&'a str>,
    selected: Option<&'a str>,
    chat_ready: bool,
}

/// One streamed answer-content chunk of a chat reply:
/// `{"type":"delta","content":"..."}` plus the echoed request `id`.
///
/// Delivery: durable - a transcript chunk of a chat in flight; a dropped
/// chunk is a hole in the reply no later frame repairs.
#[derive(Debug, Serialize)]
pub(crate) struct DeltaFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    content: String,
    /// The request's `id`, echoed verbatim when it carried one and omitted
    /// from the wire when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
}

impl DeltaFrame {
    /// Builds a delta frame carrying `content`, echoing `id` when present.
    pub(crate) fn new(content: String, id: Option<&serde_json::Value>) -> Self {
        Self {
            kind: "delta",
            content,
            id: id.cloned(),
        }
    }
}

/// One chunk of the model's reasoning side channel, rendered by the UI as
/// the Thinking block: `{"type":"reasoning","content":"..."}` plus the
/// echoed request `id`.
///
/// Delivery: durable - a transcript chunk on the reasoning side channel;
/// chunks are append-only and irreplaceable.
#[derive(Debug, Serialize)]
pub(crate) struct ReasoningFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    content: String,
    /// The request's `id`, echoed verbatim when it carried one and omitted
    /// from the wire when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
}

impl ReasoningFrame {
    /// Builds a reasoning frame carrying `content`, echoing `id` when
    /// present.
    pub(crate) fn new(content: String, id: Option<&serde_json::Value>) -> Self {
        Self {
            kind: "reasoning",
            content,
            id: id.cloned(),
        }
    }
}

/// The terminal frame of a completed chat stream: `{"type":"done"}` plus
/// the echoed request `id`.
///
/// Delivery: durable - the stream's terminal marker; dropping it leaves
/// the client's chat in flight forever.
#[derive(Debug, Serialize)]
pub(crate) struct DoneFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The request's `id`, echoed verbatim when it carried one and omitted
    /// from the wire when it did not.
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<serde_json::Value>,
}

impl DoneFrame {
    /// Builds the terminal frame, echoing `id` when present.
    pub(crate) fn new(id: Option<&serde_json::Value>) -> Self {
        Self {
            kind: "done",
            id: id.cloned(),
        }
    }
}

/// A chat failure report - transport, mid-stream, or a declined stream:
/// `{"type":"error","message":"..."}` plus the echoed request `id`.
///
/// Delivery: durable - a terminal transcript outcome; dropping it leaves
/// the chat unresolved.
#[derive(Debug, Serialize)]
pub(crate) struct ErrorFrame {
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
    pub(crate) fn new(message: String, id: Option<&serde_json::Value>) -> Self {
        Self {
            kind: "error",
            message,
            id: id.cloned(),
        }
    }
}

/// The `/voice` stream announcement: `{"type":"stream","generation":N}`,
/// sent when a `start` begins a new stream generation and before any of
/// that generation's interim or final frames. Generations count from 1
/// per connection, so the client can discard frames a stop/restart race
/// left behind from a superseded take.
///
/// Delivery: durable - the single per-take generation announcement that
/// every following interim and final frame refers to; nothing resends it.
#[derive(Debug, Serialize)]
pub(crate) struct StreamFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    generation: u64,
}

impl StreamFrame {
    /// Builds the announcement for `generation`.
    pub(crate) fn new(generation: u64) -> Self {
        Self {
            kind: "stream",
            generation,
        }
    }
}

/// One interim transcription push on `/voice`:
/// `{"type":"interim","committed":"...","tentative":"...","generation":N}`.
/// `committed` is the take's crystallized prefix (append-only within a
/// take), `tentative` is the interim model's decode of the audio past it,
/// and `generation` names the announced stream generation the frame
/// belongs to.
///
/// Delivery: ephemeral - each interim supersedes the previous one, so a
/// dropped interim is overwritten by the next.
#[derive(Debug, Serialize)]
pub(crate) struct InterimFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    committed: String,
    tentative: String,
    generation: u64,
}

impl InterimFrame {
    /// Builds an interim frame from the take's two transcript fields,
    /// tagged with the take's stream generation.
    pub(crate) fn new(committed: String, tentative: String, generation: u64) -> Self {
        Self {
            kind: "interim",
            committed,
            tentative,
            generation,
        }
    }
}

/// The take's single stop reply on `/voice`:
/// `{"type":"final","text":"...","frames":N,"generation":N}` - the
/// assembled transcript, the total PCM frames received since the most
/// recent start, and the announced stream generation the take belongs to.
///
/// Delivery: durable - the take's single stop reply; it has no successor
/// and is never resent.
#[derive(Debug, Serialize)]
pub(crate) struct FinalFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
    frames: u64,
    generation: u64,
}

impl FinalFrame {
    /// Builds the stop reply from the transcript and the frame count,
    /// tagged with the take's stream generation.
    pub(crate) fn new(text: String, frames: u64, generation: u64) -> Self {
        Self {
            kind: "final",
            text,
            frames,
            generation,
        }
    }
}

// Every test below pins one frame's wire shape against the exact JSON
// literal the pre-refactor code built with `serde_json::json!`, so a field
// rename, retype, or optionality change fails here before it reaches a
// socket.
#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal update with the given label.
    fn stub(label: impl Into<String>) -> StatusBarUpdate {
        StatusBarUpdate {
            label: label.into(),
            description: String::new(),
            progress: None,
            severity: Severity::Info,
            activity: Activity::General,
        }
    }

    #[test]
    fn a_status_update_serializes_as_a_status_frame() {
        let frame = serde_json::to_value(stub("Ready").frame()).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "status",
                "label": "Ready",
                "description": "",
                "progress": null,
                "severity": "info",
                "activity": "general",
            }),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn progress_and_the_remaining_variants_serialize() {
        let update = StatusBarUpdate {
            progress: Some(Progress {
                current: 1,
                total: 2,
            }),
            severity: Severity::Error,
            activity: Activity::Thinking,
            ..stub("Working")
        };
        let frame = serde_json::to_value(update.frame()).expect("the frame serializes");
        assert_eq!(
            frame["progress"],
            serde_json::json!({"current": 1, "total": 2})
        );
        assert_eq!(frame["severity"], "error");
        assert_eq!(frame["activity"], "thinking");
        // Debug serializes too; the UI, not the bus, ignores it.
        let debug = serde_json::to_value(
            StatusBarUpdate {
                severity: Severity::Debug,
                activity: Activity::Generating,
                ..stub("x")
            }
            .frame(),
        )
        .expect("the frame serializes");
        assert_eq!(debug["severity"], "debug");
        assert_eq!(debug["activity"], "generating");
    }

    #[test]
    fn a_catalog_push_serializes_as_a_models_frame() {
        let push = CatalogPush {
            models: vec![serde_json::json!({"id": "test-model", "object": "model"})],
        };
        let frame = serde_json::to_value(push.frame()).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "models",
                "models": [{"id": "test-model", "object": "model"}],
            }),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn a_workbench_snapshot_serializes_as_a_workbench_frame() {
        let snapshot = WorkbenchSnapshot {
            profiles: vec!["main".to_string(), "coding".to_string()],
            active: Some("main".to_string()),
            switching: None,
            selected_model: Some("claude-sonnet-4-6".to_string()),
            chat_ready: true,
        };
        let frame = serde_json::to_value(snapshot.frame()).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "workbench",
                "profiles": ["main", "coding"],
                "active": "main",
                "switching": null,
                "selected": "claude-sonnet-4-6",
                "chat_ready": true,
            }),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn a_chat_request_round_trips_the_wire_shapes() {
        // The `/ws` chat frame: `type` and `id` ride beside the request's
        // own fields and are ignored by the deserializer.
        let request: ChatRequest = serde_json::from_value(serde_json::json!({
            "type": "chat",
            "id": 7,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .expect("the chat frame deserializes");
        assert_eq!(request.model, "test-model");
        // The upstream body: exactly the two fields, nothing added.
        assert_eq!(
            serde_json::to_value(&request).expect("the request serializes"),
            serde_json::json!({
                "model": "test-model",
                "messages": [{"role": "user", "content": "ping"}],
            })
        );
    }

    #[test]
    fn a_delta_frame_serializes_with_and_without_the_echoed_id() {
        let untagged = serde_json::to_value(DeltaFrame::new("po".to_string(), None))
            .expect("the frame serializes");
        assert_eq!(
            untagged,
            serde_json::json!({"type": "delta", "content": "po"}),
            "an absent id is omitted from the wire, not serialized as null"
        );
        let id = serde_json::json!(1);
        let tagged = serde_json::to_value(DeltaFrame::new("po".to_string(), Some(&id)))
            .expect("the frame serializes");
        assert_eq!(
            tagged,
            serde_json::json!({"type": "delta", "content": "po", "id": 1})
        );
    }

    #[test]
    fn a_reasoning_frame_serializes_with_and_without_the_echoed_id() {
        let untagged = serde_json::to_value(ReasoningFrame::new("hmm ".to_string(), None))
            .expect("the frame serializes");
        assert_eq!(
            untagged,
            serde_json::json!({"type": "reasoning", "content": "hmm "})
        );
        let id = serde_json::json!(1);
        let tagged = serde_json::to_value(ReasoningFrame::new("hmm ".to_string(), Some(&id)))
            .expect("the frame serializes");
        assert_eq!(
            tagged,
            serde_json::json!({"type": "reasoning", "content": "hmm ", "id": 1})
        );
    }

    #[test]
    fn a_done_frame_serializes_with_and_without_the_echoed_id() {
        let untagged = serde_json::to_value(DoneFrame::new(None)).expect("the frame serializes");
        assert_eq!(untagged, serde_json::json!({"type": "done"}));
        let id = serde_json::json!(2);
        let tagged = serde_json::to_value(DoneFrame::new(Some(&id))).expect("the frame serializes");
        assert_eq!(tagged, serde_json::json!({"type": "done", "id": 2}));
    }

    #[test]
    fn an_error_frame_serializes_with_and_without_the_echoed_id() {
        let untagged =
            serde_json::to_value(ErrorFrame::new("Gateway unreachable".to_string(), None))
                .expect("the frame serializes");
        assert_eq!(
            untagged,
            serde_json::json!({"type": "error", "message": "Gateway unreachable"})
        );
        let id = serde_json::json!(7);
        let tagged = serde_json::to_value(ErrorFrame::new(
            "Gateway unreachable".to_string(),
            Some(&id),
        ))
        .expect("the frame serializes");
        assert_eq!(
            tagged,
            serde_json::json!({"type": "error", "message": "Gateway unreachable", "id": 7})
        );
    }

    #[test]
    fn a_stream_frame_serializes_its_generation() {
        let frame = serde_json::to_value(StreamFrame::new(3)).expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "stream",
                "generation": 3,
            })
        );
    }

    #[test]
    fn an_interim_frame_serializes_both_transcript_fields() {
        let frame = serde_json::to_value(InterimFrame::new(
            "ask not".to_string(),
            "what you".to_string(),
            1,
        ))
        .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "interim",
                "committed": "ask not",
                "tentative": "what you",
                "generation": 1,
            })
        );
    }

    #[test]
    fn a_final_frame_serializes_the_transcript_and_the_frame_count() {
        let frame = serde_json::to_value(FinalFrame::new(String::new(), 192, 2))
            .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({
                "type": "final",
                "text": "",
                "frames": 192,
                "generation": 2,
            })
        );
    }

    #[test]
    fn the_voice_control_messages_are_bare_words() {
        assert_eq!(VOICE_START, "start");
        assert_eq!(VOICE_STOP, "stop");
    }
}
