//! Gateway-owned local generative inference: pinned `llama-server`
//! provisioning, the operator artifact cache, and the managed child
//! lifecycle.
//!
//! [`LocalRuntime`] provisions a pinned `llama-server` binary, downloads each
//! configured GGUF into the operator cache, spawns one child per
//! `[[local_model]]`, and registers each as a normal OpenAI-routed
//! [`Model`](promptforge_gateway_routing::Model). Dropping the runtime kills
//! the children. The blob cache store behind the gateway's `/v1/cache` routes
//! lives in [`cache`]; the artifact store and download machinery in
//! [`artifacts`]; bounded GGUF header inspection in [`gguf`].
//!
//! Failures are reported as [`LocalError`]; an explicit teardown failure is
//! reported as [`ShutdownError`](promptforge_gateway_protocol::ShutdownError).
//! The crate contains no HTTP routing and no error envelopes; those live in
//! the gateway crate.

pub mod artifacts;
pub mod cache;
mod dialect;
mod error;
pub mod gguf;
#[cfg(llama_cuda_embedded)]
mod llama_cuda_bundle;
mod runtime;
mod server;
mod sidecar;
#[cfg(test)]
mod testsupport;
mod upstream;

pub use crate::dialect::DialectResolveError;
pub use crate::error::LocalError;
pub use crate::runtime::{LocalRuntime, resolve_cache_root};
