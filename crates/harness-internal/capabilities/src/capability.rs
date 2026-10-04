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

use crate::service::{HostServices, ServiceId, ServiceKey};
use crate::tool::Tool;

#[cfg(test)]
#[path = "capability-tests.rs"]
mod tests;

/// Code that runs when a run is set up and makes services available to it.
///
/// A capability is delivered in a pack and declared in a prompt's
/// frontmatter by its [`id`](Capability::id). Before a run is prepared, the
/// Harness calls [`create`](Capability::create) at most once per declared
/// capability, in declaration order. It assembles each returned
/// [`Contribution`] into the run's tool catalog and its preludes.
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

    /// Returns a one-sentence description of the capability, shown to
    /// Hosts.
    fn description(&self) -> &str;

    /// Returns the capabilities that conflict with this one when declared
    /// in the same run.
    ///
    /// For example, bashkit and a terminal each give the run its own
    /// filesystem, so a run uses at most one of the two. Activation checks
    /// every pair of declared capabilities that are present. The check is
    /// symmetric, so only one member of a pair needs to name the other. A
    /// conflicting pair fails preparation, and the failure names both
    /// members. The default returns an empty list.
    fn conflicts(&self) -> &[CapabilityId] {
        &[]
    }

    /// Returns the ids of the run services this capability needs from
    /// [`RunServices`].
    ///
    /// Activation checks each id with [`RunServices::provides`] before it
    /// calls this capability's [`create`](Capability::create). A provider
    /// registered under the id but supplied as a different type than the
    /// id names counts as missing. When a required capability needs a
    /// missing service, activation skips its `create` and refuses the run,
    /// naming the capability and the service. When the capability is
    /// optional, activation calls `create` anyway, and the capability
    /// decides how to handle the missing service. The default returns an
    /// empty list.
    fn needs(&self) -> &[ServiceId] {
        &[]
    }

    /// Activates the capability for one run.
    ///
    /// The Harness calls this at most once per run, before the run is
    /// prepared, and passes the run's services. The run receives the
    /// capability's tools and prelude only when this call succeeds.
    ///
    /// # Errors
    /// Returns a [`CapabilityError`] whose message is safe to show to a
    /// model when the capability fails to activate, for example when a
    /// backend handshake fails or the run is cancelled.
    fn create(&self, services: &RunServices) -> Result<Contribution, CapabilityError>;
}

/// The cancellation flag and services a capability receives when it
/// activates for a run.
///
/// Configuration the Host supplies for a capability arrives here, never
/// through the prompt.
#[derive(Clone)]
#[non_exhaustive]
pub struct RunServices {
    /// The run's cancellation flag. It is the same synchronous handle the
    /// Engine polls, so a capability sees a cancellation by the Host by
    /// polling it too.
    pub cancel: CancelHandle,
    /// The services the run has, read through [`get`](RunServices::get)
    /// and [`provides`](RunServices::provides). Present or absent, each
    /// stays so for the whole run.
    pub(crate) host: HostServices,
}

impl RunServices {
    /// Builds the services handed to [`Capability::create`] for one run,
    /// with an empty set of Host services.
    #[must_use]
    pub fn new(cancel: CancelHandle) -> RunServices {
        RunServices::with_host(cancel, HostServices::new())
    }

    /// Builds the services handed to [`Capability::create`] for one run,
    /// with `host` as the services the run has.
    #[must_use]
    pub fn with_host(cancel: CancelHandle, host: HostServices) -> RunServices {
        RunServices { cancel, host }
    }

    /// Returns the run's provider registered under the id of `key`, or
    /// `None` when that provider is missing or was supplied as a different
    /// type.
    #[must_use]
    pub fn get<T: ?Sized + Send + Sync + 'static>(&self, key: &ServiceKey<T>) -> Option<Arc<T>> {
        self.host.get(key)
    }

    /// Returns whether the run has a provider registered under `id` and
    /// supplied as the type that `id` names.
    #[must_use]
    pub fn provides(&self, id: &ServiceId) -> bool {
        self.host.provides(id)
    }
}

impl std::fmt::Debug for RunServices {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RunServices")
            .field("cancel", &self.cancel)
            .field("host", &self.host)
            .finish()
    }
}

/// The tools and optional Lua prelude that a capability adds to a run.
#[derive(Default)]
pub struct Contribution {
    /// The tools the capability adds to the run. Each tool's id sits under
    /// the capability's own id: `namespace/pack/name` for a
    /// `namespace/pack` capability.
    pub tools: Vec<Arc<dyn Tool>>,
    /// Lua source that every section VM of the run installs.
    /// [`Capability::create`] builds it for this run, so it can embed facts
    /// fixed at activation. A prelude defines tables and functions that
    /// call the capability's tools through `tools.call` by full id. It may
    /// call a tool or the store only after it has loaded.
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
    /// The capability failed to activate in [`Capability::create`].
    Activation,
    /// The run was cancelled before or during activation.
    Cancelled,
    /// Any other capability failure.
    Other,
}

/// An error from [`Capability::create`], with a message that is safe to
/// show to a model.
///
/// The `Display` message is written for the caller. Any underlying cause
/// stays out of the message and is available through
/// [`std::error::Error::source`]. Match on [`CapabilityError::kind`] to
/// tell failures apart. Like [`ToolError`](promptforge::tools::ToolError),
/// it pairs a stable kind for code with a message written to be read by a
/// model.
#[derive(Debug)]
#[non_exhaustive]
pub struct CapabilityError {
    kind: CapabilityErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl CapabilityError {
    /// Builds an error with the kind `Other` and the message `text`. Its
    /// `source` returns `None`. The message must be safe to show to a
    /// model.
    #[must_use]
    pub fn message(text: impl Into<String>) -> CapabilityError {
        CapabilityError {
            kind: CapabilityErrorKind::Other,
            message: text.into(),
            source: None,
        }
    }

    /// Builds an activation error with the message `text` and `src` as its
    /// underlying cause.
    ///
    /// The message must be safe to show to a model. `src` stays out of the
    /// message and is returned by `source`. The kind starts as
    /// [`CapabilityErrorKind::Activation`]. Call
    /// [`CapabilityError::with_kind`] when `src` is a different kind of
    /// failure.
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

    /// Returns this error with its kind set to `kind`.
    #[must_use]
    pub fn with_kind(mut self, kind: CapabilityErrorKind) -> CapabilityError {
        self.kind = kind;
        self
    }

    /// Returns the kind of this error.
    #[must_use]
    pub fn kind(&self) -> CapabilityErrorKind {
        self.kind
    }

    /// Returns whether the failure was a cancellation.
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
