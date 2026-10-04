//! The context a unit test lends a web tool it calls directly.

use harness::plugin::{ToolCallOrigin, ToolCaller, ToolContext};
use harness::vfs::{Access, Origin, VfsRef};

/// An access over a fresh in-memory filesystem and a script origin, which
/// lend a [`ToolContext`] to a tool a test calls directly.
pub(crate) struct TestContext {
    access: Access,
    origin: ToolCallOrigin,
}

impl TestContext {
    /// A context over a fresh default filesystem, for a call from the
    /// script of the section `Test`.
    pub(crate) fn new() -> Self {
        let access = VfsRef::default()
            .acquire(Origin::new("web tool test"))
            .expect("a fresh memory filesystem acquires");
        let origin = ToolCallOrigin {
            execution: "web-tool-test".to_owned(),
            section: "Test".to_owned(),
            caller: ToolCaller::Script,
        };
        Self { access, origin }
    }

    /// Lends the context to one call.
    pub(crate) fn lend(&self) -> ToolContext<'_> {
        ToolContext::new(&self.access, &self.origin)
    }
}
