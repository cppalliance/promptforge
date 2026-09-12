use super::super::*;
use super::*;

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
    let runtime = Mutex::new(crate::lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });
    let effective = current_tool_bindings(&tool_set, &runtime).expect("the scope must snapshot");
    assert!(
        effective.is_empty(),
        "declaring a bind must not expose it without explicit scope"
    );
}

#[tokio::test]
async fn always_advertises_concrete_schema_under_local_alias_and_dispatches_by_id() {
    let gateway = ScriptedGateway::start(aliased_tool_script("local_alias")).await;
    let client = gateway_client(gateway.addr());
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
    let runtime = Mutex::new(crate::lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });
    let effective = current_tool_bindings(&tool_set, &runtime).expect("the always scope snapshots");
    let (schemas, dispatch) = prepare_scoped_tools(&effective, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "local_alias");
    assert_eq!(schemas[0].description, "Concrete description.");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .unwrap();

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

#[tokio::test]
async fn h2_add_scopes_an_alias_and_dispatches_the_concrete_tool() {
    let gateway = ScriptedGateway::start(aliased_tool_script("section_tool")).await;
    let client = gateway_client(gateway.addr());
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
        &bindings,
        &ModelSet::default(),
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
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let scope =
        current_tool_bindings(&tool_bindings, &tool_runtime).expect("tool scope must snapshot");
    let (schemas, dispatch) = prepare_scoped_tools(&scope, &[]).expect("schemas must build");
    assert_eq!(schemas.len(), 1);
    assert_eq!(schemas[0].name, "section_tool");
    vm.teardown(&NullObserver::default(), "Only");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let (out, _) = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gateway.requests()[0]["tools"][0]["function"]["name"],
        "section_tool"
    );
}

/// The clash check is per scope: each half of a recorded near-duplicate pair
/// validates clean on its own, so two sections that each add one half stay
/// valid. (Both halves in one scope fail; that rule is pinned by the
/// always-scope test below.)
#[test]
fn near_duplicate_tools_are_valid_when_isolated_in_separate_scopes() {
    let first: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "first",
        "first_wire",
        "First concrete.",
    ));
    let second: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "second",
        "second_wire",
        "Second concrete.",
    ));
    let mut first_binding =
        crate::lua::ToolBinding::for_test("first_local", "first", Arc::clone(&first));
    first_binding.conflicts.push(crate::lua::Conflict {
        alias: "second_local".to_owned(),
        similarity: 0.98,
    });
    let mut second_binding =
        crate::lua::ToolBinding::for_test("second_local", "second", Arc::clone(&second));
    second_binding.conflicts.push(crate::lua::Conflict {
        alias: "first_local".to_owned(),
        similarity: 0.98,
    });

    for isolated in [vec![first_binding], vec![second_binding]] {
        prepare_effective_scope(&isolated, &[], EXECUTION, &NullObserver::default(), "Only")
            .expect("an isolated scope must validate");
    }
}

/// The always-scope path: both halves of a bind-time clash enter every
/// section's scope through the `always` list, and the per-block scope
/// rebuild fails on them. (An end-to-end twin-scope failure needs the real
/// model to score two descriptions at or above the 0.98 duplicate
/// threshold, which near-identical bind capabilities cannot reach
/// deterministically - so the always path is pinned here, one layer up
/// from the scope check.)
#[test]
fn near_duplicate_always_scope_fails_at_the_scope_rebuild() {
    let first: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "first",
        "first_wire",
        "First concrete.",
    ));
    let second: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "second",
        "second_wire",
        "Second concrete.",
    ));
    let mut first_binding =
        crate::lua::ToolBinding::for_test("first_local", "first", Arc::clone(&first));
    first_binding.conflicts.push(crate::lua::Conflict {
        alias: "second_local".to_owned(),
        similarity: 0.98,
    });
    let mut second_binding =
        crate::lua::ToolBinding::for_test("second_local", "second", Arc::clone(&second));
    second_binding.conflicts.push(crate::lua::Conflict {
        alias: "first_local".to_owned(),
        similarity: 0.98,
    });
    let tool_set = crate::lua::ToolSet::for_test(
        vec![first_binding, second_binding],
        vec!["first_local".to_owned(), "second_local".to_owned()],
    );
    let runtime = Mutex::new(crate::lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });

    let effective = current_tool_bindings(&tool_set, &runtime).expect("the always scope snapshots");
    let error =
        prepare_effective_scope(&effective, &[], EXECUTION, &NullObserver::default(), "Only")
            .unwrap_err();

    assert!(
        matches!(error, Error::NearDuplicateTools { .. }),
        "the always scope must fail on the recorded clash: {error}"
    );
}

#[test]
fn near_duplicate_effective_scope_fails_before_the_model_without_payload_reports() {
    let first: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "first",
        "first_wire",
        "First concrete.",
    ));
    let second: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "second",
        "second_wire",
        "Second concrete.",
    ));
    // Mirror the bind-time record: each half of the clash carries the other
    // half's alias and the picker's score.
    let mut first_binding =
        crate::lua::ToolBinding::for_test("first_local", "first", Arc::clone(&first));
    first_binding.conflicts.push(crate::lua::Conflict {
        alias: "second_local".to_owned(),
        similarity: 0.98,
    });
    let mut second_binding =
        crate::lua::ToolBinding::for_test("second_local", "second", Arc::clone(&second));
    second_binding.conflicts.push(crate::lua::Conflict {
        alias: "first_local".to_owned(),
        similarity: 0.98,
    });
    let bindings = vec![first_binding, second_binding];
    let recorder = Arc::new(Recorder::default());

    let error =
        prepare_effective_scope(&bindings, &[], EXECUTION, recorder.as_ref(), "Only").unwrap_err();

    assert!(matches!(
        error,
        Error::NearDuplicateTools {
            diagnostic,
        } if diagnostic.first_alias == "first_local"
            && diagnostic.first_id == ToolId::new("tests", "first").expect("valid id")
            && diagnostic.second_alias == "second_local"
            && diagnostic.second_id == ToolId::new("tests", "second").expect("valid id")
            && (diagnostic.similarity - 0.98).abs() < f64::EPSILON
    ));
    let events = recorder.events();
    assert!(
        events
            .iter()
            .any(|(_, detail)| { *detail == detail::TOOL_SCOPE_VALIDATION_FAILED.to_string() })
    );
    assert!(!events.iter().any(|(_, detail)| {
        *detail == detail::MODEL_TURN_COMPLETED.to_string()
            || *detail == detail::MODEL_TURN_FAILED.to_string()
    }));
    let trace = format!("{events:?}");
    for payload in ["first_local", "second_local", "Private similar description"] {
        assert!(!trace.contains(payload), "observation leaked {payload:?}");
    }
}
