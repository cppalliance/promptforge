//! What the router lets through and what it refuses.
//!
//! The auth matrix is asserted against the assembled router rather than against
//! [`require_bearer`] directly, because the claim being tested is where the
//! layer sits: `/mcp` is behind it and `/healthz` is not, and only the router
//! can show that.

use std::fs;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tempfile::TempDir;
use tower::ServiceExt;

use super::{HEALTHZ_PATH, MCP_PATH, SSE_KEEP_ALIVE, build_router, serve_http, streamable_config};
use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::config::{Config, Secret};
use crate::error::ServeError;
use crate::retrieval::Retrieval;
use crate::server::{PreparedTools, PromptForgeServer};

/// The shared bearer every fixture router is built with.
const TOKEN: &str = "shared-bearer";

/// A prompt that runs offline: one section whose Lua returns at once.
const ECHO: &str = "---\nname: echo\ndescription: Returns its argument\npromptforge: 1\n---\n\n\
# Test prompt\n\n## Main\n\n```lua\nreturn args\n```\n";

/// A router over a one-prompt catalog, and the directory that catalog reads.
fn router() -> (TempDir, axum::Router) {
    router_with(Secret::from(TOKEN.to_string()))
}

/// The same router, with the bearer layer built over `token` rather than over
/// the configured one. The layer takes the secret as an argument, so a token the
/// configuration would refuse can still be put behind it here.
fn router_with(token: Secret) -> (TempDir, axum::Router) {
    let (dir, config) = fixture(&format!("token = \"{TOKEN}\"\n"));
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(PreparedTools::new(&config.gateway).expect("prepare fixture live tools"));
    let server = PromptForgeServer::new(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    );
    (dir, build_router(server, Arc::new(token)))
}

/// A one-prompt configuration whose `[server]` table carries `server_lines`,
/// and the temporary directory its catalog reads.
fn fixture(server_lines: &str) -> (TempDir, Config) {
    let dir = tempfile::tempdir().expect("create a temporary prompts directory");
    fs::write(dir.path().join("echo.md"), ECHO).expect("write the fixture prompt");
    let config = Config::from_toml_str(&format!(
        "[server]\n{server_lines}\n\
         [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"gw\"\n\n\
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

#[tokio::test]
async fn a_call_with_no_authorization_header_is_refused() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(None))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
        "a refusal names the scheme the caller should have used"
    );
}

#[tokio::test]
async fn a_call_using_the_wrong_scheme_is_refused() {
    // The token is right; the scheme is not. Nothing but `Bearer` is accepted,
    // so a basic-auth client cannot smuggle the same secret through.
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some(&format!("Basic {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn an_empty_configured_token_authenticates_nobody() {
    // The configuration refuses an empty token, so this layer cannot be built
    // from a file; it is built with one directly to show the second defence
    // standing on its own. A caller presenting nothing is refused before the
    // comparison it would otherwise have matched.
    let (_dir, router) = router_with(Secret::from(String::new()));

    let missing = router
        .clone()
        .oneshot(initialize(None))
        .await
        .expect("the router answers");
    assert_eq!(
        missing.status(),
        StatusCode::UNAUTHORIZED,
        "no header never compares equal, whatever the configured token is"
    );

    let scheme = router
        .oneshot(initialize(Some("Basic ")))
        .await
        .expect("the router answers");
    assert_eq!(
        scheme.status(),
        StatusCode::UNAUTHORIZED,
        "a scheme that is not Bearer is refused before anything is compared"
    );
}

#[tokio::test]
async fn a_call_with_the_wrong_token_is_refused() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some("Bearer not-the-token")))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_call_with_the_right_token_reaches_the_mcp_endpoint() {
    let (_dir, router) = router();
    let response = router
        .oneshot(initialize(Some(&format!("Bearer {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert!(
        response.headers().contains_key("mcp-session-id"),
        "the handshake reached the transport and opened a session"
    );
}

#[tokio::test]
async fn a_rotated_token_refuses_the_next_request_on_an_established_session() {
    // The check is per HTTP request, not per MCP session, so possessing a
    // session id buys nothing once the token no longer matches.
    let (_dir, router) = router();
    let opened = router
        .clone()
        .oneshot(initialize(Some(&format!("Bearer {TOKEN}"))))
        .await
        .expect("the router answers");
    assert_eq!(opened.status(), StatusCode::OK);
    let session = opened
        .headers()
        .get("mcp-session-id")
        .expect("the handshake opened a session")
        .clone();

    let request = Request::builder()
        .method("POST")
        .uri(MCP_PATH)
        .header("host", "127.0.0.1")
        .header("accept", "application/json, text/event-stream")
        .header("content-type", "application/json")
        .header("authorization", "Bearer rotated-away")
        .header("mcp-session-id", session)
        .body(Body::from(
            serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }).to_string(),
        ))
        .expect("build the follow-up request");
    let response = router.oneshot(request).await.expect("the router answers");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn healthz_answers_without_a_token() {
    let (_dir, router) = router();
    let request = Request::builder()
        .uri(HEALTHZ_PATH)
        .body(Body::empty())
        .expect("build the liveness request");
    let response = router.oneshot(request).await.expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn serving_over_http_without_a_token_is_refused_by_name() {
    // The token is optional in the file because stdio never reads it. This
    // transport is the one that does, so it refuses before it binds, naming the
    // field the operator has to add rather than serving an unguarded `/mcp`.
    let (_dir, config) = fixture("bind = \"127.0.0.1:0\"\n");
    assert!(config.server.token.is_none());
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(PreparedTools::new(&config.gateway).expect("prepare fixture live tools"));

    let error = serve_http(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        Arc::new(Retrieval::idle()),
        tools,
    )
    .await
    .expect_err("http will not serve without a shared bearer");

    assert!(matches!(error, ServeError::MissingToken), "{error}");
    assert!(error.to_string().contains("[server].token"), "{error}");
}

#[test]
fn an_idle_stream_is_pinged_every_fifteen_seconds() {
    // A run reports progress on the stream its call opened; an idle proxy that
    // closed it would take the notifications with it.
    assert_eq!(streamable_config().sse_keep_alive, Some(SSE_KEEP_ALIVE));
    assert!(
        streamable_config().legacy_session_mode,
        "progress rides the session's stream, so sessions stay on"
    );
}
