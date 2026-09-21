//! Agent-session frames: the `/agents/ws` socket's frame family.

use promptforge_api_types::event::Event;
use promptforge_api_types::metrics::{CallMetrics, ToolCallEvent};
use serde::Serialize;

/// The agent list pushed when an `/agents/ws` socket connects:
/// `{"type":"agents","agents":["chat","research"]}`.
///
/// Delivery: ephemeral - every push is the complete discovered list,
/// resent on every connect; there is no incremental form to lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentsFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The launchable agent names, in discovery order.
    agents: Vec<String>,
}

impl AgentsFrame {
    /// Builds the list frame over the discovered agent names.
    #[must_use]
    pub fn new(agents: Vec<String>) -> Self {
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
pub struct AgentSessionFrame {
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
    pub fn new(session: String, agent: String) -> Self {
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
pub enum AgentEventKind {
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
pub struct AgentEvent {
    /// What kind of thing happened.
    pub kind: AgentEventKind,
    /// The reporting scope: the agent's name.
    pub section: String,
    /// The model-turn counter the event was reported under.
    pub turn: u32,
    /// The kind-specific untrusted payload.
    pub content: String,
    /// The model that produced the event, for model-attributed kinds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// The provider-issued tool-call id the event answers to, for tool
    /// results.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// The provider's finish reason, when it sent one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub finish_reason: Option<String>,
    /// Everything measured about the model call that produced the event.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<CallMetrics>,
}

impl AgentEvent {
    /// Projects one engine event onto the wire shape, or `None` for a
    /// variant the transcript does not render (lifecycle, task, and debug
    /// events).
    #[must_use]
    pub fn from_event(event: &Event) -> Option<Self> {
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
pub struct AgentEventFrame {
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
    pub fn new(index: u64, reply: Option<u64>, event: &Event) -> Option<Self> {
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
pub enum AgentDeltaKind {
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
pub struct AgentDeltaFrame {
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
    pub fn new(channel: AgentDeltaKind, content: String, reply: u64) -> Self {
        Self {
            kind: "agent_delta",
            channel,
            content,
            reply,
        }
    }
}
