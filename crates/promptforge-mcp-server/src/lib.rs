//! PromptForge MCP server.
//!
//! Runs PromptForge prompts for an agentic harness (Cursor, Claude Code). A
//! prompt is a command: it runs because a caller named it to `run_prompt`, so
//! no prompt is published as a tool of its own and `tools/list` is the same
//! fixed set of built-ins whatever the catalog holds. Execution happens here,
//! against the gateway, so a prompt is never an MCP prompt.
//!
//! [`Config`] parses the `prompts.toml` that names the bind address, the shared
//! token, the prompts directory, the gateway, and which prompts the harness sees;
//! [`Catalog::resolve`] turns that configuration and the prompts directory into
//! the set of prompts a harness may call, either refusing to start over an
//! incomplete result or keeping the failures visible as broken entries;
//! [`tool_definitions`] is what `tools/list` answers with; [`PromptForgeServer`]
//! runs a call to completion against the gateway and reports it as a
//! [`RunResult`]; [`McpObserver`] reports that run as it goes, so a client sees
//! a caption change rather than one silent multi-minute call; and
//! [`RunRegistry`] admits the run, hands the caller a `run_id` when it outlasts
//! the client's patience, and keeps the result collectable afterwards.
//! [`build_router`] puts that handler behind a shared bearer at `/mcp` with an
//! unauthenticated `/healthz` beside it, and [`serve_http`] and [`serve_stdio`]
//! are the two transports it is reached on. [`Watcher`] keeps the catalog
//! current while the server runs, so writing a prompt is an edit-and-call loop
//! rather than an edit-restart-call one: no client is notified and none needs
//! to be, since the tool list never moves and every call reads the catalog
//! fresh. [`Retrieval`] is what answers `need_prompt`: a plain-English
//! capability in, up to three candidate prompts out, rebuilt on the same swap
//! when a save changed what it ranks on.

mod catalog;
mod config;
mod error;
#[cfg(test)]
mod levels;
mod progress;
mod registry;
mod result;
mod retrieval;
mod server;
mod tools;
mod transport;
mod watch;

pub use crate::catalog::{Catalog, CatalogHandle, Entry, OnBroken};
pub use crate::config::{
    CatalogConfig, Config, GatewayConfig, PathsConfig, PromptConfig, Secret, ServerConfig,
};
pub use crate::error::{CatalogError, ConfigError, Fault, ServeError, WatchError};
pub use crate::progress::{McpObserver, ProgressPump};
pub use crate::registry::{RunRegistry, RunSlot};
pub use crate::result::{RunResult, RunStatus};
pub use crate::retrieval::{Candidate, Retrieval, Shortlist};
pub use crate::server::{PreparedTools, PromptForgeServer};
pub use crate::tools::{CHECK_RUN, LIST_PROMPTS, NEED_PROMPT, RUN_PROMPT, tool_definitions};
pub use crate::transport::{HEALTHZ_PATH, MCP_PATH, build_router, serve_http, serve_stdio};
pub use crate::watch::{Reload, Reloader, Watcher};
