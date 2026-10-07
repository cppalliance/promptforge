//! The Plugin identity vocabulary: [`PluginId`] and its parse
//! error, plus [`Prelude`], the Lua source a declared Plugin hands
//! the Engine.
//!
//! A Plugin ships in a crate and is installed under a local, one-segment
//! [`PluginId`], such as `web`, and every tool it offers sits under that
//! name, such as `web/fetch`. The Engine knows Plugins by identity alone:
//! a prompt declares them by plain name, which makes each one required
//! and runs its prelude, and an exact tool slot names one through its
//! [`ToolId`] prefix. A run's catalog holds the tools of every Plugin the
//! caller can serve, declared or not. The Plugin contract - the `Package`
//! label, the `Plugin` trait, and the services a Plugin reads - is in
//! `promptforge-plugin`, and installing Plugins is the caller's; the
//! Engine never builds a Plugin.

use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::tools::ToolId;

#[cfg(test)]
#[path = "plugins-tests.rs"]
mod tests;

/// The local name a Plugin is installed under: one segment, such as `web`.
///
/// A Plugin's id is the first segment of every tool id it contributes:
/// `web` contributes `web/fetch`. An id is the name alone, and a name
/// resolves to the only installed Plugin of that name. A `@` version
/// marker in the id is a parse error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct PluginId(GlobalName);

impl PluginId {
    /// Parses a Plugin identity, requiring exactly one segment.
    ///
    /// # Errors
    /// Returns [`PluginIdError`] when the id has more than one segment
    /// ([`PluginIdErrorKind::SegmentCount`]), is empty or has an empty
    /// segment ([`PluginIdErrorKind::Empty`]), or a segment contains a
    /// character other than a lowercase ASCII letter, a digit, `-`, `_`,
    /// or `.` ([`PluginIdErrorKind::Control`]).
    pub fn parse(name: &str) -> Result<PluginId, PluginIdError> {
        let name =
            GlobalName::parse(name).map_err(|e| PluginIdError::from_global_name_kind(e.kind()))?;
        if name.segments().len() != 1 {
            return Err(PluginIdError {
                kind: PluginIdErrorKind::SegmentCount,
                reason: "a Plugin id is one segment, such as `web`, with no '/'",
            });
        }
        Ok(PluginId(name))
    }

    /// Builds an identity from the first segment of a validated tool id.
    ///
    /// Crate-internal: backs [`crate::tools::ToolId::plugin`]. The source
    /// tool id was validated at parse, so its first segment is already a
    /// valid Plugin id.
    pub(crate) fn from_segment(segment: GlobalName) -> PluginId {
        debug_assert!(
            segment.segments().len() == 1,
            "a tool id's Plugin is exactly its first segment"
        );
        PluginId(segment)
    }

    /// Returns whether `tool` belongs to this Plugin.
    ///
    /// A tool belongs to a Plugin when the tool id's first segment is the
    /// Plugin's id, so `web/fetch` belongs to `web`. Every tool a Plugin
    /// contributes belongs to it. The caller must check this when it
    /// assembles the run's catalog.
    #[must_use]
    pub fn contains(&self, tool: &ToolId) -> bool {
        tool.plugin() == *self
    }
}

impl std::fmt::Display for PluginId {
    /// The one-segment string form, such as `web`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for PluginId {
    /// Serializes the id as its one-segment string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for PluginId {
    /// Deserializes the id from its one-segment string and validates it
    /// the same way `PluginId::parse` does. An invalid string, or any value
    /// that is not a string, such as a map or a number, fails
    /// deserialization with a message naming the plain-name form.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PluginIdVisitor)
    }
}

/// Deserializes a [`PluginId`] from a string alone. It asks for any value,
/// not a string, because YAML would otherwise read a scalar such as `42` or
/// `true` as its text.
struct PluginIdVisitor;

impl serde::de::Visitor<'_> for PluginIdVisitor {
    type Value = PluginId;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("a Plugin's plain name, such as `web`")
    }

    fn visit_str<E: serde::de::Error>(self, text: &str) -> Result<PluginId, E> {
        PluginId::parse(text)
            .map_err(|error| E::custom(format!("invalid Plugin id `{text}`: {}", error.reason)))
    }
}

/// A stable classification of a [`PluginIdError`] that callers can match
/// on to handle each kind of failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginIdErrorKind {
    /// The id had more than one segment.
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
            GlobalNameErrorKind::Empty => (PluginIdErrorKind::Empty, "segments must not be empty"),
            GlobalNameErrorKind::Control => (
                PluginIdErrorKind::Control,
                "segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
            ),
        };
        PluginIdError { kind, reason }
    }
}

/// Lua source that a declared Plugin adds to the Lua VM of every
/// section in a run.
///
/// A prelude defines tables and functions, such as `sh.run(script)`, that
/// call the Plugin's own tools through `tools.call`. The Engine runs
/// the source knowing only the Plugin's id, which names the prelude in
/// tracebacks and error messages. The chunk receives the Plugin's local
/// name as `...`, so `local plugin = ...` lets it call
/// `tools.call(plugin .. "/ask")` whatever name the Plugin was installed
/// under.
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
