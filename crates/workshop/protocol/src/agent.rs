//! Agent-session frames: the `/agents/ws` socket's frame family.

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
pub struct AgentEventFrame {
    #[serde(rename = "type")]
    kind: &'static str,
    /// The entry's log index.
    index: u64,
    /// The reply id this event settles, present on the model-round
    /// content kinds and omitted elsewhere.
    #[serde(skip_serializing_if = "Option::is_none")]
    reply: Option<u64>,
    /// The logged entry, in its persisted vocabulary shape.
    event: promptforge_api_types::events::RuntimeEvent,
}

impl AgentEventFrame {
    /// Builds the frame for the entry at `index`.
    #[must_use]
    pub fn new(
        index: u64,
        reply: Option<u64>,
        event: promptforge_api_types::events::RuntimeEvent,
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
/// Delivery: ephemeral - deltas ride a bounded broadcast and may drop
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
    /// Builds a delta frame carrying `content` on `channel`, stamped with
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
