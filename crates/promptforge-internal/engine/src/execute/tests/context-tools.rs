//! Fixture tool sets: a binding beside its implementation, a run's tool
//! set beside the Harness's tool table behind it, and the helpers that
//! arm a run state with them.

use super::*;

/// A binding for a fixture tool beside its implementation: the binding
/// goes into the run's tool set, the implementation into the Harness's
/// tool table [`arm_tools`] hands the driver, so a script or model call on
/// the alias resolves through the same id the binding journals.
pub(in super::super) fn fixture_binding(
    alias: &str,
    description: &str,
    tool: Arc<dyn TestTool>,
) -> (crate::lua::ToolBinding, Arc<dyn TestTool>) {
    let binding = crate::lua::ToolBinding::for_test(alias, description, &tool.descriptor());
    (binding, tool)
}

/// A run's tool set beside the implementations behind it: the set goes to
/// the run state (what the engine advertises and journals), the table to
/// this test's Harness (what the driver performs a `ToolCall` with).
/// A bare [`ToolSet`](crate::lua::ToolSet) converts into a fixture with no
/// implementations, for the tests whose tools are never called.
#[derive(Clone, Default)]
pub(in super::super) struct FixtureTools {
    set: crate::lua::ToolSet,
    table: TestToolTable,
}

impl FixtureTools {
    /// Builds the fixture from bindings paired with their implementations
    /// and the prompt-wide `always` aliases.
    pub(in super::super) fn new(
        bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
        always: Vec<String>,
    ) -> Self {
        let mut table = TestToolTable::new();
        let bindings = bindings
            .into_iter()
            .map(|(binding, tool)| {
                table.insert(tool);
                binding
            })
            .collect();
        Self {
            set: crate::lua::ToolSet::for_test(bindings, always),
            table,
        }
    }

    /// The bindings as the run's set, for a test that inspects them.
    pub(in super::super) fn set(&self) -> &crate::lua::ToolSet {
        &self.set
    }

    /// Installs the set on the run state and returns `host` carrying the
    /// implementations the driver's tool performer resolves.
    pub(in super::super) fn install(&self, ctx: &RunState, host: RunHost) -> RunHost {
        *ctx.tool_set()
            .lock()
            .expect("the tool set mutex is not poisoned") = self.set.clone();
        host.tools(self.table.clone())
    }
}

impl From<crate::lua::ToolSet> for FixtureTools {
    fn from(set: crate::lua::ToolSet) -> Self {
        Self {
            set,
            table: TestToolTable::new(),
        }
    }
}

/// Arms the run state's shared tool set with `bindings` (every alias
/// prompt-wide through `always`) and returns `host` carrying the
/// implementations, so `TokioDriver::new` performs the calls.
pub(in super::super) fn arm_tools(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
) -> RunHost {
    let always = bindings
        .iter()
        .map(|(binding, _)| binding.alias().to_owned())
        .collect();
    arm_tools_scoped(ctx, host, bindings, always)
}

/// Arms the run state's shared tool set with `bindings` and exactly
/// `always` as the prompt-wide scope, returning `host` carrying the
/// implementations.
pub(in super::super) fn arm_tools_scoped(
    ctx: &RunState,
    host: RunHost,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
    always: Vec<String>,
) -> RunHost {
    FixtureTools::new(bindings, always).install(ctx, host)
}

/// The test's tools as the two halves the Harness assembles from its
/// activated capabilities: the catalog of descriptors the run's
/// frontmatter tool slots (under `tests/tools`) fill against at prepare,
/// and the table of implementations the driver's tool performer resolves
/// a `ToolCall` effect's id in.
pub(in super::super) fn fixture_tools(
    tools: &[Arc<dyn TestTool>],
) -> (promptforge_types::tools::ToolCatalog, TestToolTable) {
    let table = TestToolTable::from_tools(tools);
    let catalog = table
        .catalog()
        .expect("the fixture tools have legal wire names and distinct ids");
    (catalog, table)
}
