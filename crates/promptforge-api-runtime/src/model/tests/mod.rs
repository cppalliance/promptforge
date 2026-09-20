use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use super::*;
use crate::lua::{SectionVm, ToolSet, resolve_model_binding};
use crate::store::Access;
use crate::test_support::recording::null_emitter;
use crate::untrusted::GuardNonce;
use crate::{Error, Result};
use promptforge_api_types::emitter::Emitter;
use promptforge_model_client::model::{CompletionOptions, ModelInvocation};
use serde_json::json;

/// A fresh stock handle's access capability, for tests that inject host
/// values into a standalone VM.
fn fresh_access() -> Arc<Access> {
    Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("model test fixture"))
            .expect("the stock backend acquires"),
    )
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
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking,
        },
        ctx(window),
    )
    .with_capabilities(capabilities.iter().map(|word| (*word).to_owned()).collect())
}

/// Shares a model set the way the run shares its own.
fn shared_models(bindings: Vec<ModelBinding>) -> Arc<Mutex<ModelSet>> {
    Arc::new(Mutex::new(ModelSet::from_parts(bindings, None)))
}

/// Shares an empty tool set (these fixtures declare no tool slots).
fn shared_tools() -> Arc<Mutex<ToolSet>> {
    Arc::new(Mutex::new(ToolSet::default()))
}

fn section_vm_with_models(
    models: &Arc<Mutex<ModelSet>>,
    emitter: &Emitter,
    section: &str,
) -> Result<SectionVm> {
    let vm = SectionVm::new_for_section(
        &GuardNonce::from_seed(0x7e57),
        &shared_tools(),
        models,
        emitter,
        section,
    )?;
    vm.install_captured_bindings()?;
    Ok(vm)
}

/// Reads the section's effective model binding through a view over the VM's
/// shared set, mirroring the engine's read path.
fn resolve_section_model(vm: &SectionVm) -> Result<Option<ModelBinding>> {
    let (models, runtime) = vm.model_bag_handles()?;
    resolve_model_binding(&Mutex::new(models), &runtime).map_err(Error::from)
}

mod always;
mod integration;
