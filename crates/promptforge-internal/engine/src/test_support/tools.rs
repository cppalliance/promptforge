//! The suites' fixture tool: [`FnTool`], a tool written as a closure.
//!
//! The Engine holds no implementation and names no implementation trait;
//! the production trait (`Plugin`) is the Plugin contract's, in
//! `promptforge-plugin`, which the Engine never depends on. The suites
//! still need something to perform a `ToolCall` effect with, so this is
//! the test double: one tool per value, with its id, description, schema,
//! and call, which a scenario answers the effect through. Nothing here
//! reaches the Engine.

use std::sync::Arc;

use promptforge_types::tools::{ToolDescriptor, ToolError, ToolId, ToolOutput};

/// The closure an [`FnTool`] answers its calls with.
type FnToolHandler =
    dyn Fn(serde_json::Value) -> Result<ToolOutput, ToolError> + Send + Sync + 'static;

/// A fixture tool whose call is a closure, answered at once by the serial
/// test drivers.
#[derive(Clone)]
pub(crate) struct FnTool {
    id: ToolId,
    description: String,
    schema: serde_json::Value,
    structured: bool,
    handler: Arc<FnToolHandler>,
}

impl FnTool {
    /// The tool `id`, whose arguments match `schema` and whose calls
    /// `handler` answers, with an empty description and plain output.
    #[track_caller]
    pub(crate) fn new(
        id: &str,
        schema: serde_json::Value,
        handler: impl Fn(serde_json::Value) -> Result<ToolOutput, ToolError> + Send + Sync + 'static,
    ) -> FnTool {
        let id = ToolId::parse(id).unwrap_or_else(|error| panic!("tool id {id:?}: {error}"));
        FnTool {
            id,
            description: String::new(),
            schema,
            structured: false,
            handler: Arc::new(handler),
        }
    }

    /// Replaces the description the model reads.
    pub(crate) fn description(mut self, description: &str) -> FnTool {
        description.clone_into(&mut self.description);
        self
    }

    /// Marks the output text as one JSON value, so a binding built from
    /// the descriptor resumes a script call as a Lua table.
    pub(crate) fn structured(mut self) -> FnTool {
        self.structured = true;
        self
    }

    /// The tool's stable identity: the catalog key and what a `ToolCall`
    /// effect names.
    pub(crate) fn id(&self) -> ToolId {
        self.id.clone()
    }

    /// The tool as data: the descriptor the Engine binds and advertises,
    /// what a scenario installs in the run's catalog.
    pub(crate) fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::new(self.id(), &self.description, self.schema.clone())
            .structured(self.structured)
    }

    /// Answers one call with `args` at once.
    pub(crate) fn call_now(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        (self.handler)(args)
    }
}
