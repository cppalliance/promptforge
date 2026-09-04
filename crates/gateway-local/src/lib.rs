//! Gateway-owned local generative inference: pinned `llama-server`
//! provisioning, the operator artifact cache, and the managed child
//! lifecycle.
//!
//! [`LocalRuntime`] provisions a pinned `llama-server` binary, downloads each
//! configured GGUF into the operator cache, spawns one child per
//! `[[local_model]]`, and registers each as a normal OpenAI-routed
//! [`Model`](gateway_routing::Model). Dropping the runtime kills
//! the children. The blob cache store behind the gateway's `/v1/cache` routes
//! lives in [`cache`]; the artifact store and download machinery in
//! [`artifacts`]; chat-template metadata in [`chat_templates`]; bounded GGUF
//! header inspection in [`gguf`].
//!
//! Failures are reported as [`LocalError`]; an explicit teardown failure is
//! reported as [`ShutdownError`](shared_protocol::ShutdownError).
//! The crate contains no HTTP routing and no error envelopes; those live in
//! the gateway crate.

/// Windows `CREATE_NO_WINDOW` flag: suppresses console windows for child
/// processes spawned from a GUI-subsystem parent.
#[cfg(windows)]
pub(crate) const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub mod artifacts;
pub mod cache;
pub mod chat_templates;
mod dialect;
mod error;
pub mod gguf;
mod launch_templates;
mod runtime;
mod server;
mod sidecar;
#[cfg(test)]
mod testsupport;
mod upstream;

pub use crate::dialect::DialectResolveError;
pub use crate::error::LocalError;
pub use crate::launch_templates::{
    ChatTemplateResolution, ChatTemplateSource, inspect_chat_template,
};
pub use crate::runtime::{LocalRuntime, LocalStartFailure, LocalStartOutcome, resolve_cache_root};
