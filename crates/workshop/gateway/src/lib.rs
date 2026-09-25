//! workshop-gateway - the gateway subsystem: the bearer-authenticated
//! HTTP client for the PromptForge gateway's OpenAI-compatible API, the
//! replaceable endpoint binding and discovery-file resolution, the
//! reachability heartbeat, the gateway progress subscriber, and the
//! workshop's run event log.
//!
//! ## Invariants
//!
//! - Tier: service; may depend on: `workshop-protocol`, `workshop-registry`,
//!   `workshop-support`. Read `AGENTS.md` before adding an import.
//! - Every file in this crate stays under 500 lines; split first, then
//!   edit.
//! - No axum type appears in this crate's public API: the domain code
//!   speaks `reqwest` statuses and raw bodies, and the server maps them
//!   to HTTP responses.
//! - A bearer key is never written to logs or `Debug` output.
//! - User-visible reporting flows through the registry's push facade, so
//!   this crate never names another subsystem's bus.
//! - The gateway's progress reaches this crate as the public wire type
//!   `gateway_api_types::Progress` alone: no progress machinery is
//!   shared with the gateway family.

pub mod gateway;
pub mod gateway_binding;
pub mod gateway_progress;
pub mod handles;
pub mod heartbeat;
pub mod resolve;
#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_gateway;

pub use gateway::{
    CacheEvent, CacheResponse, GatewayClient, GatewayError, GatewayResponse, ProgressStream,
    SsePayloadStream, SwitchProfileBody, SwitchResponse,
};
pub use gateway_binding::{
    GatewayBinding, GatewayPublicationError, GatewaySnapshot, GatewayUpdater,
};
pub use handles::{GatewayHandles, register, register_tasks};
pub use heartbeat::{GatewayHealth, Heartbeat};
pub use resolve::{GatewaySource, ResolveError, ResolvedGateway};
