//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! the session tape, and the axum router so `src/main.rs` stays a thin shell.
//! Start at [`Config::load`] for configuration, [`Tape`] for the session
//! tape, and [`router`] for the HTTP API; [`spawn`] runs the whole server
//! in-process on its own thread for embedding binaries.

mod app;
mod assets;
mod atomic;
mod backoff;
mod catalog;
mod config;
mod cross_site;
mod deadline;
mod error;
mod gateway;
mod heartbeat;
mod menu;
mod protocol;
mod provision;
mod push;
mod relay;
mod routes;
mod serve;
mod session;
mod status;
mod tape;
mod voice;
mod workspace;

// The release artifact verifier lives outside src/ so build.rs shares it
// through the same `#[path]` mechanism; included here only to run its
// tests under `cargo test`.
#[cfg(test)]
#[path = "../build/manifest.rs"]
mod build_manifest;

/// Crate-internal test seams, re-exported to the integration-test binary.
/// The socket behavior tests drive the status, catalog, and menu buses,
/// the health flag, the backoff, and the heartbeat directly, so those
/// types surface here; Rust visibility cannot be feature-gated, so these
/// re-exports are present in every build and hidden from the docs. The
/// fixture helpers with test-only dependencies stay behind the
/// `test-fixtures` feature, which the crate's own dev-dependency enables
/// for every test build while production builds do not.
#[doc(hidden)]
pub mod fixtures {
    pub use crate::backoff::ReconnectBackoff;
    pub use crate::catalog::CatalogBus;
    pub use crate::heartbeat::{GatewayHealth, Heartbeat, spawn as spawn_heartbeat};
    pub use crate::menu::{MenuBus, MenuRefusal};
    pub use crate::protocol::{Activity, Progress, Severity, StatusBarUpdate};
    pub use crate::push::Push;
    pub use crate::status::StatusBus;

    #[cfg(feature = "test-fixtures")]
    pub use promptforge_transcribe::fixtures::{
        fixture_dir, jfk_samples, model_path, require_model,
    };

    #[cfg(feature = "test-fixtures")]
    pub use crate::app::fixtures::spawn_gateway;
}

pub use promptforge_transcribe::TranscribeError;

pub use app::{AppState, DEFAULT_ADDR, StateError, router};
pub use config::{
    Config, ConfigError, DEFAULT_CONFIG_PATH, DEFAULT_GATEWAY_BASE_URL, DEFAULT_VOICE_INTERVAL_MS,
    DEFAULT_VOICE_WINDOW_SECONDS, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig,
};
pub use gateway::{
    CacheEvent, CacheResponse, ChatStream, GatewayClient, GatewayError, GatewayResponse,
    SsePayloadStream, SwitchEvent, SwitchEventStream, SwitchResponse, switch_events,
};
pub use protocol::ChatRequest;
pub use serve::{ServerHandle, SpawnError, Termination, spawn};
pub use tape::{Tape, TapeError, TapeEvent};
