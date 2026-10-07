//! Catalog assembly: activation assembles the activated Plugins'
//! contributed tools into the run's catalog in declaration order,
//! enforcing tool prefix-containment at assembly; the Engine's prepare
//! fills exact slots against the catalog it is handed.

use std::sync::Arc;

use harness_plugins::PluginRegistry;
use promptforge::tools::ToolId;
use promptforge::{Environment, RunErrorKind, RunResult};

use super::activation::DECLARES_REQUIRED;
use super::support::{
    BadWireTool, ToolFixture, captured_logs, context, described_tool, fixture_tool, parse,
    prepare_activated, run_activated,
};

/// A prompt declaring `web` and `fs`, in that
/// order.
const DECLARES_TWO: &str = concat!(
    "---\n",
    "name: declares-two\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - web\n",
    "  - fs\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `web` and one exact tool slot.
const DECLARES_EXACT_SLOT: &str = concat!(
    "---\n",
    "name: declares-exact-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "plugins:\n",
    "  - web\n",
    "tools:\n",
    "  fetch: web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Registers `web` contributing one described fetch tool.
fn web_registry() -> PluginRegistry {
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "web",
            vec![described_tool("web/fetch", "Fetch a web page over HTTP")],
        )))
        .expect("web registers");
    registry
}

#[test]
fn a_contributed_tool_outside_the_plugins_id_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let good = ToolId::parse("web/fetch").expect("the id is valid");
    let stray = ToolId::parse("other/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "web",
        vec![fixture_tool("web/fetch"), fixture_tool("other/fetch")],
    );
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let logs = captured_logs(|| {
        let (ctx, requirements, activation) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-containment"),
        );
        // Containment is enforced at assembly, not reported: the run is
        // satisfiable and the stray tool simply never enters the catalog
        // or the implementation table.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(
            catalog.get(&good).is_some(),
            "the contained tool is assembled"
        );
        assert!(
            catalog.get(&stray).is_none(),
            "the containment violation is rejected at assembly"
        );
        assert_eq!(catalog.tools().len(), 1);
        assert!(activation.tools.get(&good).is_some());
        assert!(activation.tools.get(&stray).is_none());
    });
    assert!(
        logs.contains("other/fetch") && logs.contains("web"),
        "the rejection log names the Plugin and the tool: {logs}"
    );
}

#[test]
fn the_catalog_assembles_contributed_tools_in_declaration_order() {
    let prompt = parse(DECLARES_TWO, "declares-two");
    let web = ToolFixture::new(
        "web",
        vec![fixture_tool("web/fetch"), fixture_tool("web/search")],
    );
    let fs = ToolFixture::new("fs", vec![fixture_tool("fs/read")]);
    let mut registry = PluginRegistry::new();
    registry.register(Arc::new(web)).expect("web registers");
    registry.register(Arc::new(fs)).expect("fs registers");
    let (ctx, requirements, _) = prepare_activated(
        Environment::new(),
        Some(&registry),
        &prompt,
        context("prepare-order"),
    );
    assert!(requirements.is_satisfied());
    let ids: Vec<String> = ctx
        .tools()
        .tools()
        .iter()
        .map(|tool| tool.id.to_string())
        .collect();
    assert_eq!(
        ids,
        ["web/fetch", "web/search", "fs/read"],
        "declaration order, then contribution order within each Plugin"
    );
}

#[test]
fn a_repeated_tool_id_across_contributions_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let repeated = ToolId::parse("web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "web",
        vec![
            fixture_tool("web/fetch"),
            fixture_tool("web/search"),
            // The repeat: one Plugin contributes the same id twice.
            fixture_tool("web/fetch"),
        ],
    );
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let logs = captured_logs(|| {
        let (ctx, requirements, _) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-duplicate"),
        );
        // The repeat is rejected at assembly, not reported: the first
        // contribution stands and the run is satisfiable.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(catalog.get(&repeated).is_some());
        assert_eq!(
            catalog.tools().len(),
            2,
            "the repeated id enters the catalog exactly once"
        );
    });
    assert!(
        logs.contains("web/fetch") && logs.contains("web"),
        "the rejection log names the Plugin and the repeated tool: {logs}"
    );
}

#[test]
fn a_transport_illegal_wire_name_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let bad = ToolId::parse("web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "web",
        vec![
            Arc::new(BadWireTool {
                id: bad.clone(),
                wire: "fetch/v2".to_owned(),
            }),
            fixture_tool("web/search"),
        ],
    );
    let mut registry = PluginRegistry::new();
    registry
        .register(Arc::new(fixture))
        .expect("the fixture registers");
    let logs = captured_logs(|| {
        let (ctx, requirements, _) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-wire-name"),
        );
        // One bad tool costs only itself: the run is satisfiable and
        // the well-formed tool still assembles.
        assert!(requirements.is_satisfied());
        let catalog = ctx.tools();
        assert!(
            catalog.get(&bad).is_none(),
            "the illegal wire name is rejected at assembly"
        );
        assert_eq!(catalog.tools().len(), 1);
    });
    assert!(
        logs.contains("web/fetch") && logs.contains("web"),
        "the rejection log names the Plugin and the rejected tool: {logs}"
    );
}

#[test]
fn a_plugin_both_activation_and_prepare_report_missing_is_named_once() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    // The declared Plugin is absent from an empty registry (activation
    // reports it) and its exact slot finds nothing in the catalog (prepare
    // reports it): the merged refusal names it once.
    let result = run_activated(&PluginRegistry::new(), &prompt, context("refuse-once"));
    let RunResult::Failure(error) = result else {
        panic!("an absent required Plugin is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - missing required Plugin: web"
    );
}

#[test]
fn an_exact_slot_fills_against_the_activated_catalog() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    let (ctx, requirements, activation) = prepare_activated(
        Environment::new(),
        Some(&web_registry()),
        &prompt,
        context("fill-exact"),
    );
    assert!(requirements.is_satisfied());
    let id = ToolId::parse("web/fetch").expect("the id is valid");
    let bindings = ctx.tool_bindings();
    assert_eq!(bindings.len(), 1);
    // Handles resolve alias -> id -> descriptor; the implementation is the
    // Harness's, in the activation's table under the same id.
    assert_eq!(bindings.alias_id("fetch"), Some(&id));
    assert_eq!(
        bindings.resolve("fetch").map(|tool| tool.id.clone()),
        Some(id.clone())
    );
    assert_eq!(
        bindings
            .resolve("fetch")
            .map(|tool| tool.description.as_str()),
        Some("Fetch a web page over HTTP")
    );
    assert!(bindings.tool(&id).is_some());
    assert!(bindings.resolve("undeclared").is_none());
    assert!(activation.tools.get(&id).is_some());
}
