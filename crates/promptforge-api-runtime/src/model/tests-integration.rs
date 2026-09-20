//! Lua-driven `models.use` integration tests over pre-filled role bindings.

use super::*;

/// Compiles one Lua chunk for a section VM drive.
fn chunk(source: &str) -> crate::lua::LuaProgram {
    crate::lua::LuaProgram::compile(
        source,
        "chunk",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        &null_emitter(),
        "Section",
    )
    .expect("test Lua must compile")
}

#[test]
fn models_use_selects_a_bound_role_by_label() {
    let models = shared_models(vec![bound_role(
        "analyst",
        "A careful analysis model",
        "analyst",
        131_072,
        Some(true),
        &["thinking", "frontier"],
    )]);
    let mut vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    vm.run_chunk(
        &chunk(r#"models.use("analyst")"#),
        &null_emitter(),
        "Section",
    )
    .expect("a bound label selects");
    let model = resolve_section_model(&vm).expect("the resolution reads the selection");
    let model = model.expect("a selection resolves");
    assert_eq!(model.alias(), "analyst");
    assert_eq!(model.invocation().thinking, Some(true));
    assert_eq!(
        model.capabilities(),
        &["thinking".to_owned(), "frontier".to_owned()]
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn no_use_or_default_leaves_the_section_unbound() {
    let models = shared_models(vec![bound_role(
        "analyst",
        "A careful analysis model",
        "analyst",
        131_072,
        None,
        &[],
    )]);
    let mut vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    let model = resolve_section_model(&vm).expect("the resolution reads the shared set");
    assert!(model.is_none());
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn models_use_rejects_an_unbound_label() {
    let models = shared_models(vec![bound_role(
        "analyst",
        "A careful analysis model",
        "analyst",
        131_072,
        None,
        &[],
    )]);
    let mut vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    let error = vm
        .run_chunk(
            &chunk(r#"models.use("missing")"#),
            &null_emitter(),
            "Section",
        )
        .expect_err("an unbound label must fail");
    let rendered = error.to_string();
    assert!(
        rendered.contains("models.use label \"missing\" is not a bound model role"),
        "the error must name the unbound label: {rendered}"
    );
    vm.teardown(&null_emitter(), "Section");
}

#[test]
fn models_bind_is_gone() {
    let models = shared_models(vec![]);
    let mut vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    let gone = vm
        .run_chunk(
            &chunk("return tostring(models.bind)"),
            &null_emitter(),
            "Section",
        )
        .expect("the probe runs");
    assert_eq!(
        gone,
        crate::lua::LuaBlockResult::Returned(Some("nil".to_owned())),
        "models.bind is removed"
    );
    vm.teardown(&null_emitter(), "Section");
}
