//! A run's environment: the Host snapshot a request carries and the
//! resolution of the Host's selected model, through its inference broker,
//! into the run's context.
//!
//! Everything here arrives as data or through the broker. The Harness
//! never resolves a gateway, reads a menu, or names a workspace crate: the
//! Host puts its selection and roots in each request's [`HostSnapshot`],
//! and the run binds the model the broker lists under that selection.

use std::path::PathBuf;

use promptforge::model::{CompletionError, ModelDescriptor, ModelId};

use crate::performers::InferenceBroker;

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

/// Why launch-time model resolution cannot bind a descriptor. Each cause
/// becomes the run's error, reported to the operator instead of binding
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

/// Resolves the Host's current model for one run's context: lists
/// `broker`'s models and looks up `host`'s selection in the list. A launch
/// with no selection binds the first model the broker lists.
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
    host: &HostSnapshot,
    broker: &dyn InferenceBroker,
) -> Result<Option<ModelDescriptor>, CurrentModelError> {
    let catalog = broker
        .models()
        .await
        .map_err(CurrentModelError::CatalogFetchFailed)?;
    let Some(selected) = host.selected_model.clone() else {
        return Ok(catalog.models().first().cloned());
    };
    let id = match ModelId::gateway(&selected) {
        Ok(id) => id,
        Err(error) => {
            tracing::warn!(%error, "the selected model id is invalid");
            return Ok(None);
        }
    };
    catalog
        .get(&id)
        .cloned()
        .map(Some)
        .ok_or(CurrentModelError::SelectionAbsent(selected))
}

#[cfg(test)]
#[path = "environment-tests.rs"]
mod tests;
