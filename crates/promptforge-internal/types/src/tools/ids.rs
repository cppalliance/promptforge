//! Stable tool identity and its validation errors.

use crate::capabilities::CapabilityId;
use crate::names::{GlobalName, GlobalNameErrorKind};

/// The stable identity of a tool.
///
/// A tool id is a three-segment [`GlobalName`] of the form
/// `namespace/pack/name`. In global names, the segment count tells what a name
/// refers to: two segments name a capability and three name a tool. The first
/// two segments of a tool id are the id of the capability that contributed the
/// tool. So dropping the last segment of any tool id always gives that
/// capability's id. For example, `promptforge/web/fetch` comes from
/// `promptforge/web`.
///
/// A tool's wire name, the name a model request uses for it, is not its
/// identity. When a capability is bound to a prompt, a selected tool can be
/// offered under a prompt-local alias. Calls under that alias still reach the
/// same tool.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ToolId(GlobalName);

impl ToolId {
    /// Parses a tool id, which must have exactly 3 segments
    /// (`namespace/pack/name`).
    ///
    /// # Errors
    /// Returns [`ToolIdError`] when the segment count is not exactly 3
    /// ([`ToolIdErrorKind::SegmentCount`]), a segment is empty
    /// ([`ToolIdErrorKind::Empty`]), or a segment contains a character other
    /// than a lowercase ASCII letter, a digit, `-`, `_`, or `.`
    /// ([`ToolIdErrorKind::Control`]).
    pub fn parse(id: &str) -> Result<ToolId, ToolIdError> {
        let name =
            GlobalName::parse(id).map_err(|e| ToolIdError::from_global_name_kind(e.kind()))?;
        if name.segments().len() != 3 {
            return Err(ToolIdError {
                field: "id",
                kind: ToolIdErrorKind::SegmentCount,
                reason: "a tool id must have exactly 3 segments (namespace/pack/name)",
            });
        }
        Ok(ToolId(name))
    }

    /// Returns the tool's name segment (the last of the three).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.segments()[2]
    }

    /// Returns the id of the capability that contributed this tool, which is
    /// the tool id's first two segments.
    ///
    /// Every tool id has this prefix: dropping the last segment always gives
    /// the contributing capability's id. The prefix was validated when the
    /// tool id was parsed, so this method builds the [`CapabilityId`] directly
    /// without parsing it again.
    #[must_use]
    pub fn capability(&self) -> CapabilityId {
        CapabilityId::from_prefix(self.0.capability_prefix())
    }
}

impl std::fmt::Display for ToolId {
    /// Formats the id in its canonical `namespace/pack/name` string form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for ToolId {
    /// Serializes the id as a single `namespace/pack/name` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ToolId {
    /// Deserializes the id from its `namespace/pack/name` string.
    ///
    /// The string is validated with the same rules as `ToolId::parse`. An
    /// invalid string is a deserialization error and is never accepted as an
    /// id.
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
    /// The id did not have exactly 3 segments (`namespace/pack/name`).
    SegmentCount,
    /// A segment (or a wire name) was empty.
    Empty,
    /// A wire name contained the `/` namespace separator.
    Separator,
    /// A segment contained a character outside the allowed set, or a wire name
    /// contained a control character.
    Control,
}

/// The reason a [`ToolId`] (or a validated wire name) could not be built.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid tool {field}: {reason}")]
#[non_exhaustive]
pub struct ToolIdError {
    /// What was rejected (`id` for a parse failure, or `wire name`).
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

    /// Returns what was rejected (`id` for a parse failure, or `wire name`).
    #[must_use]
    pub fn field(&self) -> &str {
        self.field
    }

    /// The crate-internal human-readable reason, reused when a wire-name
    /// rejection is re-reported as a [`crate::tools::ToolCatalogError`].
    pub(super) fn reason(&self) -> &'static str {
        self.reason
    }

    /// Maps a global-name rejection onto the tool-id error vocabulary.
    fn from_global_name_kind(global_kind: GlobalNameErrorKind) -> ToolIdError {
        let (kind, reason) = match global_kind {
            GlobalNameErrorKind::SegmentCount => (
                ToolIdErrorKind::SegmentCount,
                "a tool id must have exactly 3 segments (namespace/pack/name)",
            ),
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

/// Validates one identity component (wire name).
///
/// A component must be non-empty and free of the `/` namespace separator and any
/// control character. Tool identity itself is the 3-segment global grammar
/// ([`ToolId`]); this rule set remains for tool wire names, which are
/// single-segment transport tokens.
pub(super) fn validate_identifier(field: &'static str, value: &str) -> Result<(), ToolIdError> {
    if value.is_empty() {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Empty,
            reason: "must not be empty",
        });
    }
    if value.contains('/') {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Separator,
            reason: "must not contain the '/' separator",
        });
    }
    if value.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return Err(ToolIdError {
            field,
            kind: ToolIdErrorKind::Control,
            reason: "must not contain a control character",
        });
    }
    Ok(())
}
