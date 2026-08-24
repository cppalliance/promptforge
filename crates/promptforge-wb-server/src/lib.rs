//! PromptForge Workbench HTTP server.
//!
//! Holds the `workbench.toml` configuration, the PromptForge gateway client,
//! the session tape, and the axum router so `src/main.rs` stays a thin shell.
//! Start at [`Config::load`] for configuration, [`Tape`] for the session
//! tape, and [`router`] for the HTTP API; [`run`] binds and serves a built
//! [`AppState`].

mod app;
mod config;
mod gateway;
mod segment;
mod tape;
mod transcribe;
mod voice;

pub use app::{AppError, AppState, DEFAULT_ADDR, router, run};
pub use config::{
    Config, ConfigError, DEFAULT_CONFIG_PATH, DEFAULT_VOICE_INTERVAL_MS,
    DEFAULT_VOICE_WINDOW_SECONDS, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig,
};
pub use gateway::{
    ChatRequest, ChatStream, GatewayClient, GatewayError, GatewayResponse, SsePayloadStream,
};
pub use tape::{Tape, TapeError, TapeEvent};
pub use transcribe::TranscribeError;
