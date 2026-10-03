//! The session run's environment: the bindings a client pushes through
//! the public API (the chat catalog and the Host snapshot) and the
//! launch-time resolution of the client's selected model, through the
//! Host's inference broker, into the run's context.
//!
//! Everything here arrives as data or through the broker. The Harness
//! never resolves a gateway, reads a menu, or names a workspace crate: the
//! client pushes a [`CatalogBinding`] whenever its chat-capable model list
//! changes and a [`HostSnapshot`] whenever its selection or roots change.
//! Sessions observe catalog generation changes through a watch and read
//! the latest value at launch.

use std::fmt;
use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use harness_runner::performers::InferenceBroker;
use promptforge::model::{CompletionError, ModelDescriptor, ModelId};
use tokio::sync::watch;

/// One generation of the client's chat-capable model catalog.
///
/// A session freezes the catalog generation it launched under; a later
/// generation whose `models` differ retires the run and relaunches it
/// over the retained transcript once the accepted turn settles. An empty
/// `models` list means no chat-capable model exists at this generation:
/// a session waits on it rather than launching or relaunching a run.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatalogBinding {
    /// Monotonic generation the client assigns to each change.
    pub generation: u64,
    /// The chat-capable entries, as the gateway lists them; empty when
    /// none is available.
    pub models: Vec<serde_json::Value>,
}

/// The Host state a run reads at launch: what the `ui()` global serves
/// and the model the prompt's roles bind to.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HostSnapshot {
    /// The client's selected model id, when one is selected.
    pub selected_model: Option<String>,
    /// The workspace roots the client has granted; the first is the
    /// `ui()` snapshot's `workspace_root`.
    pub workspace_roots: Vec<PathBuf>,
}

impl HostSnapshot {
    /// The `ui()` snapshot: `selected_model` and `workspace_root`, each
    /// `null` when absent.
    #[must_use]
    pub fn ui(&self) -> serde_json::Value {
        let root = self
            .workspace_roots
            .first()
            .map(|root| root.display().to_string());
        serde_json::json!({ "selected_model": self.selected_model, "workspace_root": root })
    }
}

/// The bindings one Harness holds for every session it serves, each
/// replaceable by the client.
///
/// The catalog's generation watch holds the latest generation (`None`
/// before the first push); a session that observes a change reads the
/// catalog behind it.
pub struct Bindings {
    catalog: RwLock<Option<CatalogBinding>>,
    catalog_generation: watch::Sender<Option<u64>>,
    host: RwLock<HostSnapshot>,
}

impl fmt::Debug for Bindings {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Bindings")
            .field("catalog_generation", &self.catalog().map(|c| c.generation))
            .field("host", &self.host())
            .finish()
    }
}

impl Default for Bindings {
    fn default() -> Self {
        Self::new()
    }
}

impl Bindings {
    /// Bindings with nothing pushed yet.
    #[must_use]
    pub fn new() -> Self {
        Self {
            catalog: RwLock::new(None),
            catalog_generation: watch::Sender::new(None),
            host: RwLock::new(HostSnapshot::default()),
        }
    }

    /// Replaces the chat catalog binding and wakes the sessions watching
    /// its generation.
    pub fn set_catalog(&self, catalog: CatalogBinding) {
        let generation = catalog.generation;
        *self.catalog.write().unwrap_or_else(PoisonError::into_inner) = Some(catalog);
        self.catalog_generation.send_replace(Some(generation));
    }

    /// The most recently pushed catalog, or `None` before the first push.
    #[must_use]
    pub fn catalog(&self) -> Option<CatalogBinding> {
        self.catalog
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// A watch on the catalog generation.
    #[must_use]
    pub fn subscribe_catalog(&self) -> watch::Receiver<Option<u64>> {
        self.catalog_generation.subscribe()
    }

    /// Replaces the Host snapshot; the next launch reads it.
    pub fn set_host(&self, host: HostSnapshot) {
        *self.host.write().unwrap_or_else(PoisonError::into_inner) = host;
    }

    /// The current Host snapshot.
    #[must_use]
    pub fn host(&self) -> HostSnapshot {
        self.host
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

/// Why launch-time model resolution cannot bind a descriptor. Each cause
/// becomes the launch error, reported to the operator instead of binding
/// a fabricated fallback descriptor.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CurrentModelError {
    /// The broker's model list could not be fetched; the broker's failure
    /// is the source.
    #[error("the model catalog could not be fetched")]
    CatalogFetchFailed(#[source] CompletionError),
    /// The selected id is absent from the fetched catalog.
    #[error("the selected model `{0}` is absent from the fetched catalog")]
    SelectionAbsent(String),
}

/// Resolves the client's current model for one run's context through
/// `broker`'s model list. The selection is read at launch, so a selection
/// change takes effect on the next run. A launch with no selection binds
/// the first model the broker lists.
///
/// Returns `Ok(None)` only when there is no selection and the broker
/// lists no model, or the selected id is not representable; the prompt's
/// declared roles then stay unbound. A failed listing or a selection
/// absent from the list is a reported [`CurrentModelError`], never a
/// fabricated fallback descriptor.
///
/// # Errors
/// Returns [`CurrentModelError::CatalogFetchFailed`] when the broker's
/// model list cannot be fetched and [`CurrentModelError::SelectionAbsent`]
/// when the selected id is not in it.
pub async fn current_model(
    host: &HostSnapshot,
    broker: &dyn InferenceBroker,
) -> Result<Option<ModelDescriptor>, CurrentModelError> {
    let selection = match host.selected_model.clone() {
        None => None,
        Some(selected) => match ModelId::gateway(&selected) {
            Ok(id) => Some((selected, id)),
            Err(error) => {
                tracing::warn!(%error, "the selected model id is invalid");
                return Ok(None);
            }
        },
    };
    let catalog = broker
        .models()
        .await
        .map_err(CurrentModelError::CatalogFetchFailed)?;
    let Some((selected, id)) = selection else {
        return Ok(catalog.models().first().cloned());
    };
    let descriptor = catalog
        .get(&id)
        .cloned()
        .ok_or_else(|| CurrentModelError::SelectionAbsent(selected))?;
    Ok(Some(descriptor))
}

#[cfg(test)]
#[path = "environment-tests.rs"]
mod tests;
