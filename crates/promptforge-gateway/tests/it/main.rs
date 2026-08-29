//! End-to-end tests: a fake OpenAI backend behind the real gateway, driven by
//! the executor's real `GatewayClient`. This keeps the two independent
//! definitions of the wire shape honest.
//!
//! Determinism: the gateway is served on a caller-owned ephemeral listener
//! (no port race), shutdown is driven by a rendezvous `TestServer` fixture,
//! and concurrency tests use an arrivals channel plus per-request release
//! handles instead of sleeps.
//!
//! The suite is split into cohesive area modules (IT-007): shared scaffolding
//! lives in [`support`]; tests are grouped by surface into [`chat`],
//! [`embeddings`], [`rerank`], [`web_search`], [`queue`], [`profiles`], and
//! [`local`]. The `cuda` module holds the feature-gated live CUDA proof.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

mod support;

mod cache;
mod chat;
#[cfg(feature = "llama-cuda")]
mod cuda;
mod embeddings;
mod local;
mod profiles;
mod queue;
mod rerank;
mod web_search;
