//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! and the axum router. Start at
//! [`Config::load`] for configuration, `AgentSessions` for the
//! agent-session launcher behind `/agents/ws` (each conversation's run
//! is a Harness of its own, reached through `harness`), and [`router`]
//! for the HTTP API; [`spawn`] runs the whole server in-process on its
//! own thread for embedding binaries.
//!
//! The crate is the composition root of the workshop server
//! decomposition: the feature subsystems (`workshop-user-state`,
//! `workshop-workspace`, and the agent-sessions subsystem in `agents`,
//! which serves the `/agents/ws` agent-session socket and the
//! `/v1/models` catalog relay, owns that socket's wire frames in
//! `agents::wire`, builds each conversation's run over `workshop-agents`,
//! the conversation layer, and records every run in the run log of
//! `workshop-run-log`), the `/ws` workshop socket in
//! `workshop_socket` (the two sockets share `websocket`), the
//! domain services (`workshop-gateway`, `workshop-status`,
//! `workshop-menu`), and the vocabulary crates (`workshop-protocol`,
//! `workshop-registry`, `workshop-support`) are assembled in `app`
//! (helpers in `app::compose`), where every subsystem self-registers its
//! routes, state handles, and push channels into the registry.
//!
//! ## Invariants
//!
//! - Tier: server; may depend on: the vocabulary crates
//!   (`workshop-protocol`, `workshop-registry`, `workshop-support`),
//!   the service crates (`workshop-gateway`, `workshop-menu`,
//!   `workshop-status`), the feature crates (`workshop-agents`,
//!   `workshop-run-log`, `workshop-user-state`, `workshop-workspace`),
//!   the Harness's public crates `harness` and `harness-gateway-client`,
//!   the Plugin crates `plugin-web`, `plugin-user-input`, and
//!   `plugin-mcp`, and the Engine's public crates `promptforge` and
//!   `promptforge-plugin`.
//!   `cargo test -p build-xtask` enforces the product and container
//!   boundaries. Read `crates/workshop/server/AGENTS.md` before adding an
//!   import.
//! - One task owns each socket: a single `select!` loop reads inbound
//!   frames and writes every outbound frame itself - no outbox channel,
//!   no writer task. Agent conversations are the documented carve-out:
//!   they outlive sockets on purpose.
//! - Status-bar reporting for a conversation is derived in the server
//!   from the conversation's events, deltas, and failure reports.
//! - The workspace's granted roots are read through the registry's
//!   `WorkspaceRoots` slot, never by naming the workspace crate's
//!   internals: subsystems meet through the registry.
//! - Every API route sits behind `cross_site::guard`, which refuses
//!   cross-site requests and any `Host` that names a non-loopback
//!   authority; `/health` and the UI assets stay outside it.
//! - Every WebSocket upgrade checks its `Origin`: `/ws` and `/agents/ws`
//!   admit any loopback origin through `cross_site::origin_allowed`, and
//!   `/v1/realtime` applies a stricter same-origin check that requires a
//!   browser `Origin` to match the request's own authority. The
//!   cross-site guard stays the security boundary.
//! - Every unresolved wait that is dropped yields a cancelled frame,
//!   which the agent socket renders as `input_cancelled`.

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
