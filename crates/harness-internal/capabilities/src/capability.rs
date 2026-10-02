//! The capability activation contract.
//!
//! A capability is the activation unit: code that runs at run setup and
//! makes services available to the run. Capabilities are delivered in packs
//! (crates now, DLLs via adapters later) and identified by a 2-segment
//! [`CapabilityId`] - kind is encoded by arity, so a capability id is
//! `namespace/pack` and every tool it contributes sits under
//! `namespace/pack/name`. Before a run is prepared, the Harness activates
//! each declared capability by calling [`Capability::create`] with the
//! run's [`RunServices`]; the returned [`Contribution`] holds tools and an
//! optional Lua prelude, and grows without redesign. An activation failure
//! is a [`CapabilityError`]: a stable kind for code plus a message written
//! to be read by a model, mirroring
//! [`ToolError`](promptforge::tools::ToolError).

use std::sync::Arc;

use promptforge::cancel::CancelHandle;
use promptforge::capabilities::CapabilityId;
use promptforge::vfs::VfsRef;

use crate::input::InputBroker;
use crate::service::{HostServices, ServiceId, ServiceKey};
use crate::tool::Tool;
use crate::user_input::INPUT_BROKER;

#[cfg(test)]
#[path = "capability-tests.rs"]
mod tests;

/// The activation unit: code that runs at run setup and makes services
/// available to the run.
///
/// A capability is delivered in a pack (a crate now, a DLL via an adapter
/// later) and declared in a prompt's frontmatter by its
/// [`id`](Capability::id). Before a run is prepared, the Harness calls
/// [`create`](Capability::create) once per declared capability, in
/// declaration order, and assembles the returned [`Contribution`] into the
/// run's tool catalog and its preludes.
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

    /// A one-sentence description, surfaced to Hosts.
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

    /// Returns the ids of the run services this capability needs from
    /// [`RunServices`].
    ///
    /// Activation checks these against [`RunServices::provides`] before
    /// any capability code runs, so a provider of another type than the
    /// id names counts as missing. When a required capability needs a
    /// service the Host does not provide, activation does not call
    /// [`create`](Capability::create) and refuses the run naming both.
    /// When the capability is optional, activation calls `create` anyway
    /// and the capability decides how to work without the service. The
    /// default is no needs.
    fn needs(&self) -> &[ServiceId] {
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

/// What a capability is given at activation.
///
/// Non-exhaustive so new fields (the model client) can be added when a
/// bridge capability needs them without breaking existing capability
/// implementations. Host-supplied per-capability config arrives here,
/// never via the prompt.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunServices {
    /// The run's whole filesystem: the real directories and the declared
    /// store, handed over by the Harness before activation.
    pub vfs: VfsRef,
    /// The run's cancellation flag: the same synchronous handle the Engine
    /// polls, so a capability observes the Host's cancel by polling too.
    pub cancel: CancelHandle,
    /// The services the run has, read through [`get`](RunServices::get)
    /// and [`provides`](RunServices::provides). Present or absent, each
    /// stays so for the whole run.
    pub(crate) host: HostServices,
}

impl RunServices {
    /// Builds the services handed to [`Capability::create`] for one run,
    /// with no Host services.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::{INPUT_BROKER, RunServices};
    /// use promptforge::cancel::CancelHandle;
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    /// assert!(!services.cancel.is_cancelled());
    /// assert!(services.get(&INPUT_BROKER).is_none());
    /// ```
    #[must_use]
    pub fn new(vfs: VfsRef, cancel: CancelHandle) -> RunServices {
        RunServices::with_host(vfs, cancel, HostServices::new())
    }

    /// Builds the services handed to [`Capability::create`] for one run,
    /// with `host` as the services the run has.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use harness_capabilities::{HostServices, RunServices, ServiceKey};
    /// use promptforge::cancel::CancelHandle;
    /// use promptforge::vfs::VfsRef;
    ///
    /// const GREETING: ServiceKey<str> = ServiceKey::new("acme/greeting");
    ///
    /// let mut host = HostServices::new();
    /// host.provide(&GREETING, Arc::from("hello"))?;
    /// let services = RunServices::with_host(VfsRef::default(), CancelHandle::new(), host);
    /// assert_eq!(services.get(&GREETING).as_deref(), Some("hello"));
    /// # Ok::<(), harness_capabilities::ServiceError>(())
    /// ```
    #[must_use]
    pub fn with_host(vfs: VfsRef, cancel: CancelHandle, host: HostServices) -> RunServices {
        RunServices { vfs, cancel, host }
    }

    /// Supplies the run's input broker under [`INPUT_BROKER`], replacing
    /// any provider already under `promptforge/input-broker`: the
    /// session's broker is the run's only input broker.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    ///
    /// use harness_capabilities::{INPUT_BROKER, InputBroker, InputError, RunServices};
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
    /// let mut services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    /// services.insert_input_broker(Arc::new(Scripted));
    /// assert!(services.get(&INPUT_BROKER).is_some());
    /// ```
    pub fn insert_input_broker(&mut self, broker: Arc<dyn InputBroker>) {
        self.host.insert(&INPUT_BROKER, broker);
    }

    /// Returns the run's provider under `key`'s id, or `None` when it has
    /// none or has one of another type.
    #[must_use]
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self, key: &ServiceKey<T>) -> Option<Arc<T>> {
        self.host.get(key)
    }

    /// Returns whether the run has a provider under `id` of the type `id`
    /// names.
    ///
    /// # Examples
    ///
    /// ```
    /// use harness_capabilities::{INPUT_BROKER, RunServices};
    /// use promptforge::cancel::CancelHandle;
    ///
    /// let services = RunServices::new(promptforge::vfs::VfsRef::default(), CancelHandle::new());
    /// assert!(!services.provides(&INPUT_BROKER.id()));
    /// ```
    #[must_use]
    pub fn provides(&self, id: &ServiceId) -> bool {
        self.host.provides(id)
    }
}

impl std::fmt::Debug for RunServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunServices")
            .field("vfs", &self.vfs)
            .field("cancel", &self.cancel)
            .field("host", &self.host)
            .finish()
    }
}

/// What a capability contributes to a run: its tools and, optionally, a
/// prelude of Lua source.
///
/// Mounts and prompt fragments are deferred until the capabilities that
/// need them land. The struct is [`Default`] and grows without redesign.
///
/// # Examples
///
/// ```
/// use harness_capabilities::Contribution;
///
/// let contribution = Contribution::default();
/// assert!(contribution.tools.is_empty());
/// assert!(contribution.prelude.is_none());
/// ```
#[derive(Default)]
pub struct Contribution {
    /// The contributed tools, each identified under the capability's own
    /// full id (`namespace/pack/name` for a `namespace/pack` capability).
    pub tools: Vec<Arc<dyn Tool>>,
    /// Lua source that every section VM of the run installs, built by
    /// [`Capability::create`] for this run so it can embed facts fixed at
    /// activation. A prelude defines tables and functions that reach the
    /// capability's tools through `tools.call` by full id; it must not
    /// call a tool or the store while it loads.
    pub prelude: Option<String>,
}

impl std::fmt::Debug for Contribution {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Contribution")
            .field(
                "tools",
                &self.tools.iter().map(|tool| tool.id()).collect::<Vec<_>>(),
            )
            .field("prelude", &self.prelude.is_some())
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
