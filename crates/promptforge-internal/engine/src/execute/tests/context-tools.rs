//! Fixture tool sets: a binding beside its implementation, a run's tool
//! set beside the test driver's tool table behind it, and the helpers that
//! arm a run state with them.

use std::fmt::Write as _;

use super::*;

/// A binding for a fixture tool beside its implementation: the binding
/// goes into the run's offering under `alias` as its wire name, the
/// implementation into the tool table [`arm_tools`] hands the test driver,
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

    /// One `tools.offer('<id>')` line per binding, in binding order, for a
    /// test section whose rounds use the fixture's tools; empty when the
    /// fixture binds nothing.
    pub(in super::super) fn offer_lines(&self) -> String {
        let mut lines = String::new();
        for binding in self.set.offered() {
            writeln!(lines, "tools.offer('{}')", binding.id())
                .expect("writing to a String cannot fail");
        }
        lines
    }

    /// Installs the set on the run state and returns `fixture` carrying the
    /// implementations the driver's tool performer resolves.
    pub(in super::super) fn install(&self, ctx: &RunState, fixture: RunFixture) -> RunFixture {
        *ctx.tool_set()
            .lock()
            .expect("the tool set mutex is not poisoned") = self.set.clone();
        fixture.tools(self.table.clone())
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

/// Arms the run state's shared tool set with `bindings`, none of them in
/// any section's scope until the section offers it, and returns `fixture`
/// carrying the implementations, so `TokioDriver::new` performs the calls.
pub(in super::super) fn arm_tools(
    ctx: &RunState,
    fixture: RunFixture,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
) -> RunFixture {
    FixtureTools::new(bindings).install(ctx, fixture)
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
