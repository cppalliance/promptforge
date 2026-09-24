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
//! and is re-exported here; the `#[doc(hidden)]` items are the
//! protocol seams the transport that performs a round (the harness's
//! gateway client) shares with the engine's own test client: the request
//! body builder, the SSE reassembly, the read loop over a transport's
//! chunk source, and the internal error type it builds a [`CompletionError`]
//! from. The engine itself never performs a completion.

pub use promptforge_types::models::{
    ModelCatalog, ModelCatalogError, ModelDescriptor, ModelId, ModelIdError, ThinkingMode,
};
// Canonical in `promptforge-types`; re-exported so the streaming hook
// a host hands its chat performer names one path.
#[doc(hidden)]
pub use promptforge_model_client::client::{
    ChunkSource, ToolSchemaError, build_request_body, escape_controls, read_body_capped,
    read_completion_stream,
};
pub use promptforge_model_client::client::{
    Completion, CompletionResult, Message, ToolArguments, ToolCall, ToolSchema,
};
pub use promptforge_model_client::model::{
    CompletionError, CompletionErrorKind, CompletionOptions, ModelBinding, ModelInvocation,
    ModelSet, ModelView, Temperature, TemperatureError,
};
#[doc(hidden)]
pub use promptforge_model_client::{Error as ClientError, Timeout as ClientTimeout};
pub use promptforge_types::wire::StreamDelta;

#[cfg(test)]
mod tests;
