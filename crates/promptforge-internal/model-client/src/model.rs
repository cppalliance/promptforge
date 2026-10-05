//! Prompt-local model bindings: catalog, bind/use declarations, and invocation.
//!
//! The Harness builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a
//! pinned offline entry), and the Harness's model client performs the fetch,
//! outside this crate. H1 `models.bind` resolves a description
//! against that catalog under hard constraints, freezes invocation
//! parameters, and stores the result in the Engine's run-scoped model
//! bindings. H2 `models.use` selects at most one binding per section; H1
//! `models.default` supplies the prompt-wide default for sections that omit
//! `models.use`. Model-facing sections with neither binding fail with a
//! model-binding failure surfaced through the run error the Harness receives.

mod error;
mod options;

pub use error::{CompletionError, CompletionErrorKind};
pub use options::{
    CompletionOptions, ModelBinding, ModelInvocation, ModelSet, ModelSetError, ModelView,
    Temperature, TemperatureError,
};
// The model identity/catalog vocabulary is canonical in
// `promptforge-types` and re-exported here so existing
// `promptforge_model_client::model::` paths keep resolving.
pub use promptforge_types::models::{
    ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ModelIdError, ThinkingMode,
};

#[cfg(test)]
mod tests;
