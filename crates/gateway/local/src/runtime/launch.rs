//! Per-model launch wiring: admission, serve mode, launch options, and companions.

use std::collections::HashMap;
use std::path::Path;

use gateway_config::{LocalModelConfig, ModelKind, QueuePolicy, ThinkingMode};
use gateway_routing::queue::DominionQueue;
use tokio_util::sync::CancellationToken;

use super::{CompanionPaths, LocalAdmission};
use crate::artifacts::ArtifactStore;
use crate::error::LocalError;
use crate::launch_templates::resolve_chat_template_file;
use crate::server::{LaunchOptions, ServeMode, SpeculativeLaunch};

/// Resolves a local model's admission wiring.
///
/// The `--parallel` value is `LocalModelConfig::parallel` (default 1). A
/// model without a `dominion` gets a per-model queue limited to that same
/// number, preserving the invariant that the child's `--parallel` and the
/// queue limit are one value. A model bound to a dominion instead admits
/// through that dominion's shared queue - one `Arc` shared with every other
/// bound local model - and the dominion's own limit governs admission.
pub(super) fn resolve_admission(
    dominion_queues: &HashMap<&str, DominionQueue>,
    model: &LocalModelConfig,
) -> Result<LocalAdmission, LocalError> {
    let parallel = model.parallel();
    let queue = match model.dominion() {
        Some(dominion_id) => dominion_queues.get(dominion_id).cloned().ok_or_else(|| {
            LocalError::UnknownDominion {
                model: model.name().to_owned(),
                dominion: dominion_id.to_owned(),
            }
        })?,
        // The unbound per-model queue uses the dominion defaults for depth
        // and fairness; the reject policy only exists on dominions.
        None => DominionQueue::new(parallel as usize, 100, true, QueuePolicy::Queue),
    };
    Ok(LocalAdmission { parallel, queue })
}

/// The `llama-server` serve mode for a model kind.
///
/// The mapping is side-effect-free, so a caller can preflight an unsupported
/// kind before any provisioning side effect: a speech model has no local
/// runtime yet, and a kind added to `ModelKind` after this mapping fails
/// loudly instead of launching as a chat server.
pub(super) fn serve_mode_for(kind: ModelKind) -> Result<ServeMode, LocalError> {
    match kind {
        ModelKind::Chat => Ok(ServeMode::Chat),
        ModelKind::Embedding => Ok(ServeMode::Embeddings),
        ModelKind::Classifier => Ok(ServeMode::Reranking),
        ModelKind::Speech => Err(LocalError::UnsupportedKind {
            kind: ModelKind::Speech,
        }),
        // `ModelKind` is `#[non_exhaustive]`: a kind added after this mapping
        // fails loudly instead of launching as a chat server.
        kind => Err(LocalError::UnsupportedKind { kind }),
    }
}

pub(super) fn launch_options(
    model: &LocalModelConfig,
    parallel: u32,
) -> Result<LaunchOptions, LocalError> {
    let serve_mode = serve_mode_for(model.kind())?;
    Ok(LaunchOptions {
        ctx_size: model.context(),
        n_predict: model.n_predict(),
        parallel,
        gpu_layers: model.gpu_layers(),
        flash_attention: model.flash_attention(),
        cache_type_k: model.cache_type_k().to_owned(),
        cache_type_v: model.cache_type_v().to_owned(),
        think: !matches!(model.thinking(), ThinkingMode::Never),
        chat_template_file: None,
        serve_mode,
        speculative: None,
        multimodal_projector: None,
        path_prefix: Vec::new(),
    })
}

pub(super) fn launch_options_for(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    model_path: &Path,
    admission: &LocalAdmission,
) -> Result<LaunchOptions, LocalError> {
    let mut options = launch_options(model, admission.parallel)?;
    options.chat_template_file = resolve_chat_template_file(store, model, model_path)?;
    Ok(options)
}

/// Resolves a model's declared companions through the same `ensure_model`
/// machinery as the main model and records the owned paths in `options`.
///
/// Each companion lands in its own cache slot keyed by its own source
/// identity, with its own pin verified on hit and after download. Any
/// resolution failure returns before the caller spawns the child, so a bad
/// companion never becomes a spawned-then-failing server. A model without
/// companions leaves `options` untouched, preserving the exact command line
/// from before companions existed.
///
/// # Errors
/// Returns [`LocalError`] when a companion source cannot be resolved or its
/// pin does not match.
pub(super) fn provision_companions(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    options: &mut LaunchOptions,
    token: Option<&CancellationToken>,
) -> Result<(), LocalError> {
    let companions = provision_companion_paths(store, model, token)?;
    options.speculative = companions.speculative;
    options.multimodal_projector = companions.multimodal_projector;
    Ok(())
}

/// The provisioning half of [`provision_companions`]: resolves each declared
/// companion through `ensure_model` and returns its path without touching
/// any launch options, so the artifact step can run it ahead of the spawn.
pub(super) fn provision_companion_paths(
    store: &ArtifactStore,
    model: &LocalModelConfig,
    token: Option<&CancellationToken>,
) -> Result<CompanionPaths, LocalError> {
    let mut companions = CompanionPaths::default();
    if let Some(speculative) = model.speculative() {
        let draft_model = store.ensure_model_with_cancellation(
            speculative.source(),
            speculative.sha256(),
            None,
            token,
        )?;
        tracing::info!(
            model = %model.name(),
            path = %draft_model.display(),
            "provisioned speculative drafter GGUF"
        );
        companions.speculative = Some(SpeculativeLaunch {
            draft_model,
            draft_max: speculative.draft_max().get(),
        });
    }
    if let Some(projector) = model.multimodal_projector() {
        let projector_path = store.ensure_model_with_cancellation(
            projector.source(),
            projector.sha256(),
            None,
            token,
        )?;
        tracing::info!(
            model = %model.name(),
            path = %projector_path.display(),
            "provisioned multimodal projector GGUF"
        );
        companions.multimodal_projector = Some(projector_path);
    }
    Ok(companions)
}
