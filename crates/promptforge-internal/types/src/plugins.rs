//! The Plugin identity vocabulary: [`PluginId`] and its parse
//! error, plus [`Prelude`], the Lua source an activated Plugin hands
//! the Engine.
//!
//! A Plugin is the activation unit: code that runs at run setup and
//! makes services available to the run. Plugins ship in crates and are
//! identified by a 2-segment [`GlobalName`] - kind is encoded by arity, so
//! a Plugin id is `namespace/plugin` and every tool it contributes sits
//! under `namespace/plugin/name`. The Engine knows Plugins by identity alone:
//! a prompt declares them, an exact tool slot names one through its
//! [`ToolId`] prefix, and a [`ToolDescriptor`](crate::tools::ToolDescriptor)
//! records the conflicts of the Plugin that contributed it. Activating a
//! Plugin is the Harness's job; the Engine never activates anything.

use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::tools::ToolId;

#[cfg(test)]
#[path = "plugins-tests.rs"]
mod tests;

/// The stable identity of an installed Plugin.
///
/// A Plugin id is a [`GlobalName`] with exactly 2 segments
/// (`namespace/plugin`). The segment count tells what a name refers to: 2
/// segments name a Plugin and 3 name a tool. A Plugin's id is the
/// prefix of every tool id it contributes: `promptforge/web` contributes
/// `promptforge/web/fetch`. An id is the name alone, and a name resolves to
/// the only installed Plugin of that name. A `@` version marker in the
/// id is a parse error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct PluginId(GlobalName);

impl PluginId {
    /// Parses a Plugin identity, requiring exactly 2 segments
    /// (`namespace/plugin`).
    ///
    /// # Errors
    /// Returns [`PluginIdError`] when the id has fewer or more than 2
    /// segments ([`PluginIdErrorKind::SegmentCount`]), a segment is empty
    /// ([`PluginIdErrorKind::Empty`]), or a segment contains a character
    /// other than a lowercase ASCII letter, a digit, `-`, `_`, or `.`
    /// ([`PluginIdErrorKind::Control`]).
    pub fn parse(id: &str) -> Result<PluginId, PluginIdError> {
        let name =
            GlobalName::parse(id).map_err(|e| PluginIdError::from_global_name_kind(e.kind()))?;
        if name.segments().len() != 2 {
            return Err(PluginIdError {
                kind: PluginIdErrorKind::SegmentCount,
                reason: "a Plugin id must have exactly 2 segments (namespace/plugin)",
            });
        }
        Ok(PluginId(name))
    }

    /// Builds an identity from a 2-segment prefix split off a validated
    /// tool id.
    ///
    /// Crate-internal: backs [`crate::tools::ToolId::plugin`]. The
    /// source tool id was validated at parse, so its first two segments
    /// are already a valid Plugin id.
    pub(crate) fn from_prefix(prefix: GlobalName) -> PluginId {
        debug_assert!(
            prefix.segments().len() == 2,
            "a tool id's Plugin prefix must have exactly 2 segments (namespace/plugin)"
        );
        PluginId(prefix)
    }

    /// Returns the namespace segment.
    ///
    /// A namespace is meant to be a reverse-DNS name such as
    /// `org.rustalliance`, or `promptforge` for first-party Plugins.
    /// `PluginId::parse` accepts any valid segment as the namespace.
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.namespace()
    }

    /// Returns the Plugin segment.
    #[must_use]
    pub fn name(&self) -> &str {
        self.0.plugin()
    }

    /// Returns whether `tool` belongs to this Plugin.
    ///
    /// A tool belongs to a Plugin when dropping the last segment of the
    /// tool's id leaves exactly the Plugin's id, so `namespace/plugin/name`
    /// belongs to `namespace/plugin`. Every tool a Plugin contributes
    /// belongs to it. The caller must check this when it assembles the run's
    /// catalog.
    #[must_use]
    pub fn contains(&self, tool: &ToolId) -> bool {
        tool.plugin() == *self
    }
}

impl std::fmt::Display for PluginId {
    /// The canonical `namespace/plugin` string form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for PluginId {
    /// Serializes the id as a single `namespace/plugin` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for PluginId {
    /// Deserializes the id from its `namespace/plugin` string and validates it
    /// the same way `PluginId::parse` does. An invalid string fails
    /// deserialization.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        PluginId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A stable classification of a [`PluginIdError`] that callers can match
/// on to handle each kind of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginIdErrorKind {
    /// The id had fewer or more than 2 segments (`namespace/plugin`).
    SegmentCount,
    /// A segment was empty.
    Empty,
    /// A segment contained a character other than a lowercase ASCII letter,
    /// a digit, `-`, `_`, or `.`.
    Control,
}

/// The reason a [`PluginId`] failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid Plugin id: {reason}")]
#[non_exhaustive]
pub struct PluginIdError {
    /// A stable classification of why the id was rejected.
    kind: PluginIdErrorKind,
    /// A human-readable reason.
    reason: &'static str,
}

impl PluginIdError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> PluginIdErrorKind {
        self.kind
    }

    /// Maps a global-name rejection onto the Plugin-id error vocabulary.
    fn from_global_name_kind(global_kind: GlobalNameErrorKind) -> PluginIdError {
        let (kind, reason) = match global_kind {
            GlobalNameErrorKind::SegmentCount => (
                PluginIdErrorKind::SegmentCount,
                "a Plugin id must have exactly 2 segments (namespace/plugin)",
            ),
            GlobalNameErrorKind::Empty => (PluginIdErrorKind::Empty, "segments must not be empty"),
            GlobalNameErrorKind::Control => (
                PluginIdErrorKind::Control,
                "segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
            ),
        };
        PluginIdError { kind, reason }
    }
}

/// Lua source that an activated Plugin adds to the Lua VM of every
/// section in a run.
///
/// A prelude defines tables and functions, such as `sh.run(script)`, that
/// call the Plugin's own tools through `tools.call`. The Engine runs
/// the source knowing only the Plugin's id, which names the prelude in
/// tracebacks and error messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prelude {
    /// The Plugin that contributed the source.
    plugin: PluginId,
    /// The Lua source, compiled from text in every section VM.
    source: String,
}

impl Prelude {
    /// Pairs a Plugin's id with the prelude source it contributes.
    #[must_use]
    pub fn new(plugin: PluginId, source: impl Into<String>) -> Prelude {
        Prelude {
            plugin,
            source: source.into(),
        }
    }

    /// Returns the id of the Plugin that contributed this prelude.
    #[must_use]
    pub fn plugin(&self) -> &PluginId {
        &self.plugin
    }

    /// Returns the prelude's Lua source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }
}
