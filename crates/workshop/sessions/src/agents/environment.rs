//! The session run's capabilities and current model: the shared registry of
//! first-party capabilities every session run hands its `RunHost` (the
//! engine's loop activates against it per run), and the launch-time
//! resolution of the dropdown's current model into the per-run context.

use std::sync::Arc;

use promptforge_api_runtime::client::fetch_model_catalog;
use promptforge_api_runtime::{CapabilityRegistry, CompletionError, Web};
use promptforge_api_types::models::{ModelDescriptor, ModelId};

use super::SessionHost;

/// Builds the sessions' shared capability registry for one gateway
/// generation: the first-party capabilities built from the gateway's API
/// root and bearer - today `promptforge/web`. One registry is shared across
/// the runs of one gateway generation and rebuilt when the generation
/// changes, so a replacement gateway's root and key reach the contributed
/// tools. Model-free: the gateway's model list feeds the dropdown UI and
/// never crosses this interface.
///
/// Returns `None` - reported like an unusable model client - when the
/// gateway root or key cannot build the capability.
#[must_use]
pub fn session_registry(base_url: &str, api_key: &str) -> Option<CapabilityRegistry> {
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let web = match Web::new(&root, api_key) {
        Ok(web) => web,
        Err(error) => {
            tracing::warn!(%error, "agent sessions degraded: the gateway cannot build promptforge/web");
            return None;
        }
    };
    let mut registry = CapabilityRegistry::new();
    if registry.register(Arc::new(web)).is_err() {
        // A single registration cannot collide; the registry's error is
        // defensive on this path.
        return None;
    }
    Some(registry)
}

/// Why launch-time model resolution cannot bind a descriptor. Each cause
/// becomes the chat launch error, reported to the operator instead of
/// binding a fabricated fallback descriptor.
#[derive(Debug, thiserror::Error)]
pub(crate) enum CurrentModelError {
    /// The gateway's model catalog could not be fetched.
    #[error("the model catalog fetch failed: {0}")]
    CatalogFetchFailed(#[source] CompletionError),
    /// The selected id is absent from the fetched catalog.
    #[error("the selected model `{0}` is absent from the fetched catalog")]
    SelectionAbsent(String),
}

/// Resolves the dropdown's current model for one run's context. The
/// selection is read at launch, so a selection change takes effect on the
/// next run. A launch with no selection yet - the boot window before the
/// menu's own auto-select settles - binds the retained catalog's first
/// chat-capable model, the same fallback the menu applies. The typed
/// descriptor comes from the gateway's model list through
/// [`fetch_model_catalog`].
///
/// Returns `Ok(None)` only when neither a selection nor a catalog model
/// exists, or the id is not representable; the prompt's declared roles
/// then stay unbound. A failed catalog fetch or a selection absent from
/// the fetched catalog is a reported [`CurrentModelError`], never a
/// fabricated fallback descriptor.
pub(crate) async fn current_model(
    host: &SessionHost,
    base_url: &str,
    api_key: &str,
) -> Result<Option<ModelDescriptor>, CurrentModelError> {
    let Some(selected) = host
        .menu()
        .latest()
        .and_then(|snapshot| snapshot.selected_model)
        .or_else(|| {
            host.catalog()
                .latest_chat()?
                .models
                .first()?
                .get("id")?
                .as_str()
                .map(str::to_owned)
        })
    else {
        return Ok(None);
    };
    let id = match ModelId::gateway(&selected) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "the selected model id is invalid");
            return Ok(None);
        }
    };
    let root = format!("{}/v1", base_url.trim_end_matches('/'));
    let catalog = fetch_model_catalog(&root, api_key)
        .await
        .map_err(CurrentModelError::CatalogFetchFailed)?;
    let descriptor = catalog
        .get(&id)
        .cloned()
        .ok_or_else(|| CurrentModelError::SelectionAbsent(selected))?;
    Ok(Some(descriptor))
}

#[cfg(test)]
#[path = "environment-tests.rs"]
mod tests;
