//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! and the axum router so `src/main.rs` stays a thin shell. Start at
//! [`Config::load`] for configuration, [`WorkshopObserver`] for the run
//! event log, [`WaitRegistry`] and [`SessionInputBroker`] for agent input
//! waits, [`AgentSessions`] for the agent-session registry behind
//! `/agents/ws`, and [`router`] for the HTTP API; [`spawn`] runs the whole
//! server in-process on its own thread for embedding binaries.
//!
//! The crate is the composition root of the workshop server
//! decomposition: the feature subsystems (`workshop-sessions`,
//! `workshop-user-state`, `workshop-workspace`), the domain services (`workshop-gateway`,
//! `workshop-status`, `workshop-menu`), and the vocabulary crates
//! (`workshop-protocol`, `workshop-registry`, `workshop-support`) are
//! assembled in `app.rs`, where every subsystem self-registers its
//! routes, state handles, and push channels into the registry.
//!
//! ## Invariants
//!
//! - Tier: shell; may depend on: the vocabulary crates
//!   (`workshop-protocol`, `workshop-registry`, `workshop-support`),
//!   the service crates (`workshop-gateway`, `workshop-menu`,
//!   `workshop-status`), and the feature crates (`workshop-sessions`,
//!   `workshop-user-state`, `workshop-workspace`). Read `AGENTS.md`
//!   before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.

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
pub use workshop_status::{progress, status};

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

pub use app::{AppState, DEFAULT_ADDR, StateError, router};
pub use cross_site::{guard as cross_site_guard, origin_allowed};
pub use gateway::{
    CacheEvent, CacheResponse, GatewayClient, GatewayError, GatewayResponse, SsePayloadStream,
    SwitchOutcome, SwitchResponse,
};
pub use gateway_binding::{GatewayPublicationError, GatewayUpdater};
pub use observer::WorkshopObserver;
pub use push::Push;
pub use resolve::{GatewaySource, ResolveError, ResolvedGateway};
pub use serve::{ServerHandle, SpawnError, Termination, spawn};
pub use workshop_protocol::{Activity, InputFrame, InputResponse};
pub use workshop_sessions::{
    AgentSessions, SessionInputBroker, WaitError, WaitRegistry, deliver_input_response,
};
pub use workshop_support::{
    AgentsConfig, Config, ConfigError, DEFAULT_CONFIG_PATH, GatewayConfig, ServerConfig,
};
