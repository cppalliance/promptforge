//! `models.default` (single- and multi-arg) integration tests.

use super::*;

#[test]
fn models_always_records_binding() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.default("writer")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (_tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    assert_eq!(models.default(), Some("writer"));
}

#[test]
fn models_always_returns_inspectable_object() {
    let shared = crate::lua::LuaProgram::compile(
        r#"local needed = models.need("writer", "A tiny model", {
                   thinking = false, temperature = 0, max_tokens = 256
               })
               assert(needed.name == "writer")
               assert(needed.model_id == "small")
               assert(needed.description == "A tiny model")
               assert(needed.context == 8192)
               assert(needed.thinking == false)
               assert(needed.temperature == 0)
               assert(needed.max_tokens == 256)
               assert(needed.dialect == "openai")
               local model = models.default("writer")
               assert(model.name == "writer")
               assert(model.model_id == "small")
               assert(model.description == "A tiny model")
               assert(model.context == 8192)
               assert(model.thinking == false)
               assert(model.temperature == 0)
               assert(model.max_tokens == 256)
               assert(model.dialect == "openai")"#,
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
    .expect("models.need/always must return an inspectable Model object");
    assert_eq!(models.default(), Some("writer"));
    assert_eq!(models.bindings()[0].context().get(), 8_192);

    let vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .expect("section install must expose the same inspectable Model object");
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_without_prior_need_fails() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.default("writer")"#,
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
    let msg = error.to_string();
    assert!(msg.contains("not declared"), "unexpected error: {msg}");
}

#[test]
fn models_always_duplicate_fails() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model")
               models.default("writer")
               models.default("writer")"#,
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
    let msg = error.to_string();
    assert!(msg.contains("at most once"), "unexpected error: {msg}");
}

#[test]
fn models_always_installs_exactly() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model")
               models.default("writer")"#,
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
    let mut vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    assert_eq!(model.as_ref().map(ModelBinding::alias), Some("writer"));
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_provides_completion_options_without_use() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.default("writer")"#,
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
    let mut vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    let opts = model.as_ref().map(ModelBinding::completion_options);
    let expected = CompletionOptions {
        model: "small".to_owned(),
        temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
        max_tokens: None,
        thinking: Some(false),
        tool_dialect: ToolDialectId::OpenAi,
    };
    assert_eq!(opts, Some(expected));
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_from_h2_prologue_fails() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model")"#,
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
    let mut vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let prologue = crate::lua::LuaProgram::compile(
        r#"models.default("writer")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    let result = vm.run_chunk(&prologue, &NullObserver, "Section");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("only available during live H1 execution"),
        "unexpected error: {msg}"
    );
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_multi_arg_records_need_and_always() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.default("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (_tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    assert_eq!(models.default(), Some("writer"));
    assert!(models.binding("writer").is_some());
}

#[test]
fn models_always_multi_arg_two_args() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.default("writer", "A tiny model")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (_tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    assert_eq!(models.default(), Some("writer"));
    assert!(models.binding("writer").is_some());
}

#[test]
fn models_always_multi_arg_provides_completion_options() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.default("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
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
    let mut vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    let opts = model.as_ref().map(ModelBinding::completion_options);
    let expected = CompletionOptions {
        model: "small".to_owned(),
        temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
        max_tokens: None,
        thinking: Some(false),
        tool_dialect: ToolDialectId::OpenAi,
    };
    assert_eq!(opts, Some(expected));
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_multi_arg_installs_exactly() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.default("writer", "A tiny model", { thinking = false })"#,
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
    let mut vm = section_vm_with_model_bindings(
        &shared,
        &tools,
        &models,
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.inject_host("", &json!({}), &StoreRef::memory(), None)
        .unwrap();
    let (mb, mr) = vm.model_bag_handles();
    let model = resolve_model_binding(&mb, &mr).unwrap();
    assert_eq!(model.as_ref().map(ModelBinding::alias), Some("writer"));
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_multi_arg_and_single_arg_cannot_both_be_called() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("analyst", "careful analysis")
               models.default("writer", "A tiny model")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let tool_resolver =
        |_: &str| -> crate::Result<crate::tools::ToolId> { unreachable!("no tools") };
    let (_tools, models) = resolve_live_declarations_for_test(
        &shared,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    assert_eq!(models.default(), Some("writer"));

    // Now verify that a second always (single-arg) after multi-arg always fails.
    let shared2 = crate::lua::LuaProgram::compile(
        r#"models.default("writer", "A tiny model")
               models.default("writer")"#,
        "shared",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap();
    let error = resolve_live_declarations_for_test(
        &shared2,
        &tool_resolver,
        &fixture_resolver,
        EXECUTION,
        &NullObserver,
        "Prompt",
    )
    .unwrap_err();
    let msg = error.to_string();
    assert!(msg.contains("at most once"), "unexpected error: {msg}");
}

#[test]
fn models_always_multi_arg_duplicate_alias_fails() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model")
               models.default("writer", "A tiny model")"#,
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
    let msg = error.to_string();
    assert!(
        msg.contains("duplicate")
            || msg.contains("Duplicate")
            || msg.contains("declared more than once"),
        "unexpected error: {msg}"
    );
}
