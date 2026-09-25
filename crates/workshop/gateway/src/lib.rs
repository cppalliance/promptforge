//! workshop-gateway - the gateway subsystem: the bearer-authenticated
//! HTTP client for the PromptForge gateway's OpenAI-compatible API, the
//! replaceable endpoint binding and discovery-file resolution, the
//! reachability heartbeat, and the gateway progress subscriber.
//!
//! ## Invariants
//!
//! - Tier: service; may depend on: `workshop-protocol`, `workshop-registry`,
//!   `workshop-support`. Read the repository-root `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - No axum type appears in this crate's public API: the domain code
//!   speaks `reqwest` statuses and raw bodies, and the server maps them
//!   to HTTP responses.
//! - A bearer key is never written to logs or `Debug` output.
//! - User-visible reporting flows through the registry's push facade, and
//!   the gateway drives the menu through its
//!   [`MenuPush`](workshop_registry::MenuPush) face, so this crate never
//!   names another subsystem's bus.
//! - The gateway's progress reaches this crate as the public wire type
//!   `gateway_api_types::Progress` alone: no progress machinery is
//!   shared with the gateway family.

mod binding;
mod client;
mod handles;
mod heartbeat;
mod progress;
mod refresh;
mod resolve;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_gateway;

pub use binding::{GatewayBinding, GatewayPublicationError, GatewaySnapshot, GatewayUpdater};
pub use client::{
    ForwardedResponse, GatewayClient, GatewayError, GatewayRealtimeSocket, GatewayResponse,
    REQUEST_TIMEOUT, SwitchProfileBody, SwitchResponse,
};
pub use handles::{GatewayHandles, GatewayTaskRegistrations, register, register_tasks};
pub use heartbeat::{GatewayHealth, Heartbeat, join_status, spawn as spawn_heartbeat};
pub use refresh::{refresh_catalog, refresh_profiles};
pub use resolve::{GatewaySource, ResolveError, ResolvedGateway, report, resolve};
