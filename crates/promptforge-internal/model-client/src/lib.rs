//! The PromptForge model vocabulary: what a model round exchanges, and how a
//! prompt binds models. No transport.
//!
//! [`client`] holds the chat-completions protocol vocabulary: the wire
//! types that go out of the Engine in a `Chat` effect and come back in its
//! answer ([`client::Message`], [`client::ToolSchema`],
//! [`client::Completion`]), with the validating constructors that run the
//! neutral reply checks on every completion. [`model`] holds the catalog
//! and prompt-local binding vocabulary: [`model::ModelCatalog`] built from
//! the gateway's `GET /v1/models`, the validated [`model::ModelId`]
//! identity, and the
//! [`model::ModelBinding`]/[`model::ModelSet`]/[`model::ModelView`] types
//! model selections resolve and freeze through, with
//! [`model::CompletionError`] as the failure a round reports.
//!
//! The metrics vocabulary in [`promptforge_types::metrics`] (`Usage`,
//! `LlamaTimings`, `VllmMetrics`, `ClientTiming`, `CallMetrics`) is
//! canonical in `promptforge-types`, and this crate uses it from there:
//! [`client::Completion`] holds a round's metrics. The
//! model identity/catalog vocabulary ([`model::ModelId`],
//! [`model::ModelCatalog`], [`model::ModelDescriptor`],
//! [`model::ThinkingMode`]) is canonical there too and re-exported through
//! its historical `model` paths.
//!
//! The OpenAI wire code (the request body builder, the SSE reassembly, the
//! body and metadata parse, and the HTTP failure classifier) lives in
//! `harness-gateway-client`, which reaches this vocabulary through the
//! `promptforge` facade.
//! This crate contains no HTTP, no wire parsing, no prompt parser, no Lua
//! runtime, and no executor.
//!
//! ## Invariants
//!
//! - May depend on: `promptforge-types`. `cargo test -p build-xtask`
//!   enforces the product and container boundaries.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

pub mod client;
pub mod detail;
pub mod model;
mod normalize;

/// Crate-internal result alias over the failure a model round reports.
pub(crate) type Result<T> = std::result::Result<T, model::CompletionError>;
