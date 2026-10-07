//! [`ToolContext`]: what one tool call receives.

use std::sync::Arc;

use promptforge_types::tools::{ToolCallOrigin, ToolId};
use promptforge_vfs::Access;

use crate::{HostServices, ServiceKey};

#[cfg(test)]
#[path = "context-tests.rs"]
mod tests;

/// What one tool call lends its Plugin: the called tool's id, the call's
/// filesystem access, who made the call, and the run's own services.
///
/// The access is the call's own identity, forked from the calling chain's,
/// so the chain's earlier work happens before the call's operations and
/// the call's operations before the chain's next step. It is rooted at
/// `/`. The context borrows everything for the call alone, so a Plugin
/// cannot store the access, move it into a spawned task or thread, or keep
/// it past the call.
///
/// The services are the run's, such as its input broker. The Host-wide
/// services never reach a call; a Plugin keeps any it needs from its
/// [`Package::construct`](crate::Package::construct).
#[derive(Debug)]
pub struct ToolContext<'a> {
    tool: &'a ToolId,
    access: &'a Access,
    origin: &'a ToolCallOrigin,
    services: &'a HostServices,
}

impl<'a> ToolContext<'a> {
    /// A context lending `tool`, `access`, `origin`, and the run's
    /// `services` to one call. The Harness builds one per call.
    #[must_use]
    pub fn new(
        tool: &'a ToolId,
        access: &'a Access,
        origin: &'a ToolCallOrigin,
        services: &'a HostServices,
    ) -> Self {
        Self {
            tool,
            access,
            origin,
            services,
        }
    }

    /// The tool that was called. A Plugin with several tools matches on
    /// its [`name`](ToolId::name) to pick one.
    #[must_use]
    pub fn tool(&self) -> &'a ToolId {
        self.tool
    }

    /// The call's filesystem access, rooted at `/`.
    #[must_use]
    pub fn access(&self) -> &'a Access {
        self.access
    }

    /// Who made the call and where.
    #[must_use]
    pub fn origin(&self) -> &'a ToolCallOrigin {
        self.origin
    }

    /// Returns the run's service under `key`.
    ///
    /// Returns `None` when the run has no provider under the key's id, or
    /// when the provider was supplied as a type other than `T`.
    #[must_use]
    pub fn service<T: ?Sized + Send + Sync + 'static>(
        &self,
        key: &ServiceKey<T>,
    ) -> Option<Arc<T>> {
        self.services.get(key)
    }
}
