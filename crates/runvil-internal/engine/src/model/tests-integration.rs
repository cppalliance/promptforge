//! Lua-driven `models.use` integration tests over pre-filled role bindings.

use super::*;

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
    let vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
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
    let vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
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
    let vm =
        section_vm_with_models(&models, &null_emitter(), "Section").expect("the section VM builds");
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
