//! PromptForge Workshop HTTP server.
//!
//! Holds the `workshop.toml` configuration, the PromptForge gateway client,
//! the session tape, and the axum router so `src/main.rs` stays a thin shell.
//! Start at [`Config::load`] for configuration, [`Tape`] for the session
//! tape, and [`router`] for the HTTP API; [`spawn`] runs the whole server
//! in-process on its own thread for embedding binaries.

mod app;
mod catalog;
mod chat_ws;
mod config;
mod gateway;
mod heartbeat;
mod provision;
mod segment;
mod serve;
mod status;
mod tape;
mod transcribe;
mod voice;
mod workspace;

pub use app::{AppError, AppState, DEFAULT_ADDR, router};
pub use config::{
    Config, ConfigError, DEFAULT_CONFIG_PATH, DEFAULT_GATEWAY_BASE_URL, DEFAULT_VOICE_INTERVAL_MS,
    DEFAULT_VOICE_WINDOW_SECONDS, GatewayConfig, ServerConfig, TapeConfig, VoiceConfig,
};
pub use gateway::{
    CacheEvent, CacheResponse, ChatRequest, ChatStream, GatewayClient, GatewayError,
    GatewayResponse, SsePayloadStream,
};
pub use serve::{ServerHandle, SpawnError, spawn};
pub use tape::{Tape, TapeError, TapeEvent};
pub use transcribe::TranscribeError;
