//! Agent-session frames: the `/agents/ws` socket's frame family, typed
//! beside the socket that is their only producer.
//!
//! On connect the server pushes [`AgentsFrame`], the discovered agent
//! list. The client opens a session with `{"type":"launch","agent":"..."}`
//! or reattaches with `{"type":"attach","session":"..."}`; either is
//! answered with [`AgentSessionFrame`] naming the session id. A running
//! session streams [`AgentEventFrame`]s - the durable event log, each
//! frame holding its log index, replayed from the top on attach - and
//! [`AgentDeltaFrame`]s, the ephemeral live chunks, each stamped with the
//! `reply` id of the durable event that will supersede it.
//! `{"type":"cancel"}` fires the session's turn-cancel: cancellation is a
//! stop reason, never an error - no error frame follows, pending waits die
//! as `input_cancelled`, and the relaunched agent returns to waiting.
//! Frames already in flight from the cancelled run may still arrive
//! between the cancel and the relaunch: a defined grace window, absorbed
//! by the reply-id coalescing (the cancelled round never settles, so its
//! deltas fall to the round that eventually does), never a protocol
//! violation. The three client requests parse as [`SessionRequest`].
//!
//! A session-level failure is pushed as an id-less
//! [`ErrorFrame`](workshop_protocol::ErrorFrame): a model round that
//! failed while the program survived it (the built-in chat `pcall`s
//! `models.chat` and returns to waiting), or a run that ended in error.
//! Delivery on this socket: ephemeral - the reports are sent on a bounded
//! broadcast beside the deltas and may drop under lag; the durable
//! transcript already shows the failed turn as one without a reply, and
//! terminal failures also land on the status bus. The input-wait frames
//! stay in `workshop-protocol`.
//!
//! The TypeScript half of this contract is
//! `crates/workshop/ui/src/services/protocol.ts`; the two files
//! cross-cite each other so a shape change touches both or neither.
//! Every shape is pinned by this module's tests and by the shared fixture
//! `tests/fixtures/agent-frames.json`, asserted as the same JSON by the
//! SPA suite's `crates/workshop/ui/test/agent-wire-fixtures.mjs`, so drift
//! on either side fails that side's tests.

use promptforge::event::Event;
use promptforge::metrics::{CallMetrics, ToolCallEvent};
use serde::{Deserialize, Serialize};

/// A session request the client sends: `launch`, `attach`, or `cancel`.
/// Fields beyond the ones a variant names are ignored.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum SessionRequest {
    /// `{"type":"launch","agent":"..."}` starts a session running `agent`.
    Launch {
        /// The discovered agent to run.
        agent: String,
    },
    /// `{"type":"attach","session":"..."}` reattaches to a running
    /// session.
    Attach {
        /// The id an earlier [`AgentSessionFrame`] named.
        session: String,
    },
    /// `{"type":"cancel"}` fires the attached session's turn-cancel.
    Cancel,
}

/// Why an inbound frame is not a [`SessionRequest`]. Each variant renders
/// as the socket's refusal text, so serde's own error text never reaches
/// the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum RequestRefusal {
    /// A `launch` frame whose `agent` is absent or not a string.
    #[error("launch frame without an agent name")]
    LaunchWithoutAgent,
    /// An `attach` frame whose `session` is absent or not a string.
    #[error("attach frame without a session id")]
    AttachWithoutSession,
    /// A frame whose `type` names no request. The socket routes
    /// `input_response` before parsing, so the text lists it too.
    #[error(
        "unknown frame type; expected \"launch\", \"attach\", \"input_response\", \
         or \"cancel\""
    )]
    UnknownType,
}

impl SessionRequest {
    /// Parses one inbound frame. Only a JSON object carries a `type`, so
    /// anything else is an unknown type.
    ///
    /// # Errors
    /// Returns the [`RequestRefusal`] naming what the frame lacks.
    pub(crate) fn parse(frame: &serde_json::Value) -> Result<Self, RequestRefusal> {
        let refusal = match frame.get("type").and_then(serde_json::Value::as_str) {
            Some("launch") => RequestRefusal::LaunchWithoutAgent,
            Some("attach") => RequestRefusal::AttachWithoutSession,
            Some("cancel") => RequestRefusal::UnknownType,
            _ => return Err(RequestRefusal::UnknownType),
        };
        Self::deserialize(frame).map_err(|_| refusal)
    }
}

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
    #[must_use]
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
    #[must_use]
    pub(crate) fn new(session: String, agent: String) -> Self {
        Self {
            kind: "agent_session",
            session,
            agent,
        }
    }
}

/// The kind of one [`AgentEvent`] on the wire, labelled with the Agent
/// Client Protocol `sessionUpdate` names so the SPA's transcript stays
/// ACP-conversant:
///
/// | Variant | Label |
/// |---|---|
/// | [`AssistantReply`](Self::AssistantReply) | `agent_message` |
/// | [`AssistantToolCalls`](Self::AssistantToolCalls) | `tool_call` |
/// | [`ToolResult`](Self::ToolResult) | `tool_call_update` |
/// | [`Thinking`](Self::Thinking) | `agent_thought` |
/// | [`UserInput`](Self::UserInput) | `user_message` |
///
/// Exactly the engine's content [`Event`] variants a transcript renders;
/// lifecycle, task, and debug events never frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[non_exhaustive]
pub(crate) enum AgentEventKind {
    /// A completed assistant reply.
    #[serde(rename = "agent_message")]
    AssistantReply,
    /// A batch of tool calls the model requested.
    #[serde(rename = "tool_call")]
    AssistantToolCalls,
    /// The result of one dispatched tool call.
    #[serde(rename = "tool_call_update")]
    ToolResult,
    /// A completed block of model thinking.
    #[serde(rename = "agent_thought")]
    Thinking,
    /// Text the user supplied.
    #[serde(rename = "user_message")]
    UserInput,
}

/// One content [`Event`] in the shape the `/agents/ws` wire uses: the
/// ACP-labelled `kind`, the reporting `section`, the model-turn counter,
/// the kind-specific `content` string (a tool-call batch renders as the
/// JSON array of its calls), and the model, tool-call id, finish reason,
/// and metrics where the kind includes them. `content` and every other
/// free-text field is untrusted model-, tool-, or user-authored data. An
/// [`Event`] locates itself by its provenance (the task and sequence),
/// which the wire does not yet expose.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub(crate) struct AgentEvent {
    /// What kind of thing happened.
    pub(crate) kind: AgentEventKind,
    /// The reporting scope: the agent's name.
    pub(crate) section: String,
    /// The model-turn counter the event was reported under.
    pub(crate) turn: u32,
    /// The kind-specific untrusted payload.
    pub(crate) content: String,
    /// The model that produced the event, for model-attributed kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    /// The provider-issued tool-call id the event answers to, for tool
    /// results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) tool_call_id: Option<String>,
    /// The provider's finish reason, when it sent one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) finish_reason: Option<String>,
    /// Everything measured about the model call that produced the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) metrics: Option<CallMetrics>,
}

impl AgentEvent {
    /// Projects one engine event onto the wire shape, or `None` for a
    /// variant the transcript does not render (lifecycle, task, and debug
    /// events).
    #[must_use]
    pub(crate) fn from_event(event: &Event) -> Option<Self> {
        let base = |kind: AgentEventKind, turn: u32, content: String| AgentEvent {
            kind,
            section: event.section().to_owned(),
            turn,
            content,
            model: None,
            tool_call_id: None,
            finish_reason: None,
            metrics: None,
        };
        Some(match event {
            Event::UserInput { text, .. } => base(AgentEventKind::UserInput, 0, text.clone()),
            Event::Thinking {
                turn, model, text, ..
            } => AgentEvent {
                model: Some(model.clone()),
                ..base(AgentEventKind::Thinking, *turn, text.clone())
            },
            Event::AssistantReply {
                turn,
                text,
                finish_reason,
                model,
                metrics,
                ..
            } => AgentEvent {
                model: Some(model.clone()),
                finish_reason: finish_reason.clone(),
                metrics: metrics.clone(),
                ..base(AgentEventKind::AssistantReply, *turn, text.clone())
            },
            Event::AssistantToolCalls {
                turn, model, calls, ..
            } => AgentEvent {
                model: Some(model.clone()),
                ..base(
                    AgentEventKind::AssistantToolCalls,
                    *turn,
                    render_tool_calls(calls),
                )
            },
            Event::ToolResult {
                turn,
                tool_call_id,
                content,
                ..
            } => AgentEvent {
                tool_call_id: Some(tool_call_id.clone()),
                ..base(AgentEventKind::ToolResult, *turn, content.clone())
            },
            _ => return None,
        })
    }
}

/// Renders a tool-call batch as the JSON array of its calls, so a reader
/// parses the ids, names, and arguments back out of one string field.
/// The calls hold only strings and JSON values, so serialization cannot
/// fail; the fallback keeps the projection total.
fn render_tool_calls(calls: &[ToolCallEvent]) -> String {
    serde_json::to_string(calls).unwrap_or_else(|_| "[]".to_owned())
}

/// One durable entry of an agent session's event log:
/// `{"type":"agent_event","index":N,"event":{...}}` plus, on the
/// model-round content kinds (`agent_thought`, `agent_message`,
/// `tool_call`), the `reply` id that coalesces the round's ephemeral
/// deltas away (see [`AgentDeltaFrame`]).
///
/// Delivery: durable - `index` is the entry's position in the session's
/// event log, the per-client cursor recovers everything past it on
/// reconnect, and a future `replayFrom` cursor is stored in the same field.
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
    /// The logged entry, in its wire shape.
    event: AgentEvent,
}

impl AgentEventFrame {
    /// Builds the frame for the entry at `index`, or `None` when `event`
    /// is a variant the transcript does not render.
    #[must_use]
    pub(crate) fn new(index: u64, reply: Option<u64>, event: &Event) -> Option<Self> {
        Some(Self {
            kind: "agent_event",
            index,
            reply,
            event: AgentEvent::from_event(event)?,
        })
    }
}

/// Which streaming side channel one agent delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
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
/// Delivery: ephemeral - deltas are sent on a bounded broadcast and may drop
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
    /// Builds a delta frame holding `content` on `channel`, stamped with
    /// the superseding `reply` id.
    #[must_use]
    pub(crate) fn new(channel: AgentDeltaKind, content: String, reply: u64) -> Self {
        Self {
            kind: "agent_delta",
            channel,
            content,
            reply,
        }
    }
}

#[cfg(test)]
#[path = "wire-tests.rs"]
mod tests;
