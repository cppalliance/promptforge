//! The suites' fixture tool implementations: [`TestTool`], the
//! [`TestToolTable`] a `ToolCall` effect's id resolves in, and, in this
//! crate's own tests, `FnTool`, a fixture tool written as a closure.
//!
//! The Engine holds no implementation and names no implementation trait;
//! the production trait (`Plugin`) is the Plugin contract's, in
//! `promptforge-plugin`, which the Engine never depends on. The suites
//! still need something to perform a `ToolCall` effect with, so these are
//! the test doubles: one tool per object, with its id, description,
//! schema, and call, built into the [`Performers`] the tokio test driver takes by
//! [`RunFixture`](super::RunFixture). Nothing here reaches the Engine.
//!
//! The async methods are declared in the boxed form
//! `#[async_trait::async_trait]` expands an `async fn` to, so a suite
//! writes its fixtures as `async fn` under that dev-only macro while the
//! Engine crate itself declares no async-trait dependency.
//!
//! [`Performers`]: super::Performers

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge_types::tools::{
    ToolCatalog, ToolCatalogError, ToolDescriptor, ToolError, ToolId, ToolOutput,
};

/// The future a fixture's async method returns: boxed, `Send`, and bounded
/// by the borrow of `self`, exactly as `#[async_trait::async_trait]`
/// expands an `async fn` impl.
type FixtureFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// A fixture tool the tokio test driver dispatches a `ToolCall` effect to.
///
/// The surface: a stable [`id`](TestTool::id), a
/// model-facing [`description`](TestTool::description), a JSON-Schema
/// [`parameters_schema`](TestTool::parameters_schema), the
/// [`structured_output`](TestTool::structured_output) flag, and the
/// future-returning [`call`](TestTool::call). [`descriptor`](TestTool::descriptor)
/// is the tool as data, what a suite installs in the run's catalog.
pub trait TestTool: Send + Sync {
    /// The tool's stable identity: the catalog key and what a `ToolCall`
    /// effect names.
    fn id(&self) -> ToolId;

    /// The one-sentence description the model reads.
    fn description(&self) -> &str;

    /// The JSON-Schema `object` the tool's arguments must match.
    fn parameters_schema(&self) -> serde_json::Value;

    /// Whether the output text is one JSON value resumed as data.
    fn structured_output(&self) -> bool {
        false
    }

    /// The tool as data: the descriptor the Engine binds and advertises.
    fn descriptor(&self) -> ToolDescriptor {
        ToolDescriptor::new(self.id(), self.description(), self.parameters_schema())
            .structured(self.structured_output())
    }

    /// Performs one call with `args`. The future resolves to the tool's
    /// output or its own model-safe [`ToolError`].
    fn call<'life0, 'async_trait>(
        &'life0 self,
        args: serde_json::Value,
    ) -> FixtureFuture<'async_trait, Result<ToolOutput, ToolError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait;
}

/// The fixture implementations behind a run's catalog, keyed by identity:
/// what the tokio test driver's tool performer resolves a `ToolCall`
/// effect's id in.
#[derive(Clone, Default)]
pub struct TestToolTable {
    tools: BTreeMap<ToolId, Arc<dyn TestTool>>,
}

impl TestToolTable {
    /// Builds an empty table.
    #[must_use]
    pub fn new() -> TestToolTable {
        TestToolTable::default()
    }

    /// Builds a table holding every tool in `tools`.
    #[must_use]
    pub fn from_tools(tools: &[Arc<dyn TestTool>]) -> TestToolTable {
        let mut table = TestToolTable::new();
        for tool in tools {
            table.insert(Arc::clone(tool));
        }
        table
    }

    /// Adds `tool` under its own identity; a repeated identity keeps the
    /// first implementation.
    pub fn insert(&mut self, tool: Arc<dyn TestTool>) {
        self.tools.entry(tool.id()).or_insert(tool);
    }

    /// Returns the implementation registered under `id`.
    #[must_use]
    pub fn get(&self, id: &ToolId) -> Option<Arc<dyn TestTool>> {
        self.tools.get(id).map(Arc::clone)
    }

    /// Returns whether the table is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    /// The table's tools as the catalog of descriptors the Engine binds
    /// against, in identity order.
    ///
    /// # Errors
    /// Returns the catalog's construction error when two descriptors share
    /// an id, which a table holding one tool per id never builds.
    pub fn catalog(&self) -> Result<ToolCatalog, ToolCatalogError> {
        let descriptors: Vec<ToolDescriptor> =
            self.tools.values().map(|tool| tool.descriptor()).collect();
        ToolCatalog::new(&descriptors)
    }
}

impl fmt::Debug for TestToolTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TestToolTable")
            .field("ids", &self.tools.keys().collect::<Vec<_>>())
            .finish()
    }
}

/// The closure an [`FnTool`] answers its calls with.
#[cfg(test)]
type FnToolHandler =
    dyn Fn(serde_json::Value) -> Result<ToolOutput, ToolError> + Send + Sync + 'static;

/// A fixture tool whose call is a closure, answered at once: the serial
/// test drivers call it directly, and the tokio test driver awaits it as a
/// ready future.
#[cfg(test)]
#[derive(Clone)]
pub(crate) struct FnTool {
    id: ToolId,
    description: String,
    schema: serde_json::Value,
    structured: bool,
    handler: Arc<FnToolHandler>,
}

#[cfg(test)]
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

    /// Answers one call with `args` at once.
    pub(crate) fn call_now(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError> {
        (self.handler)(args)
    }
}

#[cfg(test)]
impl TestTool for FnTool {
    fn id(&self) -> ToolId {
        self.id.clone()
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        self.schema.clone()
    }

    fn structured_output(&self) -> bool {
        self.structured
    }

    fn call<'life0, 'async_trait>(
        &'life0 self,
        args: serde_json::Value,
    ) -> FixtureFuture<'async_trait, Result<ToolOutput, ToolError>>
    where
        'life0: 'async_trait,
        Self: 'async_trait,
    {
        Box::pin(std::future::ready(self.call_now(args)))
    }
}
