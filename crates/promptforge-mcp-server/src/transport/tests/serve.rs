//! `Host` validation, allowed-host resolution, the keep-alive interval, and the
//! serve-and-shutdown paths for both transports.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::catalog::{Catalog, CatalogHandle, OnBroken};
use crate::error::ServeErrorKind;
use crate::server::PreparedTools;
use crate::transport::{
    SSE_KEEP_ALIVE, resolve_allowed_hosts, serve_http, serve_stdio_on, streamable_config,
};

use super::{fixture, initialize_host, router_hosts, server_fixture};

#[tokio::test]
async fn serving_over_http_without_a_token_is_refused_by_name() {
    // The token is optional in the file because stdio never reads it. This
    // transport is the one that does, so it refuses before it binds, naming the
    // field the operator has to add rather than serving an unguarded `/mcp`.
    let (_dir, config) = fixture("bind = \"127.0.0.1:0\"\n");
    assert!(config.server.token.is_none());
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture live tools"),
    );

    let error = serve_http(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
        std::future::pending::<()>(),
    )
    .await
    .expect_err("http will not serve without a shared bearer");

    assert_eq!(error.kind(), ServeErrorKind::MissingToken, "{error}");
    assert!(error.to_string().contains("[server].token"), "{error}");
}

#[test]
fn an_idle_stream_is_pinged_every_fifteen_seconds() {
    // A run reports progress on the stream its call opened; an idle proxy that
    // closed it would take the notifications with it. The value is asserted
    // independently of the constant, so a change to either is a test failure the
    // other must be reconciled with rather than a change that moves silently.
    assert_eq!(SSE_KEEP_ALIVE, std::time::Duration::from_secs(15));
    assert_eq!(
        streamable_config(CancellationToken::new(), Vec::new()).sse_keep_alive,
        Some(std::time::Duration::from_secs(15))
    );
    assert!(
        streamable_config(CancellationToken::new(), Vec::new()).legacy_session_mode,
        "progress rides the session's stream, so sessions stay on"
    );
}

#[test]
fn an_empty_allowed_hosts_keeps_the_loopback_default_on_a_loopback_bind() {
    let bind = "127.0.0.1:9310".parse().expect("a loopback bind");
    let hosts = resolve_allowed_hosts(bind, &[]).expect("a loopback bind needs no enumeration");
    assert_eq!(hosts, ["localhost", "127.0.0.1", "::1"]);
}

#[test]
fn a_non_loopback_bind_with_no_allowed_hosts_is_refused() {
    let bind = "0.0.0.0:8080".parse().expect("a wildcard bind");
    let err = resolve_allowed_hosts(bind, &[])
        .expect_err("a non-loopback bind with no enumerated hosts is a contradiction");
    assert_eq!(err.kind(), ServeErrorKind::AllowedHosts);
    assert!(err.to_string().contains("allowed_hosts"), "{err}");
}

#[test]
fn an_explicit_allowed_hosts_list_is_honoured_on_any_bind() {
    let bind = "0.0.0.0:8080".parse().expect("a wildcard bind");
    let configured = ["example.com".to_string(), "example.com:8080".to_string()];
    let hosts = resolve_allowed_hosts(bind, &configured).expect("an enumerated list is accepted");
    assert_eq!(hosts, configured);
}

#[tokio::test]
async fn a_request_whose_host_is_not_allowed_is_refused() {
    // The bearer is right, so the request clears the auth layer and reaches the
    // transport's Host validation: an authority the operator did not enumerate
    // is the DNS-rebinding case the allow-list exists to stop.
    let (_dir, router) = router_hosts(vec!["example.com".to_string()]);
    let response = router
        .oneshot(initialize_host("evil.example.net"))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_request_using_an_allowed_dns_host_reaches_the_endpoint() {
    // The wildcard-bind case the finding calls out: an enumerated DNS authority
    // is reachable where the loopback default would have rejected it.
    let (_dir, router) = router_hosts(vec!["example.com".to_string()]);
    let response = router
        .oneshot(initialize_host("example.com"))
        .await
        .expect("the router answers");
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn http_serves_and_then_shuts_down_cleanly() {
    // Bind an ephemeral port, hand serve_http a shutdown it can trip, and prove
    // the accept loop drains and returns Ok once the signal fires rather than
    // running until the test is torn down.
    let (_dir, config) = fixture("bind = \"127.0.0.1:0\"\ntoken = \"shared-bearer\"\n");
    let catalog =
        Catalog::resolve(&config, OnBroken::Reject).expect("the fixture catalog resolves");
    let tools = Arc::new(
        PreparedTools::new(
            &config.gateway,
            promptforge_core::model::ModelCatalog::empty(),
        )
        .expect("prepare fixture live tools"),
    );
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(serve_http(
        Arc::new(config),
        Arc::new(CatalogHandle::new(catalog)),
        tools,
        async move {
            let _ = rx.await;
        },
    ));
    // Let it reach the accept loop before signalling, so the shutdown exercises
    // a running server rather than racing the bind.
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(()).expect("the server is still serving");
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("serve_http returns promptly after the shutdown signal")
        .expect("the serve task does not panic");
    assert!(
        outcome.is_ok(),
        "a clean shutdown is not an error: {outcome:?}"
    );
}

#[tokio::test]
async fn stdio_serves_and_then_shuts_down_cleanly() {
    // Drive a stdio session over an in-memory pipe with a peer that connects
    // but never sends `initialize`, so the session is parked in its handshake.
    // Tripping the shutdown must still return Ok promptly rather than hang,
    // which is the case a serve that only cancelled an established session
    // would miss.
    let (_dir, _config, server) = server_fixture("token = \"shared-bearer\"\n");
    let (client, server_io) = tokio::io::duplex(4096);
    let (read, write) = tokio::io::split(server_io);
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(serve_stdio_on(server, read, write, async move {
        let _ = rx.await;
    }));
    tokio::time::sleep(Duration::from_millis(100)).await;
    tx.send(()).expect("the session is still running");
    let outcome = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("serve_stdio_on returns promptly after the shutdown signal")
        .expect("the serve task does not panic");
    assert!(
        outcome.is_ok(),
        "a clean shutdown is not an error: {outcome:?}"
    );
    // Held to the end so the transport's write half is not closed early, which
    // would end the session on its own and mask the shutdown path.
    drop(client);
}
