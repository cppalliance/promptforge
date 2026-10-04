//! The capability identity vocabulary: [`CapabilityId`] and its parse
//! error, plus [`Prelude`], the Lua source an activated capability hands
//! the Engine.
//!
//! A capability is the activation unit: code that runs at run setup and
//! makes services available to the run. Capabilities are delivered in packs
//! (crates now, DLLs via adapters later) and identified by a 2-segment
//! [`GlobalName`] - kind is encoded by arity, so a capability id is
//! `namespace/pack` and every tool it contributes sits under
//! `namespace/pack/name`. The Engine knows capabilities by identity alone:
//! a prompt declares them, an exact tool slot names one through its
//! [`ToolId`] prefix, and a [`ToolDescriptor`](crate::tools::ToolDescriptor)
//! records the conflicts of the capability that contributed it. The
//! activation contract - the `Capability` trait, the services it is handed,
//! and the contribution it returns - is the Harness's, in
//! `harness-capabilities`; the Engine never activates anything.

use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::tools::ToolId;

#[cfg(test)]
#[path = "capabilities-tests.rs"]
mod tests;

/// The stable identity of an installed capability.
///
/// A capability id is a [`GlobalName`] with exactly 2 segments
/// (`namespace/pack`). The segment count tells what a name refers to: 2
/// segments name a capability and 3 name a tool. A capability's id is the
/// prefix of every tool id it contributes: `promptforge/web` contributes
/// `promptforge/web/fetch`. An id is the name alone, and a name resolves to
/// the only installed capability of that name. A `@` version marker in the
/// id is a parse error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct CapabilityId(GlobalName);

impl CapabilityId {
    /// Parses a capability identity, requiring exactly 2 segments
    /// (`namespace/pack`).
    ///
    /// # Errors
    /// Returns [`CapabilityIdError`] when the id has fewer or more than 2
    /// segments ([`CapabilityIdErrorKind::SegmentCount`]), a segment is empty
    /// ([`CapabilityIdErrorKind::Empty`]), or a segment contains a character
    /// other than a lowercase ASCII letter, a digit, `-`, `_`, or `.`
    /// ([`CapabilityIdErrorKind::Control`]).
    pub fn parse(id: &str) -> Result<CapabilityId, CapabilityIdError> {
        let name = GlobalName::parse(id)
            .map_err(|e| CapabilityIdError::from_global_name_kind(e.kind()))?;
        if name.segments().len() != 2 {
            return Err(CapabilityIdError {
                kind: CapabilityIdErrorKind::SegmentCount,
                reason: "a capability id must have exactly 2 segments (namespace/pack)",
            });
        }
        Ok(CapabilityId(name))
    }

    /// Builds an identity from a 2-segment prefix split off a validated
    /// tool id.
    ///
    /// Crate-internal: backs [`crate::tools::ToolId::capability`]. The
    /// source tool id was validated at parse, so its first two segments
    /// are already a valid capability id.
    pub(crate) fn from_prefix(prefix: GlobalName) -> CapabilityId {
        debug_assert!(
            prefix.segments().len() == 2,
            "a tool id's capability prefix must have exactly 2 segments (namespace/pack)"
        );
        CapabilityId(prefix)
    }

    /// Returns the namespace segment.
    ///
    /// A namespace is meant to be a reverse-DNS name such as
    /// `org.rustalliance`, or `promptforge` for first-party capabilities.
    /// `CapabilityId::parse` accepts any valid segment as the namespace.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.namespace()
    }

    /// Returns the pack segment.
    #[must_use]
    pub fn pack(&self) -> &str {
        self.0.pack()
    }

    /// Returns whether `tool` belongs to this capability.
    ///
    /// A tool belongs to a capability when dropping the last segment of the
    /// tool's id leaves exactly the capability's id, so `namespace/pack/name`
    /// belongs to `namespace/pack`. Every tool a capability contributes
    /// belongs to it. The caller must check this when it assembles the run's
    /// catalog.
    #[must_use]
    pub fn contains(&self, tool: &ToolId) -> bool {
        tool.capability() == *self
    }
}

impl std::fmt::Display for CapabilityId {
    /// The canonical `namespace/pack` string form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for CapabilityId {
    /// Serializes the id as a single `namespace/pack` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for CapabilityId {
    /// Deserializes the id from its `namespace/pack` string and validates it
    /// the same way `CapabilityId::parse` does. An invalid string fails
    /// deserialization.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        CapabilityId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A stable classification of a [`CapabilityIdError`] that callers can match
/// on to handle each kind of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityIdErrorKind {
    /// The id had fewer or more than 2 segments (`namespace/pack`).
    SegmentCount,
    /// A segment was empty.
    Empty,
    /// A segment contained a character other than a lowercase ASCII letter,
    /// a digit, `-`, `_`, or `.`.
    Control,
}

/// The reason a [`CapabilityId`] failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid capability id: {reason}")]
#[non_exhaustive]
pub struct CapabilityIdError {
    /// A stable classification of why the id was rejected.
    kind: CapabilityIdErrorKind,
    /// A human-readable reason.
    reason: &'static str,
}

impl CapabilityIdError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> CapabilityIdErrorKind {
        self.kind
    }

    /// Maps a global-name rejection onto the capability-id error vocabulary.
    fn from_global_name_kind(global_kind: GlobalNameErrorKind) -> CapabilityIdError {
        let (kind, reason) = match global_kind {
            GlobalNameErrorKind::SegmentCount => (
                CapabilityIdErrorKind::SegmentCount,
                "a capability id must have exactly 2 segments (namespace/pack)",
            ),
            GlobalNameErrorKind::Empty => {
                (CapabilityIdErrorKind::Empty, "segments must not be empty")
            }
            GlobalNameErrorKind::Control => (
                CapabilityIdErrorKind::Control,
                "segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
            ),
        };
        CapabilityIdError { kind, reason }
    }
}

/// Lua source that an activated capability adds to the Lua VM of every
/// section in a run.
///
/// A prelude defines tables and functions, such as `sh.run(script)`, that
/// call the capability's own tools through `tools.call`. The Engine runs
/// the source knowing only the capability's id, which names the prelude in
/// tracebacks and error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prelude {
    /// The capability that contributed the source.
    capability: CapabilityId,
    /// The Lua source, compiled from text in every section VM.
    source: String,
}

impl Prelude {
    /// Pairs a capability's id with the prelude source it contributes.
    #[must_use]
    pub fn new(capability: CapabilityId, source: impl Into<String>) -> Prelude {
        Prelude {
            capability,
            source: source.into(),
        }
    }

    /// Returns the id of the capability that contributed this prelude.
    #[must_use]
    pub fn capability(&self) -> &CapabilityId {
        &self.capability
    }

    /// Returns the prelude's Lua source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}
