//! The model vocabulary a host exchanges with a run: what a `Chat` effect
//! includes ([`Message`], [`ToolSchema`], [`CompletionOptions`],
//! [`ModelBinding`]) and what its answer returns ([`Completion`],
//! [`CompletionResult`], [`CompletionError`]), plus the prompt-local
//! binding vocabulary ([`ModelSet`], [`ModelView`], [`ModelInvocation`])
//! and the catalog identity the host resolves selections against
//! ([`ModelCatalog`], [`ModelDescriptor`], [`ModelId`]).
//!
//! A host builds a [`ModelCatalog`] from the gateway's `GET /v1/models`
//! (or a pinned offline entry). H1 `models.default` parks a declared role
//! as the prompt-wide default; H2 `models.use` selects at most one binding
//! per section. Model-facing sections with neither fail with a
//! model-binding failure surfaced through [`crate::RunError`].
//!
//! The implementation sits in the private `promptforge-model-client` crate
//! and is re-exported here. The transport codec a round's performer runs
//! (the request body builder, the read loop over a transport's chunk
//! source, and the error type it builds a [`CompletionError`] from) is not:
//! the facade publishes it from `promptforge-model-client` directly, and
//! the engine's own test client imports it from there. The engine itself
//! never performs a completion.

pub use promptforge_model_client::client::{
    Completion, CompletionResult, Message, ToolArguments, ToolCall, ToolSchema,
};
pub use promptforge_model_client::model::{
    CompletionError, CompletionErrorKind, CompletionOptions, ModelBinding, ModelInvocation,
    ModelSet, ModelView, Temperature, TemperatureError,
};
pub use promptforge_types::models::{
    ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ModelIdError, ThinkingMode,
};
// Canonical in `promptforge-types`; re-exported so the streaming hook
// a host hands its chat performer names one path.
pub use promptforge_types::wire::StreamDelta;

#[cfg(test)]
mod tests;
