//! The capability identity vocabulary: [`CapabilityId`] and its parse
//! error.
//!
//! A capability is the activation unit: code that runs at run setup and
//! makes services available to the run. Capabilities are delivered in packs
//! (crates now, DLLs via adapters later) and identified by a 2-segment
//! [`GlobalName`] - kind is encoded by arity, so a capability id is
//! `namespace/pack` and every tool it contributes sits under
//! `namespace/pack/name`. The engine knows capabilities by identity alone:
//! a prompt declares them, an exact tool slot names one through its
//! [`ToolId`] prefix, and a [`ToolDescriptor`](crate::tools::ToolDescriptor)
//! records the conflicts of the capability that contributed it. The
//! activation contract - the `Capability` trait, the services it is handed,
//! and the contribution it returns - is the harness's, in
//! `harness-capabilities`; the engine never activates anything.

use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::tools::ToolId;

#[cfg(test)]
#[path = "capabilities-tests.rs"]
mod tests;

/// The stable identity of an installed capability.
///
/// Identity is a 2-segment [`GlobalName`] (`namespace/pack`): the global
/// naming grammar encodes kind by arity, and a capability's id is the
/// prefix of every tool id it contributes (`promptforge/web` contributes
/// `promptforge/web/fetch`, no exceptions). v1 is unversioned: a name
/// resolves to the only installed capability and a `@` is a parse error.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub struct CapabilityId(GlobalName);

impl CapabilityId {
    /// Parses a capability identity, requiring exactly 2 segments
    /// (`namespace/pack`).
    ///
    /// # Errors
    /// Returns [`CapabilityIdError`] when the segment count is not exactly 2
    /// ([`CapabilityIdErrorKind::SegmentCount`]), a segment is empty
    /// ([`CapabilityIdErrorKind::Empty`]), or a segment contains a character
    /// outside the global-name charset ([`CapabilityIdErrorKind::Control`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("promptforge/web")?;
    /// assert_eq!(id.namespace(), "promptforge");
    /// assert_eq!(id.pack(), "web");
    /// # Ok::<(), promptforge_types::capabilities::CapabilityIdError>(())
    /// ```
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

    /// Builds an identity from a string already known to be valid.
    ///
    /// Crate-internal: backs [`crate::detail::capability_id_from_validated`].
    pub(crate) fn from_validated(id: &str) -> CapabilityId {
        let name = GlobalName::from_validated(id);
        debug_assert!(
            name.segments().len() == 2,
            "a static capability id must have exactly 2 segments (namespace/pack): {id}"
        );
        CapabilityId(name)
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

    /// Returns the namespace segment (reverse-DNS or `promptforge`).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("org.rustalliance/core")?;
    /// assert_eq!(id.namespace(), "org.rustalliance");
    /// # Ok::<(), promptforge_types::capabilities::CapabilityIdError>(())
    /// ```
    #[must_use]
    pub fn namespace(&self) -> &str {
        self.0.namespace()
    }

    /// Returns the pack segment.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("promptforge/web")?;
    /// assert_eq!(id.pack(), "web");
    /// # Ok::<(), promptforge_types::capabilities::CapabilityIdError>(())
    /// ```
    #[must_use]
    pub fn pack(&self) -> &str {
        self.0.pack()
    }

    /// Returns whether `tool` sits under this capability's id.
    ///
    /// Containment is total: a contributed tool's id is always its
    /// contributing capability's id plus one name segment
    /// (`namespace/pack/name` for a `namespace/pack` capability), so
    /// dropping the tool's last segment must yield exactly this id.
    /// The host enforces containment when the run's catalog is assembled.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_types::capabilities::CapabilityId;
    /// use promptforge_types::tools::ToolId;
    ///
    /// let web = CapabilityId::parse("promptforge/web")?;
    /// let fetch = ToolId::parse("promptforge/web/fetch")?;
    /// let stray = ToolId::parse("promptforge/other/fetch")?;
    /// assert!(web.contains(&fetch));
    /// assert!(!web.contains(&stray));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
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
    /// Serializes the identity as its one `namespace/pack` string.
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.0.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for CapabilityId {
    /// Deserializes the identity from its string form, validating it as a
    /// 2-segment global name: an invalid string is a data error, never a
    /// silently accepted identity.
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = <String as serde::Deserialize>::deserialize(deserializer)?;
        CapabilityId::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// A stable, matchable classification of a [`CapabilityIdError`].
///
/// Every public error exposes a `kind()` classifier so callers can branch on
/// the failure without matching a private representation (DESIGN-5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityIdErrorKind {
    /// The id did not have exactly 2 segments (`namespace/pack`).
    SegmentCount,
    /// A segment was empty.
    Empty,
    /// A segment contained a character outside the allowed set.
    Control,
}

/// The reason a [`CapabilityId`] could not be parsed.
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
    /// Returns the stable classification of this error (DESIGN-5).
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
