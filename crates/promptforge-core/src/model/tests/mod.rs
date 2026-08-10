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

mod always;
mod integration;

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
