//! The suites' stand-in for the Harness's tool implementations:
//! [`TestTool`], and the [`TestToolTable`] a `ToolCall` effect's id
//! resolves in.
//!
//! The Engine holds no implementation and names no implementation trait;
//! the production trait (`Tool`) is the Harness's, in
//! `harness-capabilities`, and a `promptforge-*` crate never depends on a
//! Harness crate. The suites still need something to perform a `ToolCall`
//! effect with, so these are the test doubles: the same method shapes as
//! the Harness's trait (so a fixture reads like a production tool), built
//! into the [`Performers`] the tokio test driver takes by
//! [`RunHarness`](super::RunHarness). Nothing here reaches the Engine.
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

/// A fixture tool the tokio test driver dispatches a `ToolCall` effect to:
/// the suites' stand-in for the Harness's `Tool`.
///
/// The surface is the Harness trait's: a stable [`id`](TestTool::id), a
/// transport [`wire_name`](TestTool::wire_name), a model-facing
/// [`description`](TestTool::description), a JSON-Schema
/// [`parameters_schema`](TestTool::parameters_schema), the
/// [`structured_output`](TestTool::structured_output) flag, and the
/// future-returning [`call`](TestTool::call). [`descriptor`](TestTool::descriptor)
/// is the tool as data, what a suite installs in the run's catalog.
pub trait TestTool: Send + Sync {
    /// The tool's stable identity: the catalog key and what a `ToolCall`
    /// effect names.
    fn id(&self) -> ToolId;

    /// The transport name the tool is advertised under before aliasing.
    fn wire_name(&self) -> &str;

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
        ToolDescriptor::new(
            self.id(),
            self.wire_name(),
            self.description(),
            self.parameters_schema(),
        )
        .structured(self.structured_output())
    }

    /// Performs one call with `args`, as the Harness's tool performer
    /// would. The future resolves to the tool's output or its own
    /// model-safe [`ToolError`].
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
    /// Returns the catalog's construction error when a fixture has a
    /// transport-illegal wire name.
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
