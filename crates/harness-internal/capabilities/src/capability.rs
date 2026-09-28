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

use crate::input::InputBroker;
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

    /// Returns the host services this capability needs from
    /// [`RunServices`].
    ///
    /// Activation checks these against [`RunServices::provides`] before
    /// any capability code runs. When a required capability needs a
    /// service the host does not provide, activation does not call
    /// [`create`](Capability::create) and refuses the run naming both.
    /// When the capability is optional, activation calls `create` anyway
    /// and the capability decides how to work without the service. The
    /// default is no needs.
    fn needs(&self) -> &[Service] {
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
    /// failed backend handshake, cancellation).
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError>;
}

/// A host service a capability can need: the closed set of optional
/// services the harness supplies through [`RunServices`].
///
/// A capability names what it needs through [`Capability::needs`], and
/// activation checks each one with [`RunServices::provides`]. The run's
/// filesystem and cancel signal are always present, so they are not
/// listed here. Adding a service is a harness change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Service {
    /// The operator's input broker, [`RunServices::input`].
    Input,
}

impl Service {
    /// Returns the service's name as a model reads it in a refusal, such
    /// as "an input broker".
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::Service;
    ///
    /// assert_eq!(Service::Input.description(), "an input broker");
    /// ```
    #[must_use]
    pub fn description(self) -> &'static str {
        match self {
            Service::Input => "an input broker",
        }
    }
}

/// What a capability is given at activation.
///
/// Non-exhaustive so new fields (the model client) can be added when a
/// bridge capability needs them without breaking existing capability
/// implementations. Host-supplied per-capability config arrives here,
/// never via the prompt.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunServices {
    /// The run's whole filesystem: the host roots and the declared store,
    /// built by the host before activation.
    pub vfs: VfsRef,
    /// The run's cancellation flag: the same synchronous handle the engine
    /// polls, so a capability observes the host's cancel by polling too.
    pub cancel: CancelHandle,
    /// The operator's input broker, when the host has someone to ask.
    /// `None` is a legitimate answer: a batch or eval host has nobody at
    /// the other end. Present or absent, it stays so for the whole run.
    pub input: Option<Arc<dyn InputBroker>>,
}

impl RunServices {
    /// Builds the services handed to [`Capability::create`] for one run,
    /// with no input broker.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::RunServices;
    /// use promptforge::cancel::CancelHandle;
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    /// assert!(!services.cancel.is_cancelled());
    /// assert!(services.input.is_none());
    /// ```
    #[must_use]
    pub fn new(vfs: VfsRef, cancel: CancelHandle) -> RunServices {
        RunServices {
            vfs,
            cancel,
            input: None,
        }
    }

    /// Supplies the run's input broker, returning the updated services.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use harness_capabilities::{InputBroker, InputError, RunServices};
    /// use promptforge::cancel::CancelHandle;
    ///
    /// struct Scripted;
    ///
    /// #[async_trait::async_trait]
    /// impl InputBroker for Scripted {
    ///     async fn wait(&self) -> Result<String, InputError> {
    ///         Ok("hello".to_owned())
    ///     }
    /// }
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new())
    ///     .with_input(Arc::new(Scripted));
    /// assert!(services.input.is_some());
    /// ```
    #[must_use]
    pub fn with_input(mut self, broker: Arc<dyn InputBroker>) -> RunServices {
        self.input = Some(broker);
        self
    }

    /// Returns whether this run has `service`: for [`Service::Input`],
    /// whether the host supplied an input broker.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::{RunServices, Service};
    /// use promptforge::cancel::CancelHandle;
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    /// assert!(!services.provides(Service::Input));
    /// ```
    #[must_use]
    pub fn provides(&self, service: Service) -> bool {
        match service {
            Service::Input => self.input.is_some(),
        }
    }
}

impl std::fmt::Debug for RunServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunServices")
            .field("vfs", &self.vfs)
            .field("cancel", &self.cancel)
            .field("input", &self.input.is_some())
            .finish()
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
