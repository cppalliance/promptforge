//! PromptForge MCP server.
//!
//! Publishes PromptForge prompts to an agentic harness (Cursor, Claude Code) as
//! callable MCP tools. Some prompts get their own entry in `tools/list`; the
//! rest are reachable through built-in listing and retrieval tools. Execution
//! happens here, against the gateway, so a prompt is always a tool and never an
//! MCP prompt.
//!
//! What exists so far is the crate's configuration and its catalog. [`Config`]
//! parses the `prompts.toml` that names the bind address, the shared token, the
//! prompts directory, the gateway, and which prompts the harness sees;
//! [`Catalog::resolve`] turns that configuration and the prompts directory into
//! the set of prompts a harness may call, either refusing to start over an
//! incomplete result or keeping the failures visible as broken entries. The MCP
//! surface and the transports come next.

mod catalog;
mod config;
mod error;

pub use crate::catalog::{Catalog, CatalogHandle, Entry, OnBroken};
pub use crate::config::{
    CatalogConfig, Config, Expose, GatewayConfig, PathsConfig, PromptConfig, Secret, ServerConfig,
};
pub use crate::error::{CatalogError, ConfigError, Fault};
