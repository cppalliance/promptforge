//! Tests for model-visible tool scoping through `tools.offer`.

use promptforge_parser::test_support::one_section;

use super::fixtures::scoped_tool;
use super::models_loop::{LOOP_TO_TEXT, loop_scenario};
use super::*;
use crate::lua::{ToolBinding, ToolSet};
use crate::test_support::scenario::TEST_SEED;

/// The tool set offering `tool` under the wire name `alias`, described as
/// `description`, for a test that reads the offering without a run.
fn offering_of(alias: &str, description: &str, tool: &FnTool) -> ToolSet {
    ToolSet::new(
        Vec::new(),
        std::collections::BTreeMap::new(),
        vec![ToolBinding::for_test(
            alias,
            description,
            &tool.descriptor(),
        )],
    )
}

/// An offered tool stays out of the model-visible scope until
/// `tools.offer` names it: the scope snapshot over an untouched runtime is
/// empty. (The advertised-set half of this rule is the loop's schema
/// build, pinned by the offer tests below.)
#[test]
fn offered_tools_are_not_injected_without_an_offer() {
    let (tool, _) = scoped_tool("concrete", "Concrete description.");
    let tools = offering_of("tools_concrete", "capability", &tool);
    let runtime = Mutex::new(promptforge_lua::ToolRuntime::default());
    let effective = current_tool_bindings(&tools, &runtime).expect("the scope must snapshot");
    assert!(
        effective.is_empty(),
        "an offering must not expose a tool without an explicit offer"
    );
}

#[test]
fn an_offered_tool_advertises_its_concrete_schema_under_its_wire_name_and_dispatches_by_id() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("aliased_call", "tools_concrete", "{\"value\":\"payload\"}")]),
        text("aliased final"),
    ]);
    let (tool, called) = scoped_tool("concrete", "Concrete description.");
    let tools = offering_of("tools_concrete", "Concrete description.", &tool);
    let runtime = Mutex::new(promptforge_lua::ToolRuntime {
        added: vec!["tools_concrete".to_owned()],
        ..promptforge_lua::ToolRuntime::default()
    });
    let effective = current_tool_bindings(&tools, &runtime).expect("the offered scope snapshots");
    let (schemas, _) = prepare_scoped_tools(&effective, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name(), "tools_concrete");
    assert_eq!(schemas[0].description(), "Concrete description.");

    let source = one_section(&format!("tools.offer('tools/concrete')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("tools_concrete", tool)
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "aliased final");
    assert_eq!(called.load(Ordering::SeqCst), 1);
    let bodies = gateway.requests();
    let function = &bodies[0].tools[0];
    assert_eq!(function.name(), "tools_concrete");
    assert_eq!(function.description(), "Concrete description.");
    assert_eq!(
        function.parameters(),
        &json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    );
}

#[test]
fn a_section_offer_by_id_scopes_the_tool_and_dispatches_it() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("aliased_call", "tools_section", "{\"value\":\"payload\"}")]),
        text("aliased final"),
    ]);
    let (tool, called) = scoped_tool("section", "Section concrete.");
    let tools = offering_of("tools_section", "capability", &tool);
    let vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(&TEST_SEED),
        &Arc::new(tools),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Only",
    )
    .expect("the section VM builds")
    .inject_values("", &json!({}), &fresh_access())
    .expect("values must inject");

    // The section's `tools.offer` lands in its tool runtime; the scope
    // snapshot over it includes the offered tool's wire name.
    let offer = LuaProgram::compile(
        "tools.offer('tools/section')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Only",
    )
    .expect("the offer chunk must compile");
    vm.run_chunk(&offer, &null_emitter(), "Only")
        .expect("tools.offer must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name(), "tools_section");
    vm.teardown(&null_emitter(), "Only");

    // The same `tools.offer` inside a section scopes the tool for the
    // loop's rounds: the round advertises it and dispatches the concrete
    // tool behind it.
    let source = one_section(&format!("tools.offer('tools/section')\n{LOOP_TO_TEXT}"));
    let trace = loop_scenario(source)
        .offer("tools_section", tool)
        .chat(&gateway)
        .run();

    assert_eq!(trace.text(), "aliased final");
    assert_eq!(called.load(Ordering::SeqCst), 1);
    assert_eq!(gateway.requests()[0].tools[0].name(), "tools_section");
}
