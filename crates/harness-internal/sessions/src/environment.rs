//! The session run's environment: the Host snapshot a client pushes
//! through the public API and the launch-time resolution of the client's
//! selected model, through the Host's inference broker, into the run's
//! context.
//!
//! Everything here arrives as data or through the broker. The Harness
//! never resolves a gateway, reads a menu, or names a workspace crate: the
//! client pushes a [`HostSnapshot`] whenever its selection or roots change,
//! and each run reads the latest one once the broker has listed its
//! models.

use std::path::PathBuf;
use std::sync::{PoisonError, RwLock};

use harness_runner::performers::InferenceBroker;
use promptforge::model::{CompletionError, ModelDescriptor, ModelId};

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

/// The bindings one Harness holds for every session it serves: the Host
/// snapshot, replaceable by the client.
#[derive(Debug, Default)]
pub struct Bindings {
    host: RwLock<HostSnapshot>,
}

impl Bindings {
    /// Bindings with nothing pushed yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

/// Resolves the client's current model for one run's context: lists
/// `broker`'s models, then reads the Host snapshot from `bindings`, so a
/// broker that holds its list until it has a model to offer starts the
/// run under the selection made while it waited. Returns that snapshot
/// beside the model. A selection change takes effect on the next run, and
/// a launch with no selection binds the first model the broker lists.
///
/// The model is `None` only when there is no selection and the broker
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
    bindings: &Bindings,
    broker: &dyn InferenceBroker,
) -> Result<(HostSnapshot, Option<ModelDescriptor>), CurrentModelError> {
    let catalog = broker
        .models()
        .await
        .map_err(CurrentModelError::CatalogFetchFailed)?;
    let host = bindings.host();
    let Some(selected) = host.selected_model.clone() else {
        let first = catalog.models().first().cloned();
        return Ok((host, first));
    };
    let id = match ModelId::gateway(&selected) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "the selected model id is invalid");
            return Ok((host, None));
        }
    };
    let descriptor = catalog
        .get(&id)
        .cloned()
        .ok_or(CurrentModelError::SelectionAbsent(selected))?;
    Ok((host, Some(descriptor)))
}

#[cfg(test)]
#[path = "environment-tests.rs"]
mod tests;
