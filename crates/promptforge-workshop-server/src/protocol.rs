//! The wire protocol of the workshop sockets: every JSON frame the server
//! exchanges with the UI, typed in one place, with zero I/O.
//!
//! Frames are grouped by direction - inbound (client to server) first,
//! outbound (server to client) second. Nothing here touches a socket, a
//! task, or a clock, so every wire shape is pinned by a plain unit test
//! below. The TypeScript half of this contract is
//! `ui/src/services/protocol.ts`; the two files cross-cite each other so a
//! shape change touches both or neither. The agent-session frame family is
//! additionally pinned by the shared fixture
//! `tests/fixtures/agent-frames.json`, asserted as the same JSON by the
//! fixture test below and by the SPA suite's
//! `ui/test/agent-wire-fixtures.mjs`, so drift on either side fails that
//! side's tests. The wire shapes are additionally frozen end to end by the
//! characterization tests in `tests/it`.
//!
//! # Inbound chat-socket frames
//!
//! `{"type":"chat","id":N,"model":"...","messages":[...]}` opens one
//! streaming completion: [`ChatRequest`] carries the fields forwarded
//! upstream, and the optional `id` names the chat on every frame of its
//! reply. `{"type":"cancel","id":N}` tears down the in-flight chat with
//! that id (the untagged chat when the id is absent): the upstream
//! completion is dropped and the tape records the abandonment, while
//! every other chat on the socket streams on. A cancel naming no live
//! chat is ignored, because a cancel racing its own `done` is normal.
//! A cancel gets no reply frame of any kind - no acknowledgment, and no
//! terminal `done` or `error` for the chat it tore down - so the client
//! settles the canceled chat locally when it sends the frame.
//!
//! A chat without an `id` occupies the single untagged slot, and a
//! second untagged chat sent while one is live is refused with an
//! id-less `error` frame. That refusal is indistinguishable on the wire
//! from a terminal error of the live untagged chat - both are
//! `{"type":"error","message":...}` with no `id` - so a client should
//! never run a second untagged chat; tag every concurrent chat instead.
//!
//! `{"type":"select_model","model":"..."}` selects the chat model: the
//! menu validates the id against the retained catalog and publishes a
//! fresh [`WorkbenchFrame`] on success; an unknown model is refused
//! with an `error` frame. `{"type":"switch_profile","name":"..."}`
//! starts a gateway profile switch: the pending snapshot publishes
//! immediately, stage progress arrives as [`StatusFrame`]s, and the
//! settled menu publishes a final [`WorkbenchFrame`] and a
//! [`CatalogFrame`]; a switch requested while one runs is refused with
//! an `error` frame. Both events may carry an optional `id`, echoed on
//! the `error` frame that refuses them, exactly as a chat's is.
//!
//! No inbound frame is pushed by the server, so none takes a delivery
//! classification; the reply frames they trigger are classified below.
//!
//! # Agent-session frames
//!
//! The `/agents/ws` socket speaks its own frame family. On connect the
//! server pushes [`AgentsFrame`], the discovered agent list. The client
//! opens a session with `{"type":"launch","agent":"..."}` or reattaches
//! with `{"type":"attach","session":"..."}`; either is answered with
//! [`AgentSessionFrame`] naming the session id. A running session streams
//! [`AgentEventFrame`]s - the durable event log, each frame carrying its
//! log index, replayed from the top on attach - and [`AgentDeltaFrame`]s,
//! the ephemeral live chunks, each stamped with the `reply` id of the
//! durable event that will supersede it. `{"type":"cancel"}` fires the
//! session's turn-cancel: cancellation is a stop reason, never an error -
//! no error frame follows, pending waits die as `input_cancelled`, and
//! the relaunched agent returns to waiting. Frames already in flight
//! from the cancelled run may still arrive between the cancel and the
//! relaunch: a defined grace window, absorbed by the reply-id
//! coalescing (the cancelled round never settles, so its deltas fall to
//! the round that eventually does), never a protocol violation.
//!
//! A session-level failure is pushed as an id-less [`ErrorFrame`]: a
//! model round that failed while the program survived it (the built-in
//! chat `pcall`s `models.chat` and returns to waiting), or a run that
//! ended in error. Delivery on this socket: ephemeral - the reports ride
//! a bounded broadcast beside the deltas and may drop under lag; the
//! durable transcript already shows the failed turn as one without a
//! reply, and terminal failures also land on the status bus.
//!
//! # Agent-session input frames
//!
//! An agent session asks its operator for input through the Workshop's
//! `user_input` tool. Three frames carry that conversation: the server
//! pushes [`InputFrame::Required`] when a wait opens and
//! [`InputFrame::Cancelled`] when one dies unresolved, and the client
//! answers with an `input_response` frame parsed as [`InputResponse`].
//! Both pushed frames are durable: the wait registry retains every
//! unresolved wait and the session resends it on reconnect, so a push
//! lost to a dead socket is repaired by the resent set - a live wait
//! reappears, and a stale prompt is dropped because its token is absent.
//! Cancellation is an explicit outcome, never silence: every path out of
//! an unresolved wait pushes `input_cancelled` for its token. The session
//! loops that route these frames arrive with agent sessions; the shapes
//! and classification are pinned here first.
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
//! gateway) is sent directly
//! by the loop that owns the socket, which delivers exactly without any
//! cursor - no shared state exists for a cursor to index.
//!
//! **Ephemeral** frames may drop under lag. They ride bounded channels
//! (a broadcast where the state fans out); a client too slow to drain
//! its channel lags out and its connection may drop. The drop is
//! harmless because every ephemeral frame has a repair path that owes
//! nothing to its predecessors: status and catalog are complete
//! snapshots resent on reconnect.
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
//!   reconnect, exactly like the catalog frame. The connect-time send -
//!   the retained snapshot follows the status and catalog snapshots on
//!   every new session, so the UI boots with zero HTTP state fetches -
//!   is that resend promise, not a third delivery class.
//!
//! Chat replies multiplex on one socket, and their ordering promise is
//! per chat: frames within one chat are strictly ordered - deltas in
//! stream order, the terminal `done` or `error` after every delta of its
//! chat - while distinct chats stream concurrently and interleave
//! freely, demuxed by the echoed `id`. The interleaving changes nothing
//! about the durable classification of the chat reply frames above.
//!
use serde::{Deserialize, Serialize};

pub use promptforge_gateway_protocol::wire::ChatRequest;

// --- Inbound: client to server -------------------------------------------

/// Parses an inbound chat body into the shared wire request.
///
/// The workshop, not the client, chooses streaming (`/chat` is buffered,
/// `/ws` streams), and the request forwarded upstream carries exactly
/// `model` and `messages`: the frame envelope (`type`, `id`), any
/// caller-sent `stream` flag, and every other field the gateway does not
/// name are dropped here, before the request is relayed.
pub(crate) fn parse_chat_request(
    mut value: serde_json::Value,
) -> Result<ChatRequest, serde_json::Error> {
    if let Some(object) = value.as_object_mut() {
        object.remove("stream");
    }
    let mut request: ChatRequest = serde_json::from_value(value)?;
    request.rest.clear();
    Ok(request)
}

// --- Agent-session frames --------------------------------------------------

/// The agent list pushed when an `/agents/ws` socket connects:
/// `{"type":"agents","agents":["chat","research"]}`.
///
/// Delivery: ephemeral - every push is the complete discovered list,
/// resent on every connect; there is no incremental form to lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentsFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The launchable agent names, in discovery order.
    agents: Vec<String>,
}

impl AgentsFrame {
    /// Builds the list frame over the discovered agent names.
    pub(crate) fn new(agents: Vec<String>) -> Self {
        Self {
            kind: "agents",
            agents,
        }
    }
}

/// The direct reply to a `launch` or `attach` frame:
/// `{"type":"agent_session","session":"...","agent":"..."}`. The client
/// keeps the session id to reattach after a disconnect - sessions
/// outlive sockets.
///
/// Delivery: durable - a direct per-request reply sent by the loop that
/// owns the socket, the contract's no-cursor case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentSessionFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The session's unguessable id.
    session: String,
    /// The launched agent's name.
    agent: String,
}

impl AgentSessionFrame {
    /// Builds the acknowledgment for `session` running `agent`.
    pub(crate) fn new(session: String, agent: String) -> Self {
        Self {
            kind: "agent_session",
            session,
            agent,
        }
    }
}

/// One durable entry of an agent session's event log:
/// `{"type":"agent_event","index":N,"event":{...}}` plus, on the
/// model-round content kinds (`agent_thought`, `agent_message`,
/// `tool_call`), the `reply` id that coalesces the round's ephemeral
/// deltas away (see [`AgentDeltaFrame`]).
///
/// Delivery: durable - `index` is the entry's position in the session's
/// event log, the per-client cursor recovers everything past it on
/// reconnect, and a future `replayFrom` cursor rides the same field.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct AgentEventFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The entry's log index.
    index: u64,
    /// The reply id this event settles, present on the model-round
    /// content kinds and omitted elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<u64>,
    /// The logged entry, in its persisted vocabulary shape.
    event: promptforge_core_support::events::RuntimeEvent,
}

impl AgentEventFrame {
    /// Builds the frame for the entry at `index`.
    pub(crate) fn new(
        index: u64,
        reply: Option<u64>,
        event: promptforge_core_support::events::RuntimeEvent,
    ) -> Self {
        Self {
            kind: "agent_event",
            index,
            reply,
            event,
        }
    }
}

/// Which streaming side channel one agent delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum AgentDeltaKind {
    /// Answer content, superseded by the round's `agent_message` event.
    Text,
    /// Reasoning content, superseded by the round's `agent_thought`
    /// event.
    Reasoning,
}

/// One live streaming chunk of an agent's model round:
/// `{"type":"agent_delta","kind":"text","content":"...","reply":N}`.
///
/// Every delta is stamped with the `reply` id of the durable event that
/// will supersede it, so the SPA coalesces chunks by that id and replaces
/// them when the event arrives (the ACP messageId chunk-vs-upsert rule).
///
/// Delivery: ephemeral - deltas ride a bounded broadcast and may drop
/// under lag; the completed-reply event is the repair path, which is why
/// agent deltas never enter the event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct AgentDeltaFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// Which side channel the chunk belongs to.
    #[serde(rename = "kind")]
    channel: AgentDeltaKind,
    /// The chunk's text.
    content: String,
    /// The id of the durable event that will supersede this delta.
    reply: u64,
}

impl AgentDeltaFrame {
    /// Builds a delta frame carrying `content` on `channel`, stamped with
    /// the superseding `reply` id.
    pub(crate) fn new(channel: AgentDeltaKind, content: String, reply: u64) -> Self {
        Self {
            kind: "agent_delta",
            channel,
            content,
            reply,
        }
    }
}

// --- Agent-session input frames -------------------------------------------

/// A pushed user-input lifecycle frame on an agent session's socket.
///
/// `{"type":"input_required","token":"..."}` announces an open wait: the
/// SPA pins its input box to the token and answers with an
/// `input_response` frame. `{"type":"input_cancelled","token":"..."}`
/// announces a wait that died unresolved, so the SPA never holds a
/// prompt against a dead token - cancellation is an outcome on the wire,
/// never silence.
///
/// Delivery: durable - the [`WaitRegistry`](crate::WaitRegistry) retains
/// every unresolved wait and the session resends it on reconnect, so a
/// push lost to a dead socket is repaired by the resent set: a live wait
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

// --- Outbound: server to client ------------------------------------------

/// One status bar update: what the bar should show right now.
///
/// Every update is a complete snapshot, so a lagging receiver loses nothing
/// by skipping intermediates. `label` is the short text rendered in the
/// status bar; `description` is the longer tooltip shown on hover.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StatusBarUpdate {
    /// Short text rendered in the status bar.
    pub label: String,
    /// Longer text shown as the bar's tooltip.
    pub description: String,
    /// Determinate progress, when the activity can report it.
    pub progress: Option<Progress>,
    /// How loudly the update speaks; the UI ignores `Debug` updates.
    pub severity: Severity,
    /// Which subsystem is active, driving the bar's activity indicator.
    pub activity: Activity,
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
pub struct Progress {
    /// Units completed so far.
    pub current: u64,
    /// Units expected in total.
    pub total: u64,
}

/// How loudly a status update speaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
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
pub enum Activity {
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
        // own fields and are stripped by `parse_chat_request`.
        let request = parse_chat_request(serde_json::json!({
            "type": "chat",
            "id": 7,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .expect("the chat frame parses");
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
    fn a_chat_request_drops_the_stream_flag_and_unnamed_fields() {
        // A caller-sent `stream` flag is ignored even when it is not a
        // boolean, and fields the gateway does not name never ride the
        // shared wire type's passthrough into the relayed body.
        let request = parse_chat_request(serde_json::json!({
            "type": "chat",
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
            "stream": "yes",
            "temperature": 0.5,
        }))
        .expect("a bogus stream flag is dropped, not an error");
        assert!(!request.stream);
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
    fn an_agents_frame_serializes_the_discovered_names() {
        let frame = serde_json::to_value(AgentsFrame::new(vec![
            "chat".to_owned(),
            "research".to_owned(),
        ]))
        .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({"type": "agents", "agents": ["chat", "research"]}),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn an_agent_session_frame_serializes_its_id_and_agent() {
        let frame =
            serde_json::to_value(AgentSessionFrame::new("a1b2".to_owned(), "chat".to_owned()))
                .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({"type": "agent_session", "session": "a1b2", "agent": "chat"}),
        );
    }

    #[test]
    fn an_agent_event_frame_carries_its_log_index_and_optional_reply_id() {
        use promptforge_core_support::events::{RuntimeEvent, RuntimeEventKind};
        let event = RuntimeEvent {
            kind: RuntimeEventKind::UserInput,
            section: "chat".to_owned(),
            chain_id: 0,
            depth: 0,
            turn: 0,
            content: "hi".to_owned(),
            model: None,
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        };
        let plain = serde_json::to_value(AgentEventFrame::new(3, None, event.clone()))
            .expect("the frame serializes");
        assert_eq!(plain["type"], "agent_event");
        assert_eq!(plain["index"], 3, "the frame carries the entry's log index");
        assert!(
            plain.get("reply").is_none(),
            "an absent reply id is omitted from the wire, not serialized as null"
        );
        assert_eq!(
            plain["event"],
            serde_json::to_value(&event).expect("events serialize"),
            "the entry rides in its persisted vocabulary shape"
        );
        let stamped = serde_json::to_value(AgentEventFrame::new(4, Some(1), event))
            .expect("the frame serializes");
        assert_eq!(
            stamped["reply"], 1,
            "a superseding event is stamped with the reply id its deltas carried"
        );
    }

    #[test]
    fn an_agent_delta_frame_is_stamped_with_its_superseding_reply_id() {
        let text = serde_json::to_value(AgentDeltaFrame::new(
            AgentDeltaKind::Text,
            "po".to_owned(),
            2,
        ))
        .expect("the frame serializes");
        assert_eq!(
            text,
            serde_json::json!({"type": "agent_delta", "kind": "text", "content": "po", "reply": 2}),
        );
        let reasoning = serde_json::to_value(AgentDeltaFrame::new(
            AgentDeltaKind::Reasoning,
            "hmm".to_owned(),
            2,
        ))
        .expect("the frame serializes");
        assert_eq!(
            reasoning,
            serde_json::json!({
                "type": "agent_delta", "kind": "reasoning", "content": "hmm", "reply": 2,
            }),
        );
    }

    #[test]
    fn an_input_required_frame_serializes_with_its_token() {
        let frame = serde_json::to_value(InputFrame::Required {
            token: "a1b2c3".to_owned(),
        })
        .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({"type": "input_required", "token": "a1b2c3"}),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn an_input_cancelled_frame_serializes_with_its_token() {
        let frame = serde_json::to_value(InputFrame::Cancelled {
            token: "a1b2c3".to_owned(),
        })
        .expect("the frame serializes");
        assert_eq!(
            frame,
            serde_json::json!({"type": "input_cancelled", "token": "a1b2c3"}),
            "the wire shape matches the chat protocol's frame taxonomy"
        );
    }

    #[test]
    fn an_input_response_parses_its_body_byte_exact_ignoring_the_envelope() {
        let gnarly = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash 🦀";
        let response: InputResponse = serde_json::from_value(serde_json::json!({
            "type": "input_response",
            "token": "a1b2c3",
            "text": gnarly,
        }))
        .expect("the frame parses with its envelope tag present");
        assert_eq!(response.token, "a1b2c3");
        assert_eq!(
            response.text, gnarly,
            "the operator's text survives the wire byte-exact"
        );
    }

    /// The shared agent-frame fixture, asserted as the same JSON by the SPA
    /// suite (`ui/test/agent-wire-fixtures.mjs`): a wire drift on either
    /// side fails that side's fixture test.
    const AGENT_FRAME_FIXTURE: &str = include_str!("../tests/fixtures/agent-frames.json");

    /// Parses the shared fixture into one object keyed by case name.
    fn agent_fixture() -> serde_json::Value {
        serde_json::from_str(AGENT_FRAME_FIXTURE).expect("the fixture is valid JSON")
    }

    /// The fixture's `agent_event_minimal` entry as the vocabulary type.
    fn minimal_fixture_event() -> promptforge_core_support::events::RuntimeEvent {
        use promptforge_core_support::events::{RuntimeEvent, RuntimeEventKind};
        RuntimeEvent {
            kind: RuntimeEventKind::UserInput,
            section: "chat".to_owned(),
            chain_id: 0,
            depth: 0,
            turn: 0,
            content: "hi".to_owned(),
            model: None,
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        }
    }

    /// The fixture's `agent_event_stamped` entry as the vocabulary type,
    /// every metrics section populated.
    fn stamped_fixture_event() -> promptforge_core_support::events::RuntimeEvent {
        use promptforge_core_support::events::{
            CallMetrics, ClientTiming, LlamaTimings, RuntimeEvent, RuntimeEventKind, Usage,
            VllmMetrics,
        };
        RuntimeEvent {
            kind: RuntimeEventKind::AssistantReply,
            section: "chat".to_owned(),
            chain_id: 1,
            depth: 0,
            turn: 2,
            content: "hello".to_owned(),
            model: Some("llama-3".to_owned()),
            tool_call_id: None,
            finish_reason: Some("stop".to_owned()),
            metrics: Some(CallMetrics {
                usage: Some(Usage {
                    prompt_tokens: 7,
                    completion_tokens: 3,
                    total_tokens: 10,
                    cached_tokens: Some(2),
                    reasoning_tokens: Some(1),
                }),
                llama: Some(LlamaTimings {
                    prompt_n: 7,
                    prompt_ms: 12.5,
                    prompt_per_second: 560.0,
                    predicted_n: 3,
                    predicted_ms: 30.5,
                    predicted_per_second: 98.5,
                    draft_n: 4,
                    draft_n_accepted: 2,
                }),
                vllm: Some(VllmMetrics {
                    time_to_first_token_ms: Some(8.5),
                    generation_time_ms: Some(22.5),
                    queue_time_ms: Some(1.5),
                    mean_itl_ms: Some(7.5),
                    tokens_per_second: Some(133.5),
                }),
                client: Some(ClientTiming {
                    ttft_ms: Some(9.5),
                    mean_itl_ms: Some(8.25),
                    e2e_ms: 41.5,
                }),
            }),
        }
    }

    #[test]
    fn the_shared_fixture_pins_exactly_the_agreed_case_list() {
        let fixture = agent_fixture();
        let mut cases: Vec<&str> = fixture
            .as_object()
            .expect("the fixture is one object keyed by case name")
            .keys()
            .map(String::as_str)
            .collect();
        cases.sort_unstable();
        assert_eq!(
            cases,
            [
                "agent_delta_reasoning",
                "agent_delta_text",
                "agent_event_minimal",
                "agent_event_stamped",
                "agent_session",
                "agents",
                "attach",
                "cancel",
                "input_cancelled",
                "input_required",
                "input_response",
                "launch",
            ],
            "both suites pin exactly the same case list, so a case added on \
             one side fails the other"
        );
    }

    #[test]
    fn server_to_client_agent_frames_match_the_shared_fixture() {
        // Each typed frame serializes to its fixture entry, compared as
        // values so key order in the file is free.
        let fixture = agent_fixture();
        assert_eq!(
            serde_json::to_value(AgentsFrame::new(vec![
                "chat".to_owned(),
                "research".to_owned()
            ]))
            .expect("the frame serializes"),
            fixture["agents"]
        );
        assert_eq!(
            serde_json::to_value(AgentSessionFrame::new("a1b2".to_owned(), "chat".to_owned()))
                .expect("the frame serializes"),
            fixture["agent_session"]
        );
        assert_eq!(
            serde_json::to_value(AgentEventFrame::new(3, None, minimal_fixture_event()))
                .expect("the frame serializes"),
            fixture["agent_event_minimal"]
        );
        assert_eq!(
            serde_json::to_value(AgentEventFrame::new(4, Some(1), stamped_fixture_event()))
                .expect("the frame serializes"),
            fixture["agent_event_stamped"],
            "the event rides in its persisted vocabulary shape, metrics and all"
        );
        assert_eq!(
            serde_json::to_value(AgentDeltaFrame::new(
                AgentDeltaKind::Text,
                "po".to_owned(),
                2
            ))
            .expect("the frame serializes"),
            fixture["agent_delta_text"]
        );
        assert_eq!(
            serde_json::to_value(AgentDeltaFrame::new(
                AgentDeltaKind::Reasoning,
                "hmm".to_owned(),
                2
            ))
            .expect("the frame serializes"),
            fixture["agent_delta_reasoning"]
        );
        assert_eq!(
            serde_json::to_value(InputFrame::Required {
                token: "a1b2c3".to_owned(),
            })
            .expect("the frame serializes"),
            fixture["input_required"]
        );
        assert_eq!(
            serde_json::to_value(InputFrame::Cancelled {
                token: "a1b2c3".to_owned(),
            })
            .expect("the frame serializes"),
            fixture["input_cancelled"]
        );
    }

    #[test]
    fn client_to_server_agent_frames_match_the_shared_fixture() {
        // `input_response` parses through its typed body; `launch`,
        // `attach`, and `cancel` are routed from raw JSON in
        // `session_agents::socket`, so the fixture pins exactly the fields
        // that routing reads.
        let fixture = agent_fixture();
        let response: InputResponse = serde_json::from_value(fixture["input_response"].clone())
            .expect("the fixture input_response parses");
        assert_eq!(response.token, "a1b2c3");
        assert_eq!(response.text, "two words");
        assert_eq!(fixture["launch"]["type"], "launch");
        assert_eq!(fixture["launch"]["agent"], "chat");
        assert_eq!(fixture["attach"]["type"], "attach");
        assert_eq!(fixture["attach"]["session"], "a1b2");
        assert_eq!(fixture["cancel"]["type"], "cancel");
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
}
