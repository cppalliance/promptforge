//! Lua-driven `models.bind`/`models.use`/`models.default` integration tests.

use super::*;

/// Compiles and resolves one live H1 declaration fixture.
fn resolve_shared(source: &str) -> Result<(ToolSet, ModelBindings)> {
    let shared = crate::lua::LuaProgram::compile(
        source,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )?;
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
}

#[test]
fn models_bind_resolves_and_use_selects_section_binding() {
    let (tools, models) = resolve_shared(
        r#"models.bind("analyst", "careful analysis", { thinking = false, temperature = 0, context = 40000 })"#,
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
    let (tools, models) = resolve_shared(r#"models.bind("analyst", "careful analysis")"#).unwrap();
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
fn constraint_filter_makes_bind_absent() {
    let error =
        resolve_shared(r#"models.bind("analyst", "careful analysis", { context = 200000 })"#)
            .unwrap_err();
    assert!(matches!(error, Error::ModelAbsent { .. }));
}

#[test]
fn undeclared_models_use_fails_loudly() {
    let (tools, models) = resolve_shared(r#"models.bind("analyst", "careful analysis")"#).unwrap();
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
    let error = vm
        .run_chunk(&prologue, &NullObserver, "Section")
        .expect_err("an undeclared model alias must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("models.use alias \"missing\" was not declared by models.bind"),
        "the error must name the undeclared alias and declaration requirement: {rendered}"
    );
    vm.teardown(&NullObserver, "Section");
}
