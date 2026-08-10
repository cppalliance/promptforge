
use mlua::Lua;

use super::*;
use crate::Error;
use crate::lua::{LiveBindingProducer, LuaProgram, SectionVm, ToolBindings, ToolResolver};
use crate::observe::NullObserver;
use crate::store::StoreRef;
use crate::tools::ToolRegistry;
use serde_json::json;

const EXECUTION: &str = "model-bind-test";

fn ctx(window: u32) -> NonZeroU32 {
    NonZeroU32::new(window).expect("test context window is non-zero")
}

fn gateway_id(name: &str) -> ModelId {
    ModelId::gateway(name).expect("test model alias is valid")
}

fn catalog() -> ModelCatalog {
    ModelCatalog::new([
        ModelDescriptor::new(
            gateway_id("small"),
            "A tiny model",
            ctx(8_192),
            ThinkingMode::Never,
        ),
        ModelDescriptor::new(
            gateway_id("analyst"),
            "A careful analysis model",
            ctx(131_072),
            ThinkingMode::Switchable,
        ),
        ModelDescriptor::new(
            gateway_id("always-think"),
            "Always thinks aloud",
            ctx(64_000),
            ThinkingMode::Always,
        ),
    ])
    .expect("test catalog has unique model ids")
}

fn fixture_resolver(description: &str, opts: &ModelNeedOpts) -> Result<ResolvedModel> {
    let catalog = catalog();
    let matches = catalog.filtered(opts);
    let hit = matches
        .iter()
        .find(|model| {
            (description.contains("analysis") && model.id().name() == "analyst")
                || (description.contains("tiny") && model.id().name() == "small")
        })
        .ok_or_else(|| Error::ModelAbsent {
            capability: description.to_owned(),
        })?;
    Ok(ResolvedModel {
        id: hit.id().clone(),
        invocation: ModelInvocation::from(opts),
        tool_dialect: hit.tool_dialect(),
        context: hit.context(),
    })
}

fn resolve_live_declarations_for_test(
    source: &LuaProgram,
    tool_resolver: &dyn ToolResolver,
    model_resolver: &dyn ModelResolver,
    _execution: &str,
    _observer: &dyn crate::observe::Observer,
    _section: &str,
) -> Result<(ToolBindings, ModelBindings)> {
    let registry = ToolRegistry::new(std::iter::empty()).expect("unique test registry");
    let producer = LiveBindingProducer::default();
    let lua = Lua::new();
    let result = lua.scope(|scope| {
        producer
            .install(&lua, scope, tool_resolver, &registry, model_resolver)
            .map_err(|error| mlua::Error::external(error.to_string()))?;
        lua.load(source.source()).exec()
    });
    if let Some(error) = producer.take_callback_error()? {
        return Err(error);
    }
    result.map_err(|error| Error::Lua(error.to_string()))?;
    producer.bindings()
}

fn section_vm_with_model_bindings(
    _source: &LuaProgram,
    tools: &ToolBindings,
    models: &ModelBindings,
    execution: &str,
    observer: &dyn crate::observe::Observer,
    section: &str,
) -> Result<SectionVm> {
    SectionVm::new_for_section(None, tools, models, execution, observer, section)
}

#[test]
fn context_filter_drops_small_windows() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelNeedOpts {
        context: Some(ctx(40_000)),
        ..ModelNeedOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["analyst", "always-think"]);
}

#[test]
fn thinking_false_keeps_never_and_switchable() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelNeedOpts {
        thinking: Some(false),
        ..ModelNeedOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["small", "analyst"]);
}

#[test]
fn thinking_true_keeps_switchable_and_always() {
    let catalog = catalog();
    let matches = catalog.filtered(&ModelNeedOpts {
        thinking: Some(true),
        ..ModelNeedOpts::default()
    });
    let names: Vec<_> = matches.iter().map(|m| m.id().name()).collect();
    assert_eq!(names, ["analyst", "always-think"]);
}

#[test]
fn same_weights_different_invocation_compare_unequal() {
    let id = gateway_id("analyst");
    let a = ModelBinding::new(
        "cool",
        "careful analysis",
        id.clone(),
        ModelInvocation {
            temperature: Some(Temperature::new(0.0).expect("0.0 is valid")),
            max_tokens: None,
            thinking: Some(false),
        },
        ToolDialectId::OpenAi,
        ctx(131_072),
    );
    let b = ModelBinding::new(
        "warm",
        "careful analysis",
        id,
        ModelInvocation {
            temperature: Some(Temperature::new(0.7).expect("0.7 is valid")),
            max_tokens: None,
            thinking: Some(false),
        },
        ToolDialectId::OpenAi,
        ctx(131_072),
    );
    assert_eq!(a.id(), b.id());
    assert_ne!(a.invocation(), b.invocation());
}

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
        r#"models.use("analyst")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.run_prologue(&prologue, &NullObserver, "Section")
        .unwrap();
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    assert_eq!(scopes.model.unwrap().alias(), "analyst");
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
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    assert!(scopes.model.is_none());
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
        r#"models.use("missing")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    assert!(
        vm.run_prologue(&prologue, &NullObserver, "Section")
            .is_err()
    );
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_records_binding() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.always("writer")"#,
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
    assert_eq!(models.always(), Some("writer"));
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
               local model = models.always("writer")
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
    assert_eq!(models.always(), Some("writer"));
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
        r#"models.always("writer")"#,
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
               models.always("writer")
               models.always("writer")"#,
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
               models.always("writer")"#,
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
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    assert_eq!(
        scopes.model.as_ref().map(ModelBinding::alias),
        Some("writer")
    );
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_provides_completion_options_without_use() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model", { thinking = false, temperature = 0 })
               models.always("writer")"#,
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
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    let opts = scopes.model.as_ref().map(ModelBinding::completion_options);
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
fn models_use_overrides_always() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("writer", "A tiny model", { thinking = false })
               models.need("analyst", "careful analysis", { thinking = true })
               models.always("writer")"#,
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
        r#"models.use("analyst")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    vm.run_prologue(&prologue, &NullObserver, "Section")
        .unwrap();
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    assert_eq!(
        scopes.model.as_ref().map(ModelBinding::alias),
        Some("analyst")
    );
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
        r#"models.always("writer")"#,
        "prologue",
        NonZeroU32::new(1).expect("compile source line is non-zero"),
        EXECUTION,
        &NullObserver,
        "Section",
    )
    .unwrap();
    let result = vm.run_prologue(&prologue, &NullObserver, "Section");
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
        r#"models.always("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
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
    assert_eq!(models.always(), Some("writer"));
    assert!(models.binding("writer").is_some());
}

#[test]
fn models_always_multi_arg_two_args() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.always("writer", "A tiny model")"#,
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
    assert_eq!(models.always(), Some("writer"));
    assert!(models.binding("writer").is_some());
}

#[test]
fn models_always_multi_arg_provides_completion_options() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.always("writer", "A tiny model", { thinking = false, temperature = 0 })"#,
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
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    let opts = scopes.model.as_ref().map(ModelBinding::completion_options);
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
        r#"models.always("writer", "A tiny model", { thinking = false })"#,
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
    let scopes = vm.close_scopes(&NullObserver, "Section").unwrap();
    assert_eq!(
        scopes.model.as_ref().map(ModelBinding::alias),
        Some("writer")
    );
    vm.teardown(&NullObserver, "Section");
}

#[test]
fn models_always_multi_arg_and_single_arg_cannot_both_be_called() {
    let shared = crate::lua::LuaProgram::compile(
        r#"models.need("analyst", "careful analysis")
               models.always("writer", "A tiny model")"#,
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
    assert_eq!(models.always(), Some("writer"));

    // Now verify that a second always (single-arg) after multi-arg always fails.
    let shared2 = crate::lua::LuaProgram::compile(
        r#"models.always("writer", "A tiny model")
               models.always("writer")"#,
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
               models.always("writer", "A tiny model")"#,
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

#[test]
fn descriptor_with_dialect_sets_tools_mode() {
    let descriptor = ModelDescriptor::new(
        gateway_id("gemma-local"),
        "A Gemma model",
        ctx(32_768),
        ThinkingMode::Never,
    )
    .with_dialect(ToolDialectId::Gemma3ToolCode);
    assert_eq!(descriptor.tool_dialect(), ToolDialectId::Gemma3ToolCode);
    assert_eq!(
        descriptor.tools_mode(),
        crate::dialects::ToolsMode::Emulated
    );
}

#[test]
fn descriptor_default_dialect_is_openai_native() {
    let descriptor = ModelDescriptor::new(
        gateway_id("remote"),
        "A remote model",
        ctx(8_192),
        ThinkingMode::Never,
    );
    assert_eq!(descriptor.tool_dialect(), ToolDialectId::OpenAi);
    assert_eq!(descriptor.tools_mode(), crate::dialects::ToolsMode::Native);
}

#[test]
fn model_invocation_equality_is_not_reflexive_for_nan() {
    // Documents why these float-bearing types intentionally do not implement
    // `Eq`: a NaN temperature is not equal to itself.
    let nan = ModelInvocation {
        temperature: Some(Temperature(f64::NAN)),
        max_tokens: None,
        thinking: None,
    };
    assert_ne!(nan, nan.clone());
}

#[test]
fn completion_options_equality_is_not_reflexive_for_nan() {
    // `CompletionOptions` carries an `Option<Temperature>` (an `f64`
    // newtype) temperature, so it must not implement `Eq`: a NaN temperature
    // is not equal to itself, which would violate the reflexivity `Eq`
    // promises. This test would fail to compile if `Eq` were (re)added. A NaN
    // cannot enter through the public validated setter; the field is set with
    // an in-crate `Temperature(NaN)` (private tuple) to prove the soundness
    // reason `Eq` is withheld.
    let options = CompletionOptions {
        model: "m".to_owned(),
        temperature: Some(Temperature(f64::NAN)),
        max_tokens: None,
        thinking: None,
        tool_dialect: ToolDialectId::OpenAi,
    };
    assert_ne!(options, options.clone());
}

#[test]
fn with_temperature_rejects_non_finite_and_out_of_range() {
    let base = || CompletionOptions::new("m", ToolDialectId::OpenAi);
    assert_eq!(
        base().with_temperature(f64::NAN),
        Err(TemperatureError::NotFinite)
    );
    assert_eq!(
        base().with_temperature(f64::INFINITY),
        Err(TemperatureError::NotFinite)
    );
    assert!(matches!(
        base().with_temperature(-0.1),
        Err(TemperatureError::OutOfRange { .. })
    ));
    assert!(matches!(
        base().with_temperature(2.5),
        Err(TemperatureError::OutOfRange { .. })
    ));
    // The range endpoints and an interior value are accepted.
    assert_eq!(
        base()
            .with_temperature(0.0)
            .expect("0.0 is valid")
            .temperature
            .map(Temperature::get),
        Some(0.0)
    );
    assert_eq!(
        base()
            .with_temperature(TEMPERATURE_MAX)
            .expect("2.0 is valid")
            .temperature
            .map(Temperature::get),
        Some(TEMPERATURE_MAX)
    );
    assert_eq!(
        base()
            .with_temperature(0.7)
            .expect("0.7 is valid")
            .temperature
            .map(Temperature::get),
        Some(0.7)
    );
}

#[test]
fn model_id_rejects_empty_and_control_characters() {
    assert!(ModelId::gateway("").is_err());
    assert!(ModelId::new("", "name").is_err());
    assert!(ModelId::new("server", "").is_err());
    assert!(ModelId::new("server", "na\nme").is_err());
    assert!(ModelId::gateway("valid-alias").is_ok());
}

#[test]
fn model_catalog_rejects_duplicate_ids() {
    let descriptor =
        |name: &str| ModelDescriptor::new(gateway_id(name), "d", ctx(8_192), ThinkingMode::Never);
    let err = ModelCatalog::new([descriptor("dup"), descriptor("dup")])
        .expect_err("a catalog with duplicate ids must be rejected");
    assert!(matches!(err, ModelCatalogError::DuplicateId { .. }));
    assert!(ModelCatalog::new([descriptor("a"), descriptor("b")]).is_ok());
}

#[test]
fn binding_dialect_propagates_to_completion_options() {
    let binding = ModelBinding::new(
        "gemma",
        "a local gemma model",
        gateway_id("gemma-local"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        ToolDialectId::Gemma3ToolCode,
        ctx(8_192),
    );
    let opts = binding.completion_options();
    assert_eq!(opts.tool_dialect, ToolDialectId::Gemma3ToolCode);
}

#[test]
fn binding_construction_is_atomic_with_dialect_and_context() {
    let binding = ModelBinding::new(
        "remote",
        "a remote model",
        gateway_id("remote"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        ToolDialectId::OpenAi,
        ctx(64_000),
    );
    let opts = binding.completion_options();
    assert_eq!(opts.tool_dialect, ToolDialectId::OpenAi);
    assert_eq!(binding.context().get(), 64_000);
}

#[tokio::test]
async fn fetch_model_catalog_rejects_a_wire_tools_mode_that_contradicts_the_dialect() {
    use axum::Router;
    use axum::routing::get;

    // MODEL-008: a wire `tools_mode` is validated against the mode derived
    // from `tool_dialect`. An OpenAI (native) dialect paired with an
    // `emulated` wire mode is contradictory and must be refused as malformed
    // rather than silently keeping one of the two.
    async fn models() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "data": [{
                "id": "remote",
                "description": "a remote model",
                "context": 8192,
                "thinking": "never",
                "tool_dialect": "openai",
                "tools_mode": "emulated"
            }]
        }))
    }
    let app = Router::new().route("/models", get(models));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a contradictory wire tools_mode must be rejected");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        err.to_string().contains("contradicts"),
        "the rejection must name the contradiction, got {err}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
    use axum::Router;
    use axum::routing::get;

    async fn models() -> (axum::http::StatusCode, String) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "e".repeat(transport::MAX_CATALOG_ERROR_BODY * 4),
        )
    }
    let app = Router::new().route("/models", get(models));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a 500 response must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::Backend);
    let msg = err.to_string();
    assert!(
        msg.len() < transport::MAX_CATALOG_ERROR_BODY + 128,
        "the error-path body must be bounded, got {} bytes",
        msg.len()
    );
}

#[tokio::test]
async fn fetch_model_catalog_bounds_an_oversized_success_body() {
    use axum::Router;
    use axum::routing::get;

    // A 200 response whose body exceeds the success cap must be refused
    // BEFORE decoding, not buffered unbounded. The body is deliberately not
    // valid JSON: the bound must trip first, regardless of contents.
    async fn models() -> (axum::http::StatusCode, String) {
        let oversized = usize::try_from(transport::MAX_CATALOG_BODY).unwrap() + 1;
        (axum::http::StatusCode::OK, "e".repeat(oversized))
    }
    let app = Router::new().route("/models", get(models));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("an oversized success body must be refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        err.to_string().contains("exceeds"),
        "the bound must report the size limit, got {err}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_preserves_the_json_decode_source() {
    use axum::Router;
    use axum::routing::get;

    // MODEL-009: a 200 body that is not a valid model list is classified as
    // MalformedResponse, and the underlying `serde_json::Error` survives as
    // the error-chain `#[source]` rather than being flattened into the text.
    async fn models() -> (axum::http::StatusCode, String) {
        (axum::http::StatusCode::OK, "{ this is not json".to_owned())
    }
    let app = Router::new().route("/models", get(models));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("an undecodable body must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    let source =
        std::error::Error::source(&err).expect("the decode error must be a preserved source");
    assert!(
        source.downcast_ref::<serde_json::Error>().is_some(),
        "the preserved source must be the JSON decode error, got {source}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_preserves_a_body_read_failure_source() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // MODEL-010: a non-success response whose body cannot be fully read
    // (the server promises a large body then drops the connection) must
    // surface as a typed transport failure that keeps the `reqwest::Error`
    // as its `#[source]`, not display text.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let header = "HTTP/1.1 500 Internal Server Error\r\n\
                     Content-Length: 1000000\r\n\r\n";
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(b"abc").await;
            // Socket drops here: the promised body never completes.
        }
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a truncated error body must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    assert_eq!(err.status(), Some(500));
    let source =
        std::error::Error::source(&err).expect("the read failure must be a preserved source");
    assert!(
        source.downcast_ref::<reqwest::Error>().is_some(),
        "the preserved source must be the reqwest read error, got {source}"
    );
}
