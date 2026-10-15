//! Fixture tool sets: a binding beside its implementation, a run's tool
//! set beside the test driver's tool table behind it, and the helper that
//! splits fixture tools into a catalog and a table.

use super::*;

/// A binding for a fixture tool beside its implementation: the binding
/// goes into the run's offering under `alias` as its wire name, the
/// implementation into the tool table [`FixtureTools`] hands the test driver,
/// so a model call on the wire name or a script call on the id resolves
/// through the same id the binding journals.
pub(in super::super) fn fixture_binding(
    alias: &str,
    description: &str,
    tool: Arc<dyn TestTool>,
) -> (crate::lua::ToolBinding, Arc<dyn TestTool>) {
    let binding = crate::lua::ToolBinding::for_test(alias, description, &tool.descriptor());
    (binding, tool)
}

/// A run's tool set beside the implementations behind it: the set goes to
/// the run state (what the Engine advertises and journals), the table to
/// this test's fixture (what the test driver performs a `ToolCall` with).
/// A bare [`ToolSet`](crate::lua::ToolSet) converts into a fixture with no
/// implementations, for the tests whose tools are never called.
#[derive(Clone, Default)]
pub(in super::super) struct FixtureTools {
    set: crate::lua::ToolSet,
    table: TestToolTable,
}

impl FixtureTools {
    /// Builds the fixture from offered bindings paired with their
    /// implementations, none of them in any section's scope until the
    /// section offers it.
    pub(in super::super) fn new(
        bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
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
            set: crate::lua::ToolSet {
                offered: bindings,
                ..crate::lua::ToolSet::default()
            },
            table,
        }
    }

    /// The bindings as the run's set, for a test that inspects them.
    pub(in super::super) fn set(&self) -> &crate::lua::ToolSet {
        &self.set
    }

    /// Arms the run state with the set and returns it beside `fixture`
    /// carrying the implementations the driver's tool performer resolves.
    pub(in super::super) fn install(
        self,
        ctx: RunState,
        fixture: RunFixture,
    ) -> (RunState, RunFixture) {
        (ctx.with_tool_set(self.set), fixture.tools(self.table))
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

/// The test's tools as two halves: the catalog of descriptors the run
/// offers from, and the table of implementations the driver's tool
/// performer resolves a `ToolCall` effect's id in.
pub(in super::super) fn fixture_tools(
    tools: &[Arc<dyn TestTool>],
) -> (promptforge_types::tools::ToolCatalog, TestToolTable) {
    let table = TestToolTable::from_tools(tools);
    let catalog = table
        .catalog()
        .expect("the fixture tools have distinct ids");
    (catalog, table)
}
