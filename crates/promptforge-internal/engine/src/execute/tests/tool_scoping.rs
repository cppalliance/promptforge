//! Tests for model-visible tool scoping through `tools.always_offer` and
//! `tools.offer`.

use super::models_loop::{loop_context, loop_prompt};
use super::*;
use crate::test_support::tokio_driver::TokioDriver;

/// The one-section loop every scoping test drives: one user message, then
/// the terminal record's text.
const LOOP_TO_TEXT: &str = "local msgs = messages.new()\n\
     msgs:user('Use the tool.')\n\
     models.loop(msgs)\n\
     return msgs[#msgs].content";

/// An offered tool stays out of the model-visible scope until
/// `tools.always_offer` or `tools.offer` names it: the scope snapshot over
/// an untouched runtime is empty. (The advertised-set half of this rule is
/// the loop's schema build, pinned by the offer tests below.)
#[test]
fn offered_tools_are_not_injected_without_an_offer() {
    let tool: Arc<dyn TestTool> =
        Arc::new(ScopedFixtureTool::new("concrete", "Concrete description."));
    let tools = FixtureTools::new(
        vec![fixture_binding("tools_concrete", "capability", tool)],
        Vec::new(),
    );
    let runtime = Mutex::new(promptforge_lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
        allowed_tasks: None,
    });
    let effective = current_tool_bindings(tools.set(), &runtime).expect("the scope must snapshot");
    assert!(
        effective.is_empty(),
        "an offering must not expose a tool without an explicit offer"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn always_offer_advertises_the_concrete_schema_under_its_wire_name_and_dispatches_by_id() {
    let gateway = ScriptedChat::new(aliased_tool_script("tools_concrete"));
    let tool = Arc::new(ScopedFixtureTool::new("concrete", "Concrete description."));
    let tools = FixtureTools::new(
        vec![fixture_binding(
            "tools_concrete",
            "Concrete description.",
            Arc::clone(&tool) as Arc<dyn TestTool>,
        )],
        vec!["tools_concrete".to_owned()],
    );
    let runtime = Mutex::new(promptforge_lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
        allowed_tasks: None,
    });
    let effective =
        current_tool_bindings(tools.set(), &runtime).expect("the always scope snapshots");
    let (schemas, _) = prepare_scoped_tools(&effective, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(tool_schema_name(&schemas[0]), "tools_concrete");
    assert_eq!(
        tool_schema_description(&schemas[0]),
        "Concrete description."
    );

    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let (ctx, fixture) = loop_context(&prompt, tools);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the prompt-wide tool dispatches");

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
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

#[tokio::test(flavor = "current_thread")]
async fn a_section_offer_by_id_scopes_the_tool_and_dispatches_it() {
    let gateway = ScriptedChat::new(aliased_tool_script("tools_section"));
    let tool = Arc::new(ScopedFixtureTool::new("section", "Section concrete."));
    let tools = FixtureTools::new(
        vec![fixture_binding(
            "tools_section",
            "capability",
            Arc::clone(&tool) as Arc<dyn TestTool>,
        )],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x7e57),
        &Arc::new(Mutex::new(tools.set().clone())),
        &Arc::new(Mutex::new(ModelSet::default())),
        &null_emitter(),
        "Only",
    )
    .expect("the section VM builds");
    vm.inject_values("", &json!({}), &fresh_access())
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
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(tool_schema_name(&schemas[0]), "tools_section");
    vm.teardown(&null_emitter(), "Only");

    // The same `tools.offer` inside a section scopes the tool for the
    // loop's rounds: the round advertises it and dispatches the concrete
    // tool behind it.
    let md = loop_prompt(&format!("tools.offer('tools/section')\n{LOOP_TO_TEXT}"));
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, tools);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the section-offered tool dispatches");

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(gateway.requests()[0].tools[0].name(), "tools_section");
}
