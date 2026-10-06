//! The model vocabulary a caller exchanges with a run: what a `Chat` effect
//! includes ([`Message`], [`ToolSchema`], [`CompletionOptions`],
//! [`ModelBinding`]) and what its answer returns ([`Completion`],
//! [`CompletionResult`], [`CompletionError`]), plus the prompt-local
//! binding vocabulary ([`ModelSet`], [`ModelView`], [`ModelInvocation`])
//! and the catalog identity model selections resolve against
//! ([`ModelCatalog`](promptforge_types::models::ModelCatalog), [`ModelDescriptor`], [`ModelId`]).
//!
//! H1 `models.default` parks a declared role as the prompt-wide default;
//! H2 `models.use` selects at most one binding per section. Model-facing
//! sections with neither fail with a model-binding failure surfaced
//! through [`crate::RunError`].
//!
//! The implementation sits in the private `promptforge-model-client` crate
//! and is re-exported here. The transport codec a round's performer runs
//! (the request body builder, the read loop over a transport's chunk
//! source, and the error type it builds a [`CompletionError`] from) is not:
//! the facade publishes it from `promptforge-model-client` directly. The
//! Engine itself never performs a completion.

pub(crate) use promptforge_model_client::client::{
    Completion, CompletionResult, Message, ToolCall, ToolSchema,
};
pub(crate) use promptforge_model_client::model::{
    CompletionError, CompletionErrorKind, CompletionOptions, ModelBinding, ModelInvocation,
    ModelSet, ModelView, Temperature,
};
pub(crate) use promptforge_types::models::{ModelDescriptor, ModelId, ThinkingMode};

#[cfg(test)]
mod tests;
