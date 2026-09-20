//! Prompt-local model bindings: catalog, bind/use declarations, and invocation.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a pinned
//! offline entry); the fetch itself is the host's, performed by the harness's
//! model client, never by this crate. H1 `models.bind` resolves a description
//! against that catalog under hard constraints, freezes invocation
//! parameters, and stores the result in the host's run-scoped model
//! bindings. H2 `models.use` selects at most one binding per section; H1
//! `models.default` supplies the prompt-wide default for sections that omit
//! `models.use`. Model-facing sections with neither binding fail with a
//! model-binding failure surfaced through the host's run error.

mod error;
mod options;

pub use error::{CompletionError, CompletionErrorKind};
pub use options::{
    CompletionOptions, ModelBinding, ModelInvocation, ModelSet, ModelView, Temperature,
    TemperatureError,
};
// The model identity/catalog vocabulary is canonical in
// `promptforge-api-types` and re-exported here so existing
// `promptforge_model_client::model::` paths keep resolving.
pub use promptforge_api_types::models::{
    ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ModelIdError, ThinkingMode,
};

#[cfg(test)]
mod tests;
