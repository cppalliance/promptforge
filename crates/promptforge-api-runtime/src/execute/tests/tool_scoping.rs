use super::models_loop::{loop_context, loop_prompt};
use super::*;
use crate::execute::scheduler::Scheduler;

/// The one-section loop every scoping test drives: one user message, then
/// the terminal record's text.
const LOOP_TO_TEXT: &str = "local msgs = messages.new()\n\
     msgs:user('Use the tool.')\n\
     models.loop(msgs)\n\
     return msgs[#msgs].content";

/// A bound tool stays out of the model-visible scope until `tools.always`
/// or `tools.add` names it: the scope snapshot over an untouched runtime is
/// empty. (The advertised-set half of this rule is the loop's schema build,
/// pinned by the always/add tests below.)
#[test]
fn declared_tools_are_not_injected_without_always_or_add() {
    let tool: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let tool_set = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "local_alias",
            "capability",
            tool,
        )],
        Vec::new(),
    );
    let runtime = Mutex::new(promptforge_lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });
    let effective = current_tool_bindings(&tool_set, &runtime).expect("the scope must snapshot");
    assert!(
        effective.is_empty(),
        "declaring a bind must not expose it without explicit scope"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn always_advertises_concrete_schema_under_local_alias_and_dispatches_by_id() {
    let gateway = ScriptedGateway::start(aliased_tool_script("local_alias")).await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let tool_set = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "local_alias",
            "capability",
            Arc::clone(&tool) as Arc<dyn Tool>,
        )],
        vec!["local_alias".to_owned()],
    );
    let runtime = Mutex::new(promptforge_lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });
    let effective = current_tool_bindings(&tool_set, &runtime).expect("the always scope snapshots");
    let (schemas, _) = prepare_scoped_tools(&effective, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "local_alias");
    assert_eq!(schemas[0].description, "Concrete description.");

    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, tool_set);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the always-scoped alias dispatches");

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = gateway.requests();
    let function = &bodies[0]["tools"][0]["function"];
    assert_eq!(function["name"], "local_alias");
    assert_eq!(function["description"], "Concrete description.");
    assert_eq!(
        function["parameters"],
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    );
    assert_ne!(function["name"], "canonical_wire");
}

#[tokio::test(flavor = "current_thread")]
async fn h2_add_scopes_an_alias_and_dispatches_the_concrete_tool() {
    let gateway = ScriptedGateway::start(aliased_tool_script("section_tool")).await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    let bindings = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "section_tool",
            "capability",
            Arc::clone(&tool) as Arc<dyn Tool>,
        )],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        &Arc::new(Mutex::new(bindings)),
        &Arc::new(Mutex::new(ModelSet::default())),
        EXECUTION,
        &NullObserver::default(),
        "Only",
    )
    .expect("captured bindings must install");
    vm.install_captured_bindings()
        .expect("alias globals must install");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host must inject");

    // The H2 `tools.add` lands in the section's tool runtime; the scope
    // snapshot over it carries the added alias.
    let add = LuaProgram::compile(
        "tools.add('section_tool')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver::default(),
        "Only",
    )
    .expect("the add chunk must compile");
    vm.run_chunk(&add, &NullObserver::default(), "Only")
        .expect("tools.add must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles().expect("the bag snapshots");
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, _) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "section_tool");
    vm.teardown(&NullObserver::default(), "Only");

    // The same `tools.add` inside a section scopes the alias for the
    // loop's rounds: the round advertises it and dispatches the concrete
    // tool behind it.
    let md = loop_prompt(&format!("tools.add('section_tool')\n{LOOP_TO_TEXT}"));
    let prompt = parse(&md);
    let bindings = crate::lua::ToolSet::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "section_tool",
            "capability",
            Arc::clone(&tool) as Arc<dyn Tool>,
        )],
        Vec::new(),
    );
    let ctx = loop_context(&prompt, bindings);
    let out = Scheduler::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the section-scoped alias dispatches");

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gateway.requests()[0]["tools"][0]["function"]["name"],
        "section_tool"
    );
}
