//! Prompt-local model bindings: catalog, bind/use declarations, and invocation.
//!
//! A host builds a [`ModelCatalog`] from gateway `GET /v1/models` (or a pinned
//! offline entry). H1 `models.bind` resolves a description against that catalog
//! under hard constraints, freezes invocation parameters, and stores the result
//! in the run's crate-private model bindings. H2 `models.use` selects at most
//! one binding per
//! section; H1 `models.default` supplies the prompt-wide default for sections
//! that omit `models.use`. Model-facing sections with neither binding fail with
//! a model-binding failure surfaced through [`crate::RunError`].
//!
//! The implementation lives in the `promptforge-gateway-client` crate and is
//! re-exported here unchanged, so existing `promptforge_core::model::*` paths
//! keep working.

#[cfg(test)]
pub(crate) use promptforge_gateway_client::model::ModelInvocation;
pub use promptforge_gateway_client::model::{
    CompletionError, CompletionErrorKind, CompletionOptions, ModelCatalog, ModelCatalogError,
    ModelDescriptor, ModelId, ModelIdError, TemperatureError, ThinkingMode, fetch_model_catalog,
};
pub(crate) use promptforge_gateway_client::model::{
    ModelBindOpts, ModelBinding, ModelResolver, ModelSet, ModelView, PickerModelResolver,
    ResolvedModel, Temperature,
};

#[cfg(test)]
mod tests;
