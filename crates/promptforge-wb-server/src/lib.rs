//! PromptForge Workbench HTTP server.
//!
//! Holds the `workbench.toml` configuration, the PromptForge gateway client,
//! and the axum router so `src/main.rs` stays a thin shell. Start at
//! [`Config::load`] for configuration and [`router`] for the HTTP API;
//! [`run`] binds and serves a built [`AppState`].

mod app;
mod config;
mod gateway;

pub use app::{AppState, DEFAULT_ADDR, router, run};
pub use config::{
    Config, ConfigError, DEFAULT_CONFIG_PATH, GatewayConfig, ServerConfig, TapeConfig,
};
pub use gateway::{ChatRequest, GatewayClient, GatewayError, GatewayResponse};
