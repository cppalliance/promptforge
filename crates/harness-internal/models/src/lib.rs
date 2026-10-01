//! harness-models - the Harness's model client: the HTTP transport that
//! performs the Engine's `Chat` effects against the bound gateway and
//! streams deltas back to the session, and the catalog fetch.
//!
//! [`GatewayClient`] speaks the always-streaming `/chat/completions` SSE
//! shape to one gateway URL with, usually, the gateway's shared bearer
//! key: [`GatewayClient::complete`] sends the wire vocabulary's request
//! body, reads the stream under the run's byte cap and timeout, hands each
//! `data:` payload to the Engine's shared SSE reassembly, invokes the
//! caller's delta callback live, and returns the one
//! [`Completion`](promptforge::model::Completion) the round
//! produced. [`fetch_model_catalog`] reads the gateway's typed model
//! list. The client holds only the gateway's URL and the shared key; the
//! vendor credential sits in the gateway. [`GatewayChatPerformer`] is the client as the effect loop performs
//! a `Chat` effect through it: one round per effect, with a section's own
//! round streaming its deltas to a [`DeltaSink`] the session drains.
//!
//! This is a Gateway model client, not a universal transport: it speaks
//! the one protocol the gateway serves. Everything it exchanges is the
//! Engine's vocabulary, reached through the `promptforge` crate; the
//! metrics it reports are the canonical `promptforge::metrics` ones.
//!
//! ## Invariants
//!
//! - Family: Harness, private to `crates/harness-internal/`; may depend
//!   on: `promptforge` and container siblings only.
//!   Never on a `workshop-*`, `gateway-*`, or `shared-*` crate, or a
//!   private `promptforge-*` crate. Read `AGENTS.md` before adding an
//!   import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - A gateway bearer key is never written to logs or `Debug` output.
//! - Nothing in this crate spawns a tokio task directly; the Harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod catalog;
mod config;
mod performer;
mod transport;

pub use catalog::fetch_model_catalog;
pub use config::{GatewayEndpoint, SecretError, SecretString};
pub use performer::{DeltaSink, GatewayChatPerformer};
pub use promptforge::model::{CompletionError, CompletionErrorKind};
pub use transport::GatewayClient;
