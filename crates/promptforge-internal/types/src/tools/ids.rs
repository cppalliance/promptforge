//! Stable tool identity and its validation errors.

use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::plugins::PluginId;

/// A tool's identity: its Plugin's local name, then one or more segments the
/// Plugin chooses.
///
/// A tool id is a [`GlobalName`] of two or more segments, such as
/// `web/fetch`. The first segment is the id of the Plugin that contributed
/// the tool, and the last is the tool's own name. For example, `web/fetch`
/// comes from `web`.
///
/// The tool id stays its identity under any name a model request uses for
/// it. When a Plugin is bound to a prompt, a selected tool can be offered
/// under a prompt-local alias. Calls under that alias still reach the same
/// tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ToolId(GlobalName);

impl ToolId {
    /// Parses a tool id, which must have two or more segments
    /// (`plugin/name`).
    ///
    /// # Errors
    /// Returns [`ToolIdError`] when the id has fewer than two segments
    /// ([`ToolIdErrorKind::SegmentCount`]), is empty or has an empty segment
    /// ([`ToolIdErrorKind::Empty`]), or a segment contains a character
    /// outside the set of lowercase ASCII letters, digits, `-`, `_`, and `.`
    /// ([`ToolIdErrorKind::Control`]).
    pub fn parse(id: &str) -> Result<ToolId, ToolIdError> {
        let name =
            GlobalName::parse(id).map_err(|e| ToolIdError::from_global_name_kind(e.kind()))?;
        if name.segments().len() < 2 {
            return Err(ToolIdError {
                field: "id",
                kind: ToolIdErrorKind::SegmentCount,
                reason: "a tool id has two or more segments, its Plugin's name first, such as `web/fetch`",
            });
        }
        Ok(ToolId(name))
    }

    /// Returns the tool's name: the last segment, such as `fetch` in
    /// `web/fetch`.
    #[must_use]
    pub fn name(&self) -> &str {
        let segments = self.0.segments();
        &segments[segments.len() - 1]
    }

    /// Returns the id of the Plugin that contributed this tool, which is
    /// the tool id's first segment.
    ///
    /// That segment was validated when the tool id was parsed. This method
    /// reuses that validation and builds the [`PluginId`] directly.
    #[must_use]
    pub fn plugin(&self) -> PluginId {
        PluginId::from_segment(self.0.first())
    }
}

impl std::fmt::Display for ToolId {
    /// Formats the id in its canonical slash-separated form, such as
    /// `web/fetch`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for ToolId {
    /// Serializes the id as a single slash-separated string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ToolId {
    /// Deserializes the id from its slash-separated string.
    ///
    /// The string is validated with the same rules as `ToolId::parse`. An
    /// invalid string is a deserialization error.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        ToolId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// The stable category of a [`ToolIdError`], which callers can match on.
///
/// Get it from `ToolIdError::kind` to handle each kind of rejection
/// differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolIdErrorKind {
    /// The id had fewer than two segments.
    SegmentCount,
    /// The id was empty or had an empty segment.
    Empty,
    /// A segment contained a character outside the allowed set.
    Control,
}

/// The reason a [`ToolId`] failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tool {field}: {reason}")]
#[non_exhaustive]
pub struct ToolIdError {
    /// What was rejected: `id`, the tool id being parsed.
    field: &'static str,
    /// A stable classification of why it was rejected.
    kind: ToolIdErrorKind,
    /// A human-readable reason.
    reason: &'static str,
}

impl ToolIdError {
    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> ToolIdErrorKind {
        self.kind
    }

    /// Returns what was rejected: `id`, the tool id being parsed.
    #[must_use]
    pub fn field(&self) -> &str {
        self.field
    }

    /// Maps a global-name rejection onto the tool-id error vocabulary.
    fn from_global_name_kind(global_kind: GlobalNameErrorKind) -> ToolIdError {
        let (kind, reason) = match global_kind {
            GlobalNameErrorKind::Empty => (ToolIdErrorKind::Empty, "segments must not be empty"),
            GlobalNameErrorKind::Control => (
                ToolIdErrorKind::Control,
                "segments may contain only lowercase ASCII letters, digits, '-', '_', '.'",
            ),
        };
        ToolIdError {
            field: "id",
            kind,
            reason,
        }
    }
}
