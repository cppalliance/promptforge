//! The session vocabulary clients speak and render: ids, launch requests,
//! durable events, and ephemeral deltas. `harness` re-exports every
//! type here; nothing in it names a client's wire shape.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A session's unguessable id. Sessions outlive client connections, so a
/// client keeps the id to reattach after a disconnect.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(String);

impl SessionId {
    /// Wraps an already-minted id.
    #[must_use]
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// A fresh unguessable id: 128 bits from the OS-seeded cryptographic
    /// RNG, hex-encoded - wide enough that ids never collide across
    /// restarts, so a client's retained id never names a stranger's
    /// session.
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

impl fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// What a client asks the harness to launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchRequest {
    /// The agent's name, as discovered under the configured agents path.
    pub agent: String,
    /// The run's argument text, handed to the prompt as its input.
    #[serde(default)]
    pub args: String,
}

/// One durable entry of a session's event log.
///
/// `index` is the entry's position in the session's transcript: the
/// events of every run the session has made, in log order, numbered from
/// zero and continuing across a relaunch. A client resumes past its last
/// seen index on reconnect. `reply` is present on the model-round content
/// events and names the id whose [`Delta`]s this event supersedes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// The entry's transcript index.
    pub index: u64,
    /// The reply id this event settles, present on the model-round
    /// content kinds and omitted elsewhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<u64>,
    /// The logged engine event, in its persisted shape.
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
/// Every delta is stamped with the `reply` id of the durable
/// [`SessionEvent`] that will supersede it, so a client coalesces chunks
/// by that id and replaces them when the event arrives. Deltas are
/// ephemeral: they may drop under lag, and the completed-reply event is
/// the repair path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Delta {
    /// Which side channel the chunk belongs to.
    pub kind: DeltaKind,
    /// The chunk's text.
    pub content: String,
    /// The id of the durable event that will supersede this delta.
    pub reply: u64,
}
