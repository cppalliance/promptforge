//! [`TestCall`]: lends a [`ToolContext`] to a Plugin crate's own tests.

use promptforge_types::tools::{ToolCallOrigin, ToolCaller, ToolId};
use promptforge_vfs::{Access, Origin, VfsRef};

use crate::{HostServices, ToolContext};

/// One tool call a test makes directly, without a Harness.
///
/// It owns everything a [`ToolContext`] borrows: the called tool's id, an
/// access over a fresh in-memory filesystem, an origin naming the script
/// of the section `Test`, and the run's services, which are empty unless
/// [`with_services`](TestCall::with_services) supplies them.
#[derive(Debug)]
pub struct TestCall {
    tool: ToolId,
    access: Access,
    origin: ToolCallOrigin,
    services: HostServices,
}

impl TestCall {
    /// A call of `tool` from the script of the section `Test`, over a
    /// fresh in-memory filesystem, with no services.
    ///
    /// # Panics
    /// Panics when the fresh in-memory filesystem refuses the access,
    /// which its backend never does.
    #[must_use]
    #[expect(
        clippy::expect_used,
        reason = "a test fixture; the in-memory backend accepts every acquire"
    )]
    pub fn new(tool: ToolId) -> TestCall {
        let access = VfsRef::default()
            .acquire(Origin::new("plugin test"))
            .expect("a fresh memory filesystem acquires");
        let origin = ToolCallOrigin {
            execution: "plugin-test".to_owned(),
            section: "Test".to_owned(),
            caller: ToolCaller::Script,
        };
        TestCall {
            tool,
            access,
            origin,
            services: HostServices::new(),
        }
    }

    /// Returns the call with `services` as the run's services.
    #[must_use]
    pub fn with_services(mut self, services: HostServices) -> TestCall {
        self.services = services;
        self
    }

    /// Lends the context to one call.
    #[must_use]
    pub fn context(&self) -> ToolContext<'_> {
        ToolContext::new(&self.tool, &self.access, &self.origin, &self.services)
    }
}
