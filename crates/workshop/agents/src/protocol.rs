//! The conversation vocabulary the agent socket renders: ids, durable
//! transcript entries, and ephemeral deltas. Nothing here names the
//! socket's wire shape.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A conversation's unguessable id, minted by Workshop at launch. It is
/// also the name of the conversation's run. Conversations outlive socket
/// connections, so a client keeps the id to reattach after a disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ConversationId(String);

impl ConversationId {
    /// Wraps an already-minted id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// A fresh unguessable id: 128 bits from the OS-seeded cryptographic
    /// RNG, hex-encoded - wide enough that ids never collide across
    /// restarts, so a client's retained id never names a stranger's
    /// conversation.
    #[must_use]
    pub fn fresh() -> Self {
        use rand::Rng as _;
        let mut rng = rand::rng();
        Self(format!(
            "{:016x}{:016x}",
            rng.random::<u64>(),
            rng.random::<u64>()
        ))
    }

    /// The id as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ConversationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One durable entry of a conversation's transcript.
///
/// `index` is the entry's position in the transcript: every event the
/// conversation's run has reported, in log order, numbered from zero. A
/// client resumes past its last seen index on reconnect. `reply` is
/// present on a model round's content events and holds that round's id,
/// the id its [`Delta`]s carry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// The entry's transcript index.
    pub index: u64,
    /// The round id this event settles, present on the model-round
    /// content kinds and omitted elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<u64>,
    /// The logged Engine event, in its persisted shape.
    pub event: serde_json::Value,
}

/// Which streaming side channel one delta belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[non_exhaustive]
pub enum DeltaKind {
    /// Answer content, superseded by the round's reply event.
    Text,
    /// Reasoning content, superseded by the round's thinking event.
    Reasoning,
}

/// One live streaming chunk of a model round.
///
/// Every delta is stamped with its round's id as `reply`, the id the
/// durable [`SessionEvent`] that supersedes it carries, so a client
/// coalesces chunks by that id and replaces them when the event arrives.
/// Deltas are ephemeral: they may drop under lag, and the completed-reply
/// event is the repair path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Delta {
    /// Which side channel the chunk belongs to.
    pub kind: DeltaKind,
    /// The chunk's text.
    pub content: String,
    /// The id of the round whose durable event will supersede this delta.
    pub reply: u64,
}
