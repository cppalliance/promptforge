//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! and the axum router. Start at
//! [`Config::load`] for configuration, `AgentSessions` for the
//! agent-session opener behind `/agents/ws` (every session runs in the
//! harness, reached through `harness`), and [`router`] for the HTTP
//! API; [`spawn`] runs the whole server in-process on its own thread for
//! embedding binaries.
//!
//! The crate is the composition root of the workshop server
//! decomposition: the feature subsystems (`workshop-user-state`,
//! `workshop-workspace`, and the agent-sessions subsystem in `agents`,
//! which serves the `/agents/ws` agent-session socket and the
//! `/v1/models` catalog relay and owns that socket's wire frames in
//! `agents::wire`), the `/ws` workshop socket in
//! `workshop_socket` (the two sockets share `websocket`), the
//! domain services (`workshop-gateway`, `workshop-status`,
//! `workshop-menu`), and the vocabulary crates (`workshop-protocol`,
//! `workshop-registry`, `workshop-support`) are assembled in `app`
//! (helpers in `app::compose`), where every subsystem self-registers its
//! routes, state handles, and push channels into the registry - the
//! harness among them.
//!
//! ## Invariants
//!
//! - Tier: server; may depend on: the vocabulary crates
//!   (`workshop-protocol`, `workshop-registry`, `workshop-support`),
//!   the service crates (`workshop-gateway`, `workshop-menu`,
//!   `workshop-status`), the feature crates (`workshop-user-state`,
//!   `workshop-workspace`), the harness's public API `harness`, and
//!   the engine's public API `promptforge`. Read the repository-root
//!   `AGENTS.md` and `crates/workshop/server/AGENTS.md` before adding
//!   an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - One task owns each socket: a single `select!` loop reads inbound
//!   frames and writes every outbound frame itself - no outbox channel,
//!   no writer task. Agent sessions are the documented carve-out: they
//!   outlive sockets on purpose, and the harness keeps their table.
//! - The harness reads the server's state as data pushed through its
//!   public API (the gateway binding, the chat catalog, the host
//!   snapshot); the server never hands it a bus, a registry, or a
//!   callback into itself. Status-bar reporting for a session is derived
//!   in the server from the session's events, deltas, and error reports.
//! - The workspace's granted roots are read through the registry's
//!   `WorkspaceRoots` slot, never by naming the workspace crate's
//!   internals: subsystems meet through the registry.
//! - Every WebSocket upgrade checks its `Origin`: `/ws` and `/agents/ws`
//!   admit any loopback origin through `cross_site::origin_allowed`, and
//!   `/v1/realtime` applies a stricter same-origin check that requires a
//!   browser `Origin` to match the request's own authority. The
//!   cross-site guard stays the security boundary.
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
mod websocket;
mod workshop_socket;

/// Crate-internal test seams, re-exported to the integration-test binary.
/// The socket behavior tests drive the status, catalog, and menu buses,
/// the health flag, the backoff, and the heartbeat directly, so those
/// types surface here, hidden from the docs. The module exists only in
/// test builds and under the `test-fixtures` feature, which the crate's
/// own dev-dependency enables for every test build while production
/// builds do not.
#[cfg(any(test, feature = "test-fixtures"))]
#[doc(hidden)]
pub mod fixtures;

pub use app::{AppState, router};
pub use serve::{ServerHandle, SpawnError, Termination, spawn};
pub use workshop_gateway::{GatewayPublicationError, GatewayUpdater, ResolvedGateway};
pub use workshop_protocol::InputResponse;
pub use workshop_support::{AgentsConfig, Config, GatewayConfig, ServerConfig};
