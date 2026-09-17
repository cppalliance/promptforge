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
//! The implementation lives in the `promptforge-model-client` crate. This
//! module is the crate-internal import surface for it; hosts name the model
//! vocabulary through `promptforge-api-types`'s `models` module and the
//! completion error types through [`crate::client`].

pub(crate) use promptforge_model_client::model::{
    CompletionOptions, ModelBinding, ModelDescriptor, ModelId, ModelInvocation, ModelSet,
    ModelView, ThinkingMode,
};

#[cfg(test)]
mod tests;
