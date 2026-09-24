//! Stable tool identity and its validation errors.

use crate::capabilities::CapabilityId;
use crate::names::{GlobalName, GlobalNameErrorKind};

/// The stable identity of a live tool.
///
/// Identity is a 3-segment [`GlobalName`] (`namespace/pack/name`): the global
/// naming grammar encodes kind by arity, and a tool's first two segments name
/// the capability that contributed it, so dropping the last segment of any
/// tool id always yields the contributing capability's id
/// (`promptforge/web/fetch` comes from `promptforge/web`, no exceptions). The
/// wire name used in a model request is deliberately not identity: capability
/// binding can advertise a selected tool under a prompt-local alias without
/// changing the live tool it dispatches.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct ToolId(GlobalName);

impl ToolId {
    /// Parses a tool identity, requiring exactly 3 segments
    /// (`namespace/pack/name`).
    ///
    /// # Errors
    /// Returns [`ToolIdError`] when the segment count is not exactly 3
    /// ([`ToolIdErrorKind::SegmentCount`]), a segment is empty
    /// ([`ToolIdErrorKind::Empty`]), or a segment contains a character outside
    /// the global-name charset ([`ToolIdErrorKind::Control`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    /// use promptforge_types::tools::ToolId;
    ///
    /// let id = ToolId::parse("promptforge/web/fetch")?;
    /// assert_eq!(id.name(), "fetch");
    /// assert_eq!(id.capability(), CapabilityId::parse("promptforge/web")?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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

    /// Builds an identity from a string already known to be valid.
    ///
    /// Crate-internal: backs [`crate::detail::tool_id_from_validated`].
    pub(crate) fn from_validated(id: &str) -> ToolId {
        let name = GlobalName::from_validated(id);
        debug_assert!(
            name.segments().len() == 3,
            "a static tool id must have exactly 3 segments (namespace/pack/name): {id}"
        );
        ToolId(name)
    }

    /// Returns the tool's name segment (the last of the three).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::tools::ToolId;
    ///
    /// let id = ToolId::parse("promptforge/web/fetch")?;
    /// assert_eq!(id.name(), "fetch");
    /// # Ok::<(), promptforge_types::tools::ToolIdError>(())
    /// ```
    #[must_use]
    pub fn name(&self) -> &str {
        &self.0.segments()[2]
    }

    /// Returns the contributing capability's id: the first two segments.
    ///
    /// Containment is total - dropping the last segment of any tool id always
    /// yields the id of the capability that contributed it. The prefix was
    /// validated when the tool id was parsed, so it builds the
    /// [`CapabilityId`] directly, with no re-parse.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    /// use promptforge_types::tools::ToolId;
    ///
    /// let id = ToolId::parse("promptforge/web/fetch")?;
    /// assert_eq!(id.capability(), CapabilityId::parse("promptforge/web")?);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn capability(&self) -> CapabilityId {
        CapabilityId::from_prefix(self.0.capability_prefix())
    }
}

impl std::fmt::Display for ToolId {
    /// The canonical `namespace/pack/name` string form.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

impl serde::Serialize for ToolId {
    /// Serializes the identity as its one `namespace/pack/name` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for ToolId {
    /// Deserializes the identity from its string form, validating it as a
    /// 3-segment global name: an invalid string is a data error, never a
    /// silently accepted identity.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        ToolId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A stable, matchable classification of a [`ToolIdError`].
///
/// Every public error exposes a `kind()` classifier so callers can branch on the
/// failure without matching a private representation (DESIGN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolIdErrorKind {
    /// The id did not have exactly 3 segments (`namespace/pack/name`).
    SegmentCount,
    /// A segment (or a wire name) was empty.
    Empty,
    /// A wire name contained the `/` namespace separator.
    Separator,
    /// A segment (or a wire name) contained a character outside the allowed
    /// set.
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
    /// Returns the stable classification of this error (DESIGN-5).
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
    pub(crate) fn reason(&self) -> &'static str {
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
/// single-segment transport tokens (tools.rs F4).
pub(crate) fn validate_identifier(field: &'static str, value: &str) -> Result<(), ToolIdError> {
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
