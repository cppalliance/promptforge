//! PromptForge MCP server.
//!
//! Publishes PromptForge prompts to an agentic harness (Cursor, Claude Code) as
//! callable MCP tools. Some prompts get their own entry in `tools/list`; the
//! rest are reachable through built-in listing and retrieval tools. Execution
//! happens here, against the gateway, so a prompt is always a tool and never an
//! MCP prompt.
//!
//! What exists so far is the crate's configuration, its catalog, the tool
//! surface those two produce, the handler that answers a call, and the registry
//! that outlives one. [`Config`]
//! parses the `prompts.toml` that names the bind address, the shared token, the
//! prompts directory, the gateway, and which prompts the harness sees;
//! [`Catalog::resolve`] turns that configuration and the prompts directory into
//! the set of prompts a harness may call, either refusing to start over an
//! incomplete result or keeping the failures visible as broken entries;
//! [`tool_definitions`] turns the resolved catalog into what `tools/list`
//! answers with; [`PromptForgeServer`] runs a call to completion against the
//! gateway and reports it as a [`RunResult`]; [`McpObserver`] reports that run
//! as it goes, so a client sees a caption change rather than one silent
//! multi-minute call; and [`RunRegistry`] admits the run, hands the caller a
//! `run_id` when it outlasts the client's patience, and keeps the result
//! collectable afterwards. [`build_router`] puts that handler behind a shared
//! bearer at `/mcp` with an unauthenticated `/healthz` beside it, and
//! [`serve_http`] and [`serve_stdio`] are the two transports it is reached on.
//! What comes next is the watcher that re-reads a prompt on save.

mod catalog;
mod config;
mod error;
mod progress;
mod registry;
mod result;
mod server;
mod tools;
mod transport;

pub use crate::catalog::{Catalog, CatalogHandle, Entry, OnBroken};
pub use crate::config::{
    CatalogConfig, Config, Expose, GatewayConfig, PathsConfig, PromptConfig, Secret, ServerConfig,
};
pub use crate::error::{CatalogError, ConfigError, Fault, ServeError};
pub use crate::progress::{McpObserver, ProgressPump};
pub use crate::registry::{RunRegistry, RunSlot};
pub use crate::result::{RunResult, RunStatus};
pub use crate::server::PromptForgeServer;
pub use crate::tools::{CHECK_RUN, LIST_PROMPTS, NEED_PROMPT, RUN_PROMPT, tool_definitions};
pub use crate::transport::{HEALTHZ_PATH, MCP_PATH, build_router, serve_http, serve_stdio};
