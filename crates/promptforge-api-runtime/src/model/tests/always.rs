//! `models.default` (label form) integration tests.

use super::*;

/// Compiles one Lua chunk for a section VM drive.
fn chunk(source: &str) -> crate::lua::LuaProgram {
    crate::lua::LuaProgram::compile(
        source,
        "chunk",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver::default(),
        "Section",
    )
    .expect("test Lua must compile")
}

#[test]
fn models_default_takes_a_label_and_parks_the_prompt_wide_default() {
    let models = shared_models(vec![bound_role(
        "writer",
        "A tiny model",
        "small",
        8_192,
        Some(false),
        &["no-thinking"],
    )]);
    let mut vm = section_vm_with_models(&models, &NullObserver::default(), "Section")
        .expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    vm.run_chunk(
        &chunk(r#"models.default("writer")"#),
        &NullObserver::default(),
        "Section",
    )
    .expect("a bound label becomes the default");
    assert_eq!(
        models.lock().expect("set lock").default.as_deref(),
        Some("writer")
    );
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn models_default_returns_an_inspectable_handle() {
    let models = shared_models(vec![bound_role(
        "writer",
        "A tiny model",
        "small",
        8_192,
        Some(false),
        &["no-thinking", "fast"],
    )]);
    let mut vm = section_vm_with_models(&models, &NullObserver::default(), "Section")
        .expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    vm.run_chunk(
        &chunk(
            r#"local model = models.default("writer")
               assert(model.label == "writer")
               assert(model.name == "writer")
               assert(model.model_id == "small")
               assert(model.description == "A tiny model")
               assert(model.context == 8192)
               assert(model.thinking == false)
               assert(#model.capabilities == 2)
               assert(model.capabilities[1] == "no-thinking")
               assert(model.capabilities[2] == "fast")"#,
        ),
        &NullObserver::default(),
        "Section",
    )
    .expect("the handle exposes the role label and the full keyword set");
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn models_default_rejects_an_unbound_label() {
    let models = shared_models(vec![bound_role(
        "writer",
        "A tiny model",
        "small",
        8_192,
        None,
        &[],
    )]);
    let mut vm = section_vm_with_models(&models, &NullObserver::default(), "Section")
        .expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    let error = vm
        .run_chunk(
            &chunk(r#"models.default("ghost")"#),
            &NullObserver::default(),
            "Section",
        )
        .expect_err("an unbound label is a hard error");
    assert!(
        error
            .to_string()
            .contains("models.default label \"ghost\" is not a bound model role"),
        "the rejection names the label: {error}"
    );
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn models_default_is_idempotent_and_never_changes_mid_run() {
    let models = shared_models(vec![
        bound_role("writer", "A tiny model", "small", 8_192, None, &[]),
        bound_role(
            "critic",
            "A careful analysis model",
            "analyst",
            131_072,
            None,
            &[],
        ),
    ]);
    let mut vm = section_vm_with_models(&models, &NullObserver::default(), "Section")
        .expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    // The shared library replays into every section, so re-naming the same
    // default is a no-op.
    vm.run_chunk(
        &chunk(r#"models.default("writer"); models.default("writer")"#),
        &NullObserver::default(),
        "Section",
    )
    .expect("re-naming the same default is a no-op");
    let error = vm
        .run_chunk(
            &chunk(r#"models.default("critic")"#),
            &NullObserver::default(),
            "Section",
        )
        .expect_err("the prompt-wide default cannot change mid-run");
    assert!(
        error
            .to_string()
            .contains("models.default is already \"writer\""),
        "the refusal names the parked default: {error}"
    );
    vm.teardown(&NullObserver::default(), "Section");
}

#[test]
fn models_default_resolves_the_section_model_without_use() {
    let models = shared_models(vec![bound_role(
        "writer",
        "A tiny model",
        "small",
        8_192,
        Some(false),
        &["no-thinking"],
    )]);
    let mut vm = section_vm_with_models(&models, &NullObserver::default(), "Section")
        .expect("the section VM builds");
    vm.inject_host("", &json!({}), &fresh_access()).unwrap();
    vm.run_chunk(
        &chunk(r#"models.default("writer")"#),
        &NullObserver::default(),
        "Section",
    )
    .expect("the default parks");
    let model = resolve_section_model(&vm).expect("the resolution reads the shared set");
    assert_eq!(model.as_ref().map(ModelBinding::alias), Some("writer"));
    let opts = model.as_ref().map(ModelBinding::completion_options);
    let expected = CompletionOptions::new("small").with_thinking(false);
    assert_eq!(opts, Some(expected));
    vm.teardown(&NullObserver::default(), "Section");
}
