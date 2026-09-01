//! The PromptForge gateway's model client and model-catalog vocabulary.
//!
//! [`client`] holds the `OpenAI`-compatible chat-completions transport:
//! [`client::GatewayClient`] speaks the non-streaming `/chat/completions` shape
//! to one gateway URL with a shared bearer key, and the wire types
//! ([`client::Message`], [`client::ToolSchema`], [`client::Completion`]) are
//! what it exchanges. [`model`] holds the catalog and prompt-local binding
//! vocabulary: [`model::ModelCatalog`] built from the gateway's
//! `GET /v1/models`, the validated [`model::ModelId`] identity, and the
//! [`model::ModelBinding`]/[`model::ModelSet`]/[`model::ModelView`] types a
//! host resolves and freezes model selections through.
//!
//! The crate contains no prompt parser, no Lua runtime, and no executor; it is
//! the gateway's model client only, never a universal client.

pub mod client;
mod error;
pub mod model;
mod normalize;

#[doc(hidden)]
pub use crate::error::Error;
pub(crate) use crate::error::Result;
