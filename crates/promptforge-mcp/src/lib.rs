//! PromptForge MCP server.
//!
//! Publishes PromptForge prompts to an agentic harness (Cursor, Claude Code) as
//! callable MCP tools. Some prompts get their own entry in `tools/list`; the
//! rest are reachable through built-in listing and retrieval tools. Execution
//! happens here, against the gateway, so a prompt is always a tool and never an
//! MCP prompt.
//!
//! What exists so far is the crate's configuration: [`Config`] parses the
//! `prompts.toml` that names the bind address, the shared token, the prompts
//! directory, the gateway, and which prompts the harness sees. The catalog, the
//! MCP surface, and the transports come next.

mod config;
mod error;

pub use crate::config::{
    CatalogConfig, Config, Expose, GatewayConfig, PathsConfig, PromptConfig, Secret, ServerConfig,
};
pub use crate::error::ConfigError;
