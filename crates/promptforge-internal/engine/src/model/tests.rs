//! Tests for resolving a section's model binding through the VM and shared model set.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use super::*;
use crate::lua::{SectionVm, ToolSet, resolve_model_binding};
use crate::test_support::fresh_access;
use crate::test_support::recording::null_emitter;
use crate::test_support::scenario::TEST_SEED;
use crate::untrusted::GuardNonce;
use crate::{Error, Result};
use promptforge_model_client::model::{CompletionOptions, ModelInvocation};
use promptforge_types::emitter::Emitter;
use serde_json::json;

/// Compiles one Lua chunk for a section VM drive.
#[track_caller]
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

fn ctx(window: u32) -> NonZeroU32 {
    NonZeroU32::new(window).expect("test context window is non-zero")
}

fn gateway_id(name: &str) -> ModelId {
    ModelId::gateway(name).expect("test model alias is valid")
}

/// A bound role as prepare's fill records it: label, description, resolved
/// identity, the hard-keyword thinking switch as the frozen invocation, and
/// the role's keyword set.
fn bound_role(
    label: &str,
    description: &str,
    model: &str,
    window: u32,
    thinking: Option<bool>,
    capabilities: &[&str],
) -> ModelBinding {
    ModelBinding::new(
        label,
        description,
        gateway_id(model),
        ModelInvocation::new().with_thinking(thinking),
        ctx(window),
    )
    .with_capabilities(capabilities.iter().map(|word| (*word).to_owned()).collect())
}

/// Shares a model set the way the run shares its own.
fn shared_models(bindings: Vec<ModelBinding>) -> Arc<Mutex<ModelSet>> {
    Arc::new(Mutex::new(ModelSet::from_parts(bindings, None)))
}

/// Shares an empty tool set.
fn shared_tools() -> Arc<ToolSet> {
    Arc::new(ToolSet::default())
}

fn section_vm_with_models(
    models: &Arc<Mutex<ModelSet>>,
    emitter: &Emitter,
    section: &str,
) -> Result<SectionVm> {
    Ok(SectionVm::new_for_section(
        &GuardNonce::from_seed(&TEST_SEED),
        &shared_tools(),
        models,
        emitter,
        section,
    )?)
}

/// Reads the section's effective model binding through a view over the VM's
/// shared set, mirroring the Engine's read path.
fn resolve_section_model(vm: &SectionVm) -> Result<Option<ModelBinding>> {
    let (models, runtime) = vm.model_bag_handles()?;
    resolve_model_binding(&Mutex::new(models), &runtime).map_err(Error::from)
}

#[path = "tests-always.rs"]
mod always;
#[path = "tests-integration.rs"]
mod integration;
