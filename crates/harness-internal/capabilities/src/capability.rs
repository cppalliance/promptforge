//! The capability activation contract.
//!
//! A capability is the activation unit: code that runs at run setup and
//! makes services available to the run. Capabilities are delivered in packs
//! (crates now, DLLs via adapters later) and identified by a 2-segment
//! [`CapabilityId`] - kind is encoded by arity, so a capability id is
//! `namespace/pack` and every tool it contributes sits under
//! `namespace/pack/name`. Before a run is prepared, the harness activates
//! each declared capability by calling [`Capability::create`] with the
//! run's [`RunServices`]; the returned [`Contribution`] is v1 tools-only
//! and grows without redesign. An activation failure is a
//! [`CapabilityError`]: a stable kind for code plus a message written to be
//! read by a model, mirroring [`ToolError`](promptforge::tools::ToolError).

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;
use promptforge::vfs::VfsRef;

use crate::tool::Tool;

#[cfg(test)]
#[path = "capability-tests.rs"]
mod tests;

/// The activation unit: code that runs at run setup and makes services
/// available to the run.
///
/// A capability is delivered in a pack (a crate now, a DLL via an adapter
/// later) and declared in a prompt's frontmatter by its
/// [`id`](Capability::id). Before a run is prepared, the harness calls
/// [`create`](Capability::create) once per declared capability, in
/// declaration order, and assembles the returned [`Contribution`] into the
/// run's tool catalog.
///
/// # Implementing
///
/// ```
/// use harness_capabilities::{
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
/// # Ok::<(), promptforge::capabilities::CapabilityIdError>(())
/// ```
///
/// # Invariants
///
/// - [`id`](Capability::id) returns the same value on every call; it is the
///   registry key and must be unique within a registry.
/// - Every contributed tool's id sits under the capability's own id:
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
    /// the other, never both. The default is no conflicts. Activation
    /// checks the declared present capabilities pairwise - the check is
    /// symmetric, so only one member of a pair needs to name the other -
    /// and fails preparation naming both members of a conflicting pair.
    fn conflicts(&self) -> &[CapabilityId] {
        &[]
    }

    /// Activates the capability for one run.
    ///
    /// Called once per run before prepare with the run's services. A
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
/// Non-exhaustive so new fields (the input broker, the model client) can
/// be added when a bridge capability needs them without breaking existing
/// capability implementations. Host-supplied per-capability config arrives
/// here, never via the prompt.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct RunServices {
    /// The run's filesystem.
    pub vfs: VfsRef,
    /// The run's cancellation flag: the same synchronous handle the engine
    /// polls, so a capability observes the host's cancel by polling too.
    pub cancel: CancelHandle,
}

impl RunServices {
    /// Builds the services handed to [`Capability::create`] for one run.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::RunServices;
    /// use promptforge::cancel::CancelHandle;
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::builder().build(), CancelHandle::new());
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
/// use harness_capabilities::Contribution;
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
/// mirrors [`ToolError`](promptforge::tools::ToolError): a stable
/// kind for code, a message written to be read by a model.
#[derive(Debug)]
#[non_exhaustive]
pub struct CapabilityError {
    kind: CapabilityErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CapabilityError {
    /// Builds a model-safe error with only a message (kind `Other`).
    ///
    /// # Examples
    /// ```
    /// use harness_capabilities::{CapabilityError, CapabilityErrorKind};
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
    /// use harness_capabilities::{CapabilityError, CapabilityErrorKind};
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
    /// use harness_capabilities::{CapabilityError, CapabilityErrorKind};
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
    /// use harness_capabilities::{CapabilityError, CapabilityErrorKind};
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
