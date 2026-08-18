use super::super::*;
use super::*;

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "covers cache hit, generation bump rebuild, and count persistence in one bag lifecycle"
)]
fn tool_bag_caches_on_unchanged_generation() {
    let bindings = crate::lua::ToolBindings::for_test(
        vec![
            crate::lua::ToolBinding::for_test(
                "echo",
                "echo tool",
                ToolId::new("tests", "echo").expect("valid id"),
            ),
            crate::lua::ToolBinding::for_test(
                "fetch",
                "fetch tool",
                ToolId::new("tests", "fetch").expect("valid id"),
            ),
        ],
        Vec::new(),
    );
    let mut vm = SectionVm::new_for_section(
        None,
        &bindings,
        &ModelBindings::default(),
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_echo = LuaProgram::compile(
        "tools.add('echo')",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_echo, &NullObserver, "Bag")
        .expect("tools.add(echo) must succeed");

    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    {
        let runtime = tool_runtime.lock().expect("runtime mutex");
        assert_eq!(
            runtime.generation(),
            1,
            "first tools.add must bump generation"
        );
    }
    let mut bag = ToolBag::new(tool_bindings, Arc::clone(&tool_runtime));
    let echo = EchoTool;
    let fetch = FetchTool;
    let registry =
        ToolRegistry::new([&echo as &dyn Tool, &fetch as &dyn Tool]).expect("unique test registry");

    let first = bag
        .prepare(&registry)
        .expect("first prepare must build schemas");
    assert!(!first.reused, "first prepare must rebuild");
    assert_eq!(first.schemas.len(), 1);
    assert_eq!(first.schemas[0].name, "echo");

    let second = bag
        .prepare(&registry)
        .expect("second prepare must reuse cache");
    assert!(second.reused, "unchanged generation must reuse cache");
    assert_eq!(
        second
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
        first
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
    );
    assert_eq!(second.dispatch, first.dispatch);

    let add_fetch = LuaProgram::compile(
        "tools.add('fetch')",
        "prologue-2",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Bag",
    )
    .expect("second prologue must compile");
    vm.run_chunk(&add_fetch, &NullObserver, "Bag")
        .expect("tools.add(fetch) must succeed");
    {
        let runtime = tool_runtime.lock().expect("runtime mutex");
        assert_eq!(
            runtime.generation(),
            2,
            "second tools.add must bump generation"
        );
    }

    let third = bag
        .prepare(&registry)
        .expect("prepare after mutation must rebuild");
    assert!(!third.reused, "generation mismatch must rebuild");
    assert_eq!(third.schemas.len(), 2);
    assert_eq!(
        third
            .schemas
            .iter()
            .map(|schema| schema.name.as_str())
            .collect::<Vec<_>>(),
        ["echo", "fetch"]
    );

    // Counts persist across prepare/infer; new tools seed at 0.
    let counts = ToolCallCounts::new(first.scope.bindings().iter().map(|b| b.alias().to_owned()));
    counts.increment("echo").expect("echo must be seeded");
    assert_eq!(counts.get("echo").unwrap(), Some(1));
    counts.ensure("fetch").expect("new tool seeds at 0");
    assert_eq!(counts.get("fetch").unwrap(), Some(0));
    assert_eq!(
        counts.get("echo").unwrap(),
        Some(1),
        "existing counts must persist when new tools are seeded"
    );

    vm.teardown(&NullObserver, "Bag");
}

#[test]
fn tool_description_override_appears_in_model_schema() {
    let bindings = crate::lua::ToolBindings::for_test(
        vec![crate::lua::ToolBinding::for_test(
            "echo",
            "echo capability for live matching",
            ToolId::new("tests", "echo").expect("valid id"),
        )],
        Vec::new(),
    );
    let echo = EchoTool;
    let registry = ToolRegistry::new([&echo as &dyn Tool]).expect("unique test registry");

    // tools.add(Tool) without mutating .description keeps the registry text.
    let mut default_vm = SectionVm::new_for_section(
        None,
        &bindings,
        &ModelBindings::default(),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("captured bindings must install");
    default_vm
        .inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_default = LuaProgram::compile(
        "tools.add(echo)",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("prologue must compile");
    default_vm
        .run_chunk(&add_default, &NullObserver, "Override")
        .expect("tools.add(echo) without override must succeed");
    let (default_bindings, default_runtime) = default_vm.tool_bag_handles();
    let mut default_bag = ToolBag::new(default_bindings, Arc::clone(&default_runtime));
    let default_prepared = default_bag
        .prepare(&registry)
        .expect("default prepare must build schemas");
    assert_eq!(default_prepared.schemas.len(), 1);
    assert_eq!(
        default_prepared.schemas[0].description,
        echo.description(),
        "unmutated Tool object must still advertise the registry description"
    );
    assert_eq!(
        default_prepared.scope.bindings()[0].description(),
        "echo capability for live matching",
        "live capability text must stay on the binding"
    );
    assert_eq!(
        default_prepared.scope.bindings()[0].model_description(),
        None
    );
    default_vm.teardown(&NullObserver, "Override");

    // Mutating .description before tools.add overrides the model-facing schema.
    let mut vm = SectionVm::new_for_section(
        None,
        &bindings,
        &ModelBindings::default(),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("captured bindings must install");
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .expect("host must inject");
    let add_override = LuaProgram::compile(
        "echo.description = 'Author override for the model'\n\
         assert(echo.description == 'Author override for the model')\n\
         tools.add(echo)",
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Override",
    )
    .expect("prologue must compile");
    vm.run_chunk(&add_override, &NullObserver, "Override")
        .expect("description override before tools.add must succeed");
    let (tool_bindings, tool_runtime) = vm.tool_bag_handles();
    let mut bag = ToolBag::new(tool_bindings, Arc::clone(&tool_runtime));
    let prepared = bag
        .prepare(&registry)
        .expect("override prepare must build schemas");
    assert_eq!(prepared.schemas.len(), 1);
    assert_eq!(
        prepared.schemas[0].description,
        "Author override for the model"
    );
    assert_eq!(
        prepared.scope.bindings()[0].description(),
        "echo capability for live matching",
        "override must not rewrite the live capability description"
    );
    assert_eq!(
        prepared.scope.bindings()[0].model_description(),
        Some("Author override for the model")
    );

    vm.teardown(&NullObserver, "Override");
}
