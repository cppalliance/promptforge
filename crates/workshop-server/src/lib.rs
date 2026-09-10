//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! and the axum router so `src/main.rs` stays a thin shell. Start at
//! [`Config::load`] for configuration, [`WorkshopObserver`] for the run
//! event log, [`WaitRegistry`] and [`UserInputTool`] for agent input
//! waits, [`AgentSessions`] for the agent-session registry behind
//! `/agents/ws`, and [`router`] for the HTTP API; [`spawn`] runs the whole
//! server in-process on its own thread for embedding binaries.

mod app;
mod assets;
mod atomic;
mod backoff;
mod catalog;
mod config;
mod cross_site;
mod csp;
mod deadline;
mod error;
mod gateway;
mod gateway_binding;
mod gateway_progress;
mod heartbeat;
mod input;
mod menu;
mod observer;
mod progress;
mod protocol;
mod push;
mod relay;
mod resolve;
mod routes;
mod serve;
mod session;
mod session_agents;
mod status;
#[cfg(any(test, feature = "test-fixtures"))]
mod test_gateway;
mod workspace;

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
pub use config::{
    AgentsConfig, Config, ConfigError, DEFAULT_CONFIG_PATH, GatewayConfig, ServerConfig,
};
pub use cross_site::{guard as cross_site_guard, origin_allowed};
pub use gateway::{
    CacheEvent, CacheResponse, GatewayClient, GatewayError, GatewayResponse, SsePayloadStream,
    SwitchEvent, SwitchEventStream, SwitchResponse, switch_events,
};
pub use gateway_binding::{GatewayPublicationError, GatewayUpdater};
pub use input::{
    SessionInputBroker, UserInputTool, WaitError, WaitRegistry, deliver_input_response,
};
pub use observer::WorkshopObserver;
pub use protocol::{Activity, InputFrame, InputResponse};
pub use push::Push;
pub use resolve::{GatewaySource, ResolveError, ResolvedGateway};
pub use serve::{ServerHandle, SpawnError, Termination, spawn};
pub use session_agents::AgentSessions;
