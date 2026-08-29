use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use mlua::Lua;

use super::*;
use crate::lua::{
    LiveBindingProducer, LuaProgram, SectionVm, ToolResolver, ToolSet, resolve_model_binding,
};
use crate::observe::NullObserver;
use crate::store::StoreRef;
use crate::tools::ToolCatalog;
use crate::untrusted::GuardNonce;
use crate::{Error, Result};
use promptforge_gateway_client::Error as GatewayClientError;
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

fn fixture_resolver(
    description: &str,
    opts: &ModelBindOpts,
) -> std::result::Result<ResolvedModel, GatewayClientError> {
    let catalog = catalog();
    let matches = catalog.filtered(opts);
    let hit = matches
        .iter()
        .find(|model| {
            (description.contains("analysis") && model.id().name() == "analyst")
                || (description.contains("tiny") && model.id().name() == "small")
        })
        .ok_or_else(|| GatewayClientError::ModelAbsent {
            capability: description.to_owned(),
        })?;
    Ok(ResolvedModel {
        id: hit.id().clone(),
        invocation: ModelInvocation::from(opts),
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
) -> Result<(ToolSet, ModelSet)> {
    let catalog = ToolCatalog::default();
    let producer = LiveBindingProducer::new(
        Arc::new(Mutex::new(ToolSet::default())),
        Arc::new(Mutex::new(ModelSet::default())),
    );
    let lua = Lua::new();
    let result = lua.scope(|scope| {
        producer
            .install(&lua, scope, tool_resolver, &catalog, model_resolver)
            .map_err(|error| mlua::Error::external(error.to_string()))?;
        lua.load(source.source()).exec()
    });
    if let Some(error) = producer.take_callback_error()? {
        return Err(Error::from(error));
    }
    result.map_err(Error::lua)?;
    producer.bindings().map_err(Error::from)
}

fn section_vm_with_model_bindings(
    tools: &ToolSet,
    models: &ModelSet,
    execution: &str,
    observer: &dyn crate::observe::Observer,
    section: &str,
) -> Result<SectionVm> {
    let vm = SectionVm::new_for_section(
        &GuardNonce::fresh(),
        tools,
        models,
        execution,
        observer,
        section,
    )?;
    vm.install_captured_bindings()?;
    Ok(vm)
}

/// Reads the section's effective model binding through a view over the VM's
/// frozen set, mirroring the engine's read path.
fn resolve_section_model(vm: &SectionVm) -> Result<Option<ModelBinding>> {
    let (models, runtime) = vm.model_bag_handles();
    resolve_model_binding(&Mutex::new(models), &runtime).map_err(Error::from)
}

mod always;
mod integration;
