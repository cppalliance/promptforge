//! Lua-driven `models.need`/`models.use`/`models.default` integration tests.

use super::*;

#[test]
fn models_need_resolves_and_use_selects_section_binding() {
    let shared = crate::lua::LuaProgram::compile(
            r#"models.need("analyst", "careful analysis", { thinking = false, temperature = 0, context = 40000 })"#,
            "shared",
            NonZeroU32::new(1).expect("compile source line is non-zero"),
            EXECUTION,
            &NullObserver,
            "Prompt",
        )
        .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    assert_eq!(models.bindings()[0].id().name(), "analyst");
    assert_eq!(models.bindings()[0].invocation().thinking, Some(false));

    let mut vm =
        section_vm_with_model_bindings(&tools, &models, EXECUTION, &NullObserver, "Section")
            .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let prologue = crate::lua::LuaProgram::compile(
        r#"models.use("analyst")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.run_chunk(&prologue, &NullObserver, "Section").unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    assert_eq!(model.unwrap().alias(), "analyst");
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn no_models_use_or_always_leaves_section_unbound() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("analyst", "careful analysis")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let mut vm =
        section_vm_with_model_bindings(&tools, &models, EXECUTION, &NullObserver, "Section")
            .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    assert!(model.is_none());
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn constraint_filter_makes_need_absent() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("analyst", "careful analysis", { context = 200000 })"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let error = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap_err();
    assert!(matches!(error, Error::ModelAbsent { .. }));
}

#[test]
fn undeclared_models_use_fails_loudly() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("analyst", "careful analysis")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let mut vm =
        section_vm_with_model_bindings(&tools, &models, EXECUTION, &NullObserver, "Section")
            .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let prologue = crate::lua::LuaProgram::compile(
        r#"models.use("missing")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    assert!(vm.run_chunk(&prologue, &NullObserver, "Section").is_err());
    vm.teardown(&NullObserver, "Section");
}
