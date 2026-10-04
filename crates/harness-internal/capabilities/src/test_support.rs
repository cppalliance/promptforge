//! The context a unit test lends a tool it calls directly.

use promptforge::effect::{ToolCallOrigin, ToolCaller};
use promptforge::vfs::{Access, Origin, VfsRef};

use crate::ToolContext;

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
            .acquire(Origin::new("tool test"))
            .expect("a fresh memory filesystem acquires");
        let origin = ToolCallOrigin {
            execution: "tool-test".to_owned(),
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
