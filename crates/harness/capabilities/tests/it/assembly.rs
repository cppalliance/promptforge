//! Catalog assembly and conflict checks: activation assembles the
//! activated capabilities' contributed tools into the run's catalog in
//! declaration order, enforcing tool prefix-containment at assembly, and
//! rejects capability co-activation conflicts naming both; the engine's
//! prepare fills exact slots against the catalog it is handed.

use std::sync::Arc;

use harness_capabilities::CapabilityRegistry;
use promptforge::tools::ToolId;
use promptforge::{Environment, RunErrorKind, RunResult};

use super::activation::DECLARES_REQUIRED;
use super::support::{
    BadWireTool, ToolFixture, captured_logs, context, described_tool, fixture_tool, parse,
    prepare_activated, run_activated,
};

/// A prompt declaring `promptforge/bashkit` and `promptforge/terminal`,
/// in that order.
const DECLARES_CONFLICTING: &str = concat!(
    "---\n",
    "name: declares-conflicting\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/bashkit\n",
    "  - promptforge/terminal\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` and `promptforge/fs`, in that
/// order.
const DECLARES_TWO: &str = concat!(
    "---\n",
    "name: declares-two\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "  - promptforge/fs\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// A prompt declaring `promptforge/web` and one exact tool slot.
const DECLARES_EXACT_SLOT: &str = concat!(
    "---\n",
    "name: declares-exact-slot\n",
    "description: d\n",
    "promptforge: 0\n",
    "capabilities:\n",
    "  - promptforge/web\n",
    "tools:\n",
    "  fetch: promptforge/web/fetch\n",
    "---\n\n",
    "# Title\n\n",
    "## Only\n\n",
    "Done.\n",
);

/// Registers `promptforge/web` contributing one described fetch tool.
fn web_registry() -> CapabilityRegistry {
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/web",
            &[],
            vec![described_tool(
                "promptforge/web/fetch",
                "Fetch a web page over HTTP",
            )],
        )))
        .expect("web registers");
    registry
}

#[test]
fn a_co_activation_conflict_fails_preparation_naming_both() {
    let prompt = parse(DECLARES_CONFLICTING, "declares-conflicting");
    // The check is symmetric: the conflict is found whether the earlier-
    // or the later-declared capability declares it.
    for (bashkit_conflicts, terminal_conflicts) in [
        (vec!["promptforge/terminal"], vec![]),
        (vec![], vec!["promptforge/bashkit"]),
    ] {
        let mut registry = CapabilityRegistry::new();
        registry
            .register(Arc::new(ToolFixture::new(
                "promptforge/bashkit",
                &bashkit_conflicts,
                vec![fixture_tool("promptforge/bashkit/run")],
            )))
            .expect("bashkit registers");
        registry
            .register(Arc::new(ToolFixture::new(
                "promptforge/terminal",
                &terminal_conflicts,
                vec![fixture_tool("promptforge/terminal/run")],
            )))
            .expect("terminal registers");
        let (ctx, requirements, activation) = prepare_activated(
            Environment::new(),
            Some(&registry),
            &prompt,
            context("prepare-conflict"),
        );
        assert!(!requirements.is_satisfied());
        let [conflict] = requirements.conflicts.as_slice() else {
            panic!(
                "exactly one conflict is reported: {:?}",
                requirements.conflicts
            );
        };
        // Both capabilities are named, in declaration order.
        assert_eq!(conflict.first.to_string(), "promptforge/bashkit");
        assert_eq!(conflict.second.to_string(), "promptforge/terminal");
        // A context gets one filesystem reality or the other, never
        // both: neither member of the conflicting pair activated, so
        // neither tool reached the catalog or the implementation table.
        assert!(ctx.tools().tools().is_empty());
        assert!(activation.tools.is_empty());
    }
}

#[test]
fn the_run_path_refuses_a_conflicting_pair_with_a_notice_naming_both() {
    let prompt = parse(DECLARES_CONFLICTING, "declares-conflicting");
    let mut registry = CapabilityRegistry::new();
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/bashkit",
            &["promptforge/terminal"],
            vec![],
        )))
        .expect("bashkit registers");
    registry
        .register(Arc::new(ToolFixture::new(
            "promptforge/terminal",
            &[],
            vec![],
        )))
        .expect("terminal registers");
    let result = run_activated(&registry, &prompt, context("refuse-conflict"));
    let RunResult::Failure(error) = result else {
        panic!("a conflicting pair is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    let notice = error.to_string();
    assert!(
        notice.contains("promptforge/bashkit") && notice.contains("promptforge/terminal"),
        "the notice names both conflicting capabilities: {notice}"
    );
}

#[test]
fn a_contributed_tool_outside_the_capabilitys_id_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let good = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let stray = ToolId::parse("promptforge/other/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/other/fetch"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
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
        logs.contains("promptforge/other/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the tool: {logs}"
    );
}

#[test]
fn the_catalog_assembles_contributed_tools_in_declaration_order() {
    let prompt = parse(DECLARES_TWO, "declares-two");
    let web = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/web/search"),
        ],
    );
    let fs = ToolFixture::new(
        "promptforge/fs",
        &[],
        vec![fixture_tool("promptforge/fs/read")],
    );
    let mut registry = CapabilityRegistry::new();
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
        [
            "promptforge/web/fetch",
            "promptforge/web/search",
            "promptforge/fs/read"
        ],
        "declaration order, then contribution order within each capability"
    );
}

#[test]
fn a_repeated_tool_id_across_contributions_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let repeated = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            fixture_tool("promptforge/web/fetch"),
            fixture_tool("promptforge/web/search"),
            // The repeat: one capability contributes the same id twice.
            fixture_tool("promptforge/web/fetch"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
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
        logs.contains("promptforge/web/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the repeated tool: {logs}"
    );
}

#[test]
fn a_transport_illegal_wire_name_is_rejected_at_assembly() {
    let prompt = parse(DECLARES_REQUIRED, "declares-required");
    let bad = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let fixture = ToolFixture::new(
        "promptforge/web",
        &[],
        vec![
            Arc::new(BadWireTool {
                id: bad.clone(),
                wire: "fetch/v2".to_owned(),
            }),
            fixture_tool("promptforge/web/search"),
        ],
    );
    let mut registry = CapabilityRegistry::new();
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
        logs.contains("promptforge/web/fetch") && logs.contains("promptforge/web"),
        "the rejection log names the capability and the rejected tool: {logs}"
    );
}

#[test]
fn a_capability_both_activation_and_prepare_report_missing_is_named_once() {
    let prompt = parse(DECLARES_EXACT_SLOT, "declares-exact-slot");
    // The declared capability is absent from an empty registry (activation
    // reports it) and its exact slot finds nothing in the catalog (prepare
    // reports it): the merged refusal names it once.
    let result = run_activated(&CapabilityRegistry::new(), &prompt, context("refuse-once"));
    let RunResult::Failure(error) = result else {
        panic!("an absent required capability is refused: {result:?}");
    };
    assert_eq!(error.kind(), RunErrorKind::RequirementsUnmet);
    assert_eq!(
        error.to_string(),
        "the environment cannot satisfy this prompt:\n\
         - missing required capability: promptforge/web"
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
    let id = ToolId::parse("promptforge/web/fetch").expect("the id is valid");
    let bindings = ctx.tool_bindings();
    assert_eq!(bindings.len(), 1);
    // Handles resolve alias -> id -> descriptor; the implementation is the
    // host's, in the activation's table under the same id.
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
