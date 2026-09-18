//! The capability activation contract.
//!
//! A capability is the activation unit: code that runs at run setup and
//! makes services available to the run. Capabilities are delivered in packs
//! (crates now, DLLs via adapters later) and identified by a 2-segment
//! [`GlobalName`] - kind is encoded by arity, so a
//! capability id is `namespace/pack` and every tool it contributes lives
//! under `namespace/pack/name`. At prepare time the executor activates each
//! declared capability by calling [`Capability::create`] with the run's
//! [`RunServices`]; the returned [`Contribution`] is v1 tools-only and grows
//! without redesign. An activation failure is a [`CapabilityError`]: a
//! stable kind for code plus a message written to be read by a model,
//! mirroring [`ToolError`](crate::tools::ToolError).

use std::sync::Arc;

use shared_vfs::VfsRef;

use crate::cancel::CancelHandle;
use crate::names::{GlobalName, GlobalNameErrorKind};
use crate::tools::{Tool, ToolId};

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
    /// use promptforge_api_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("promptforge/web")?;
    /// assert_eq!(id.namespace(), "promptforge");
    /// assert_eq!(id.pack(), "web");
    /// # Ok::<(), promptforge_api_types::capabilities::CapabilityIdError>(())
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
    /// For internal callers whose inputs are static capability ids, so the
    /// validation in [`CapabilityId::parse`] is redundant. Hidden from the
    /// public API: downstream callers use [`CapabilityId::parse`].
    #[doc(hidden)]
    #[must_use]
    pub fn from_validated(id: &str) -> CapabilityId {
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
    /// are already a valid capability id and need no re-parse.
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
    /// use promptforge_api_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("org.rustalliance/core")?;
    /// assert_eq!(id.namespace(), "org.rustalliance");
    /// # Ok::<(), promptforge_api_types::capabilities::CapabilityIdError>(())
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
    /// use promptforge_api_types::capabilities::CapabilityId;
    ///
    /// let id = CapabilityId::parse("promptforge/web")?;
    /// assert_eq!(id.pack(), "web");
    /// # Ok::<(), promptforge_api_types::capabilities::CapabilityIdError>(())
    /// ```
    #[must_use]
    pub fn pack(&self) -> &str {
        self.0.pack()
    }

    /// Returns whether `tool` lives under this capability's id.
    ///
    /// Containment is total: a contributed tool's id is always its
    /// contributing capability's id plus one name segment
    /// (`namespace/pack/name` for a `namespace/pack` capability), so
    /// dropping the tool's last segment must yield exactly this id.
    /// Prepare enforces containment when the run's catalog is assembled.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::capabilities::CapabilityId;
    /// use promptforge_api_types::tools::ToolId;
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

/// The activation unit: code that runs at run setup and makes services
/// available to the run.
///
/// A capability is delivered in a pack (a crate now, a DLL via an adapter
/// later) and declared in a prompt's frontmatter by its
/// [`id`](Capability::id). At prepare time the executor calls
/// [`create`](Capability::create) once per declared capability, in
/// declaration order, and assembles the returned [`Contribution`] into the
/// run's tool catalog.
///
/// # Implementing
///
/// ```
/// use promptforge_api_types::capabilities::{
///     Capability, CapabilityError, CapabilityId, Contribution, RunServices,
/// };
///
/// struct Web {
///     id: CapabilityId,
/// }
///
/// impl Capability for Web {
///     fn id(&self) -> &CapabilityId {
///         &self.id
///     }
///     fn description(&self) -> &str {
///         "Web fetch and search tools."
///     }
///     fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError> {
///         let _ = services;
///         Ok(Contribution::default())
///     }
/// }
///
/// let web = Web {
///     id: CapabilityId::parse("promptforge/web")?,
/// };
/// assert_eq!(web.id().pack(), "web");
/// # Ok::<(), promptforge_api_types::capabilities::CapabilityIdError>(())
/// ```
///
/// # Invariants
///
/// - [`id`](Capability::id) returns the same value on every call; it is the
///   registry key and must be unique within a registry.
/// - Every contributed tool's id lives under the capability's own id:
///   `namespace/pack/name` for a `namespace/pack` capability. Containment is
///   total and is checked when the run's catalog is assembled.
/// - [`create`](Capability::create) must not panic and should return
///   promptly when the run is cancelled.
pub trait Capability: Send + Sync {
    /// Returns the capability's stable identity (`namespace/pack`).
    fn id(&self) -> &CapabilityId;

    /// A one-sentence description, surfaced to hosts.
    fn description(&self) -> &str;

    /// Returns the capabilities this one cannot be activated with in one
    /// run.
    ///
    /// Co-activation rules attach at the capability level: bashkit and a
    /// terminal are two filesystem realities, and a context gets one or
    /// the other, never both. The default is no conflicts. Prepare checks
    /// the declared present capabilities pairwise - the check is
    /// symmetric, so only one member of a pair needs to name the other -
    /// and fails preparation naming both members of a conflicting pair.
    fn conflicts(&self) -> &[CapabilityId] {
        &[]
    }

    /// Activates the capability for one run.
    ///
    /// Called once per run at prepare time with the run's services. A
    /// failure returns a narrow, model-safe [`CapabilityError`] and the
    /// capability contributes nothing to the run.
    ///
    /// # Errors
    /// Returns a [`CapabilityError`] if the capability cannot activate (a
    /// missing host service, a failed backend handshake, cancellation).
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError>;
}

/// What a capability is given at activation.
///
/// Non-exhaustive so new fields (the input broker, the observer, the model
/// client) can be added when a bridge capability needs them without
/// breaking existing capability implementations. Host-supplied
/// per-capability config arrives here, never via the prompt.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RunServices {
    /// The run's filesystem.
    pub vfs: VfsRef,
    /// The run's cancellation handle.
    pub cancel: CancelHandle,
}

impl RunServices {
    /// Builds the services handed to [`Capability::create`] for one run.
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge_api_types::cancel::CancelHandle;
    /// use promptforge_api_types::capabilities::RunServices;
    ///
    /// let services = RunServices::new(shared_vfs::VfsRef::builder().build(), CancelHandle::new());
    /// assert!(!services.cancel.is_cancelled());
    /// ```
    #[must_use]
    pub fn new(vfs: VfsRef, cancel: CancelHandle) -> RunServices {
        RunServices { vfs, cancel }
    }
}

/// What a capability contributes to a run.
///
/// v1 is tools-only: mounts, prompt fragments, and Lua surface are deferred
/// until the capabilities that need them land. The struct is
/// [`Default`] and grows without redesign.
///
/// # Examples
///
/// ```
/// use promptforge_api_types::capabilities::Contribution;
///
/// let contribution = Contribution::default();
/// assert!(contribution.tools.is_empty());
/// ```
#[derive(Default)]
pub struct Contribution {
    /// The contributed tools, each identified under the capability's own
    /// full id (`namespace/pack/name` for a `namespace/pack` capability).
    pub tools: Vec<Arc<dyn Tool>>,
}

impl std::fmt::Debug for Contribution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Contribution")
            .field(
                "tools",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .finish()
    }
}

/// A stable, matchable classification of a [`CapabilityError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum CapabilityErrorKind {
    /// The capability's activation ([`Capability::create`]) failed.
    Activation,
    /// The run was cancelled before or during activation.
    Cancelled,
    /// Any other capability failure.
    Other,
}

/// A narrow, model-safe error from [`Capability::create`].
///
/// The `Display` message is caller-facing and safe to hand to a model; any
/// underlying cause is hidden behind [`std::error::Error::source`]. Match on
/// [`CapabilityError::kind`] rather than a private representation. This
/// mirrors [`ToolError`](crate::tools::ToolError): a stable kind for code, a
/// message written to be read by a model.
#[derive(Debug)]
#[non_exhaustive]
pub struct CapabilityError {
    kind: CapabilityErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CapabilityError {
    /// Builds a model-safe error carrying only a message (kind `Other`).
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::capabilities::{CapabilityError, CapabilityErrorKind};
    ///
    /// let err = CapabilityError::message("the fs capability needs a writable store");
    /// assert_eq!(err.kind(), CapabilityErrorKind::Other);
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> CapabilityError {
        CapabilityError {
            kind: CapabilityErrorKind::Other,
            message: text.into(),
            source: None,
        }
    }

    /// Builds a model-safe activation error with `src` as a hidden
    /// `#[source]`.
    ///
    /// The initial kind is [`CapabilityErrorKind::Activation`]; use
    /// [`CapabilityError::with_kind`] when the source represents another
    /// class.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::capabilities::{CapabilityError, CapabilityErrorKind};
    ///
    /// let io = std::io::Error::other("boom");
    /// let err = CapabilityError::with_source("activation failed", io);
    /// assert_eq!(err.kind(), CapabilityErrorKind::Activation);
    /// assert!(std::error::Error::source(&err).is_some());
    /// ```
    #[must_use]
    pub fn with_source(
        text: impl Into<String>,
        src: impl std::error::Error + Send + Sync + 'static,
    ) -> CapabilityError {
        CapabilityError {
            kind: CapabilityErrorKind::Activation,
            message: text.into(),
            source: Some(Box::new(src)),
        }
    }

    /// Sets the classification, returning the updated error.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::capabilities::{CapabilityError, CapabilityErrorKind};
    ///
    /// let err = CapabilityError::message("stopped").with_kind(CapabilityErrorKind::Cancelled);
    /// assert!(err.is_cancelled());
    /// ```
    #[must_use]
    pub fn with_kind(mut self, kind: CapabilityErrorKind) -> CapabilityError {
        self.kind = kind;
        self
    }

    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> CapabilityErrorKind {
        self.kind
    }

    /// Returns whether the failure was a cancellation.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_types::capabilities::{CapabilityError, CapabilityErrorKind};
    ///
    /// let err = CapabilityError::message("stopped").with_kind(CapabilityErrorKind::Cancelled);
    /// assert!(err.is_cancelled());
    /// ```
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, CapabilityErrorKind::Cancelled)
    }
}

impl std::fmt::Display for CapabilityError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for CapabilityError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}
