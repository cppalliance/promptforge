//! The PromptForge model vocabulary: what a model round exchanges, and how a
//! prompt binds models. No transport.
//!
//! [`client`] holds the chat-completions protocol vocabulary: the wire
//! types that go out of the engine in a `Chat` effect and come back in its
//! answer ([`client::Message`], [`client::ToolSchema`],
//! [`client::Completion`]), the request body builder, and the SSE
//! reassembly that folds a streamed body into a [`client::Completion`]
//! under the one strict turn rule set, reporting each decoded
//! [`promptforge_types::wire::StreamDelta`] as it goes. [`model`] holds
//! the catalog and prompt-local binding vocabulary:
//! [`model::ModelCatalog`] built from the gateway's `GET /v1/models`, the
//! validated [`model::ModelId`] identity, and the
//! [`model::ModelBinding`]/[`model::ModelSet`]/[`model::ModelView`] types
//! a host resolves and freezes model selections through, with
//! [`model::CompletionError`] as the failure a round reports.
//!
//! The metrics vocabulary in [`promptforge_types::metrics`] (`Usage`,
//! `LlamaTimings`, `VllmMetrics`, `ClientTiming`, `CallMetrics`) and
//! `StreamDelta` are canonical in `promptforge-types`, and this crate uses
//! them from there: the reassembly parses each response body's call
//! metadata into the metrics types, and [`client::Completion`] holds the
//! result. The model identity/catalog vocabulary ([`model::ModelId`],
//! [`model::ModelCatalog`], [`model::ModelDescriptor`],
//! [`model::ThinkingMode`]) is canonical there too and re-exported through
//! its historical `model` paths.
//!
//! The HTTP client that sends a round to the gateway and fetches its model
//! list is the harness's (`harness-models`), the engine's production host;
//! it reaches this vocabulary through the `promptforge` facade.
//! This crate contains no HTTP, no prompt parser, no Lua runtime, and no
//! executor.

pub mod client;
pub mod detail;
mod error;
pub mod model;
mod normalize;

pub(crate) use crate::error::Result;
pub use crate::error::{Error, Timeout};
