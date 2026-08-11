//! What the router lets through and what it refuses.
//!
//! The auth matrix is asserted against the assembled router rather than against
//! [`require_bearer`] directly, because the claim being tested is where the
//! layer sits: `/mcp` is behind it and `/healthz` is not, and only the router
//! can show that.
//!
//! The bearer matrix is in [`auth`]; `Host` validation, allowed-host
//! resolution, keep-alive, and the serve-and-shutdown paths are in [`serve`].
//! Both share the fixtures below.

mod auth;
mod serve;

use std::fs;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use super::{MCP_PATH, build_router};
use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::{Config, Secret};
use crate::server::{PreparedTools, PromptForgeServer};

/// The shared bearer every fixture router is built with.
const TOKEN: &str = "shared-bearer";

/// A prompt that runs offline: one section whose Lua returns at once.
const ECHO: &str = "---\nname: echo\ndescription: Returns its argument\npromptforge: 1\n---\n\n\
# Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n";

/// A router over a one-prompt catalog, and the directory that catalog reads.
fn router() -> (TempDir, axum::Router) {
    router_with(Secret::try_from(TOKEN.to_string()).expect("the fixture token is non-blank"))
}

/// The same router, with the bearer layer built over `token` rather than over
/// the configured one. The layer takes the secret as an argument, so a token the
/// configuration would refuse can still be put behind it here.
fn router_with(token: Secret) -> (TempDir, axum::Router) {
    let (dir, config) = fixture(&format!("token = \"{TOKEN}\"\n"));
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture live tools"),
    );
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );
    (
        dir,
        build_router(
            server,
            Arc::new(token),
            CancellationToken::new(),
            vec!["127.0.0.1".to_string()],
        ),
    )
}

/// A server built over a `[server]`-configured fixture, plus the directory its
/// catalog reads and the configuration itself. The serve tests need the pieces
/// `router_with` folds into a router, so they can hand them to the serving
/// functions directly.
fn server_fixture(server_lines: &str) -> (TempDir, Arc<Config>, PromptForgeServer) {
    let (dir, config) = fixture(server_lines);
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture live tools"),
    );
    let config = Arc::new(config);
    let server = PromptForgeServer::new(
        Arc::clone(&config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );
    (dir, config, server)
}

/// A one-prompt configuration whose `[server]` table carries `server_lines`,
/// and the temporary directory its catalog reads.
fn fixture(server_lines: &str) -> (TempDir, Config) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    fs::write(dir.path().join("echo.md"), ECHO).expect("write the fixture prompt");
    let config = Config::from_toml_str(&format!(
        "[server]\n{server_lines}\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\nkey = \"gw\"\n\n\
         [paths]\nprompts = '{}'\n\n\
         [catalog]\ninclude = [\"*.md\"]\n",
        dir.path().display()
    ))
    .expect("the fixture configuration parses");
    (dir, config)
}

/// An `initialize` POST shaped the way the streamable-HTTP transport requires,
/// carrying whatever `authorization` header value the caller supplies.
fn initialize(authorization: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        // The transport refuses a request whose `Host` is not a loopback name,
        // and a synthesized request carries no authority to fall back on.
        .header("host", "127.0.0.1")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json");
    if let Some(value) = authorization {
        builder = builder.header("authorization", value);
    }
    builder
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" },
                },
            })
            .to_string(),
        ))
        .expect("build the initialize request")
}

/// A router whose transport validates the `Host` header against `allowed_hosts`.
fn router_hosts(allowed_hosts: Vec<String>) -> (TempDir, axum::Router) {
    let (dir, config) = fixture(&format!("token = \"{TOKEN}\"\n"));
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture live tools"),
    );
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
    );
    (
        dir,
        build_router(
            server,
            Arc::new(Secret::try_from(TOKEN.to_string()).expect("the fixture token is non-blank")),
            CancellationToken::new(),
            allowed_hosts,
        ),
    )
}

/// An authorized `initialize` POST carrying `host` as its `Host` header.
fn initialize_host(host: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header("host", host)
        .header("authorization", format!("Bearer {TOKEN}"))
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "test", "version": "0" },
                },
            })
            .to_string(),
        ))
        .expect("build the initialize request")
}
