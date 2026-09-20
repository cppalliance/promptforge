//! harness-models - the harness's model client: the HTTP transport that
//! performs the engine's `Chat` effects against the bound gateway and
//! streams deltas back to the session, and the catalog fetch a host
//! resolves model selections against.
//!
//! [`GatewayClient`] speaks the always-streaming `/chat/completions` SSE
//! shape to one gateway URL with, usually, the gateway's shared bearer
//! key: [`GatewayClient::complete`] sends the wire vocabulary's request
//! body, reads the stream under the run's byte cap and timeout, hands each
//! `data:` payload to the engine's shared SSE reassembly, invokes the
//! caller's delta callback live, and returns the one
//! [`Completion`](promptforge_api_runtime::model::Completion) the round
//! produced. [`fetch_model_catalog`] reads the gateway's typed model list
//! for host-side concerns (the Workshop dropdown and its selection
//! resolution). The client holds only the gateway's URL and the shared
//! key; the vendor credential lives in the gateway, so no host ever sees
//! it. [`GatewayChatPerformer`] is the client as the effect loop performs
//! a `Chat` effect through it: one round per effect, with a section's own
//! round streaming its deltas to a [`DeltaSink`] the session drains.
//!
//! This is a Gateway model client, not a universal transport: it speaks
//! the one protocol the gateway serves. Everything it exchanges is the
//! engine's vocabulary, reached through the `promptforge-api-runtime`
//! door; the metrics it reports are the canonical
//! `promptforge-api-types` ones, never a parallel model.
//!
//! ## Invariants
//!
//! - Family: harness, private to `crates/harness/`; may depend on:
//!   `promptforge-api-runtime`, `promptforge-api-types`,
//!   `gateway-api-types`, `gateway-api-discovery`, `shared-*`, and its
//!   container siblings.
//!   Never on a `workshop-*` crate, a private `gateway-*` crate, or a
//!   `promptforge-*` crate behind the door. Read `AGENTS.md` before adding
//!   an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - A gateway bearer key is never written to logs or `Debug` output.
//! - Nothing in this crate spawns a tokio task directly; the harness
//!   spawns only through the instrumented wrapper in `harness-runner`
//!   (enforced by this crate's `clippy.toml`).

mod catalog;
mod config;
mod performer;
mod transport;

pub use catalog::fetch_model_catalog;
pub use config::{GatewayEndpoint, SecretError, SecretString};
pub use performer::{DeltaSink, GatewayChatPerformer};
pub use promptforge_api_runtime::model::{CompletionError, CompletionErrorKind};
pub use transport::GatewayClient;
