//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! and the axum router so `src/main.rs` stays a thin shell. Start at
//! [`Config::load`] for configuration, [`AgentSessions`] for the
//! agent-session opener behind `/agents/ws` (every session runs in the
//! harness, reached through `harness-api`), and [`router`] for the HTTP
//! API; [`spawn`] runs the whole server in-process on its own thread for
//! embedding binaries.
//!
//! The crate is the composition root of the workshop server
//! decomposition: the feature subsystems (`workshop-user-state`,
//! `workshop-workspace`, and the sessions subsystem in `agents`: the
//! `/ws` workbench socket, the `/agents/ws` agent-session socket, and the
//! `/v1/models` catalog relay), the domain services (`workshop-gateway`,
//! `workshop-status`, `workshop-menu`), and the vocabulary crates
//! (`workshop-protocol`, `workshop-registry`, `workshop-support`) are
//! assembled in `app.rs`, where every subsystem self-registers its
//! routes, state handles, and push channels into the registry - the
//! harness among them.
//!
//! ## Invariants
//!
//! - Tier: shell; may depend on: the vocabulary crates
//!   (`workshop-protocol`, `workshop-registry`, `workshop-support`),
//!   the service crates (`workshop-gateway`, `workshop-menu`,
//!   `workshop-status`), the feature crates (`workshop-user-state`,
//!   `workshop-workspace`), the harness's public API `harness-api`, and
//!   the engine's vocabulary `promptforge-api-types`. Read `AGENTS.md`
//!   before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - One task owns each socket: a single `select!` loop reads inbound
//!   frames and writes every outbound frame itself - no outbox channel,
//!   no writer task. Agent sessions are the documented carve-out: they
//!   outlive sockets on purpose, and the harness keeps their table.
//! - The harness reads the shell's state as data pushed through its
//!   public API (the gateway binding, the chat catalog, the host
//!   snapshot); the shell never hands it a bus, a registry, or a
//!   callback into itself. Status-bar reporting for a session is derived
//!   in the shell from the session's events, deltas, and error reports.
//! - The workspace's granted roots are read through the registry's
//!   `WorkspaceRoots` slot, never by naming the workspace crate's
//!   internals: subsystems meet through the registry.
//! - The shell's WebSocket origin policy is applied to every upgrade;
//!   the cross-site guard stays the security boundary.
//! - A dying input wait is an outcome, never silence: the harness's wait
//!   registry pushes a cancelled frame for every unresolved wait it
//!   drops, and the agent socket renders it as `input_cancelled`.

mod agents;
mod app;
mod assets;
mod cross_site;
mod csp;
mod error;
mod routes;
mod serve;

// The extracted subsystem crates, aliased at their pre-decomposition
// module paths so the shell's internals read as they did before the
// split. The tier graph is enforced by `cargo test -p build-xtask`.
pub use workshop_gateway::{
    gateway, gateway_binding, gateway_progress, heartbeat, observer, resolve,
};
pub use workshop_menu::{catalog, menu};
pub use workshop_status::status;

/// The intent-named push facade over the registry's producer sink slots:
/// business code reports what happened and never chooses a severity or
/// builds a bus payload.
pub mod push {
    pub use workshop_registry::Push;
}

#[cfg(any(test, feature = "test-fixtures"))]
pub use workshop_gateway::test_gateway;

/// Crate-internal test seams, re-exported to the integration-test binary.
/// The socket behavior tests drive the status, catalog, and menu buses,
/// the health flag, the backoff, and the heartbeat directly, so those
/// types surface here; Rust visibility cannot be feature-gated, so these
/// re-exports are present in every build and hidden from the docs. The
/// fixture helpers with test-only dependencies stay behind the
/// `test-fixtures` feature, which the crate's own dev-dependency enables
/// for every test build while production builds do not.
#[doc(hidden)]
pub mod fixtures;

pub use agents::AgentSessions;
pub use app::{AppState, DEFAULT_ADDR, StateError, router};
pub use cross_site::{guard as cross_site_guard, origin_allowed};
pub use gateway::{
    CacheEvent, CacheResponse, GatewayClient, GatewayError, GatewayResponse, SsePayloadStream,
    SwitchOutcome, SwitchResponse,
};
pub use gateway_binding::{GatewayPublicationError, GatewayUpdater};
/// The refusal an answered input wait returns when its token names no
/// unresolved wait: the harness's own, named here so an embedding host
/// keeps one import path.
pub use harness_api::WaitError;
pub use push::Push;
pub use resolve::{GatewaySource, ResolveError, ResolvedGateway};
pub use serve::{ServerHandle, SpawnError, Termination, spawn};
pub use workshop_protocol::{Activity, InputFrame, InputResponse};
pub use workshop_support::{
    AgentsConfig, Config, ConfigError, DEFAULT_CONFIG_PATH, GatewayConfig, ServerConfig,
};
