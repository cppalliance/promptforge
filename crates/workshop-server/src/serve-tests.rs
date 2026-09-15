use super::*;

use std::path::Path;

use workshop_support::{AgentsConfig, GatewayConfig, ServerConfig};
fn test_config(bind: &str, state_dir: &Path) -> Config {
    Config {
        gateway: GatewayConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            bind: bind.to_string(),
            open_browser: false,
            state_dir: state_dir.to_path_buf(),
        },
        agents: AgentsConfig::default(),
    }
}

#[tokio::test]
async fn readiness_means_the_health_endpoint_answers() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");
    let url = server.url().to_string();
    assert!(
        url.starts_with("http://127.0.0.1:"),
        "the URL carries the bound loopback address: {url}"
    );

    let response = reqwest::get(format!("{url}/health"))
        .await
        .expect("the health endpoint answers once spawn returns");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body = response.text().await.expect("the health body reads");
    assert_eq!(body, r#"{"status":"serving"}"#);

    server.shutdown().expect("graceful shutdown succeeds");
}

/// The test config points the gateway at port 1, which never listens:
/// the server must still boot and serve - the UI and its own health
/// endpoint do not depend on the gateway, and the heartbeat reports
/// the outage instead of failing startup.
#[tokio::test]
async fn the_server_boots_and_serves_the_ui_with_an_unreachable_gateway() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");
    let url = server.url().to_string();

    let health = reqwest::get(format!("{url}/health"))
        .await
        .expect("the health endpoint answers");
    assert_eq!(health.status(), reqwest::StatusCode::OK);
    // Under the headless feature the asset layer is a no-op by design, so
    // the UI index answers 404; the boot-and-health behavior above is
    // what this test proves in that configuration.
    #[cfg(not(feature = "headless"))]
    {
        let index = reqwest::get(format!("{url}/"))
            .await
            .expect("the UI answers");
        assert_eq!(index.status(), reqwest::StatusCode::OK);
    }

    server.shutdown().expect("graceful shutdown succeeds");
}

/// A grace window short enough that the forced path proves itself in
/// milliseconds instead of stalling the suite.
const TEST_GRACE: Duration = Duration::from_millis(200);

/// Connects a WebSocket to the workshop socket and returns it for the
/// caller to hold open.
async fn hold_ws_open(
    url: &str,
) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
    let address = url.strip_prefix("http://").expect("the URL is http");
    let (socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the chat socket connects");
    socket
}

/// Opens a raw connection and wedges it mid-request: the head promises
/// a body that never fully arrives, so the handler waits on the body,
/// no response begins, and the connection holds axum's graceful drain
/// open until torn down. The head must pass the cross-site guard - a
/// loopback `Host` and a JSON content type - or the guard answers 403
/// without ever polling the body and nothing wedges.
async fn wedge_http_connection(url: &str) -> tokio::net::TcpStream {
    use tokio::io::AsyncWriteExt as _;

    let address = url.strip_prefix("http://").expect("the URL is http");
    let mut wedged = tokio::net::TcpStream::connect(address)
        .await
        .expect("the raw connection opens");
    wedged
        .write_all(
            b"POST /workspace/grant HTTP/1.1\r\nhost: 127.0.0.1\r\n\
              content-type: application/json\r\ncontent-length: 64\r\n\r\n{",
        )
        .await
        .expect("the wedged request head sends");
    // Give the accept loop and the handler a beat to pick the request
    // up, so the connection is in-flight before shutdown begins.
    tokio::time::sleep(Duration::from_millis(50)).await;
    wedged
}

/// The regression this step exists to prevent: a client that never
/// closes its WebSocket must not park shutdown forever. The upgrade
/// detaches the session from axum's graceful drain, so today this stop
/// is even graceful; the assertion pins only the bound, which the
/// watchdog keeps true however axum's connection tracking evolves.
#[tokio::test]
async fn a_held_websocket_does_not_block_shutdown_past_the_grace_window() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
        .expect("server spawns");
    let _held = hold_ws_open(server.url()).await;

    let begun = std::time::Instant::now();
    server
        .shutdown()
        .expect("shutdown returns despite the held socket");
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "shutdown must return shortly after the grace window, took {:?}",
        begun.elapsed()
    );
}

/// A connection wedged mid-request does hold the graceful drain open,
/// so the watchdog must abandon the wait at the window and report the
/// stop as forced.
#[tokio::test]
async fn a_wedged_http_connection_is_forced_out_at_the_grace_window() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
        .expect("server spawns");
    let _wedged = wedge_http_connection(server.url()).await;

    let begun = std::time::Instant::now();
    let outcome = server
        .shutdown()
        .expect("shutdown returns despite the wedged connection");
    assert_eq!(
        outcome,
        Termination::Forced,
        "an in-flight request cannot drain; the watchdog must force the stop"
    );
    assert!(
        begun.elapsed() < Duration::from_secs(3),
        "shutdown must return shortly after the grace window, took {:?}",
        begun.elapsed()
    );
}

#[tokio::test]
async fn an_idle_shutdown_completes_gracefully_without_spending_the_window() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");

    let begun = std::time::Instant::now();
    let outcome = server.shutdown().expect("graceful shutdown succeeds");
    assert_eq!(
        outcome,
        Termination::Graceful,
        "nothing held the drain open"
    );
    assert!(
        begun.elapsed() < SHUTDOWN_GRACE,
        "an idle server must stop before the watchdog matters, took {:?}",
        begun.elapsed()
    );
}

#[test]
fn server_shutdown_permanently_closes_every_host_updater_clone() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");
    let updater = server.gateway_updater();
    let clone = updater.clone();

    server.shutdown().expect("graceful shutdown succeeds");

    assert!(updater.publication_closed());
    assert!(
        clone.publication_closed(),
        "application teardown closes the shared publication state for every clone"
    );
}

#[test]
fn server_handle_reports_the_identity_initially_published_into_state() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let gateway = crate::test_gateway::ValidatedGateway::spawn_in(
        "initial-key",
        "fixtures::validated_gateway_fixture_process",
    );
    let identity = gateway.validate("initial-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let resolved = ResolvedGateway::from_validated(identity.clone());
    let server = spawn_inner(
        test_config("127.0.0.1:0", dir.path()),
        Some(resolved),
        SHUTDOWN_GRACE,
    )
    .expect("server spawns");

    assert!(
        server
            .initial_gateway_identity()
            .is_some_and(|published| published.same_boot(&identity)),
        "the host can authenticate which launch candidate entered server state"
    );
    server.shutdown().expect("graceful shutdown succeeds");
}

/// The stopped barrier: when `shutdown` returns, the server is really
/// gone - nothing listens on its address - even when the stop was
/// forced.
#[tokio::test]
async fn the_stopped_barrier_reports_after_serving_has_ended() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), TEST_GRACE)
        .expect("server spawns");
    let address = server
        .url()
        .strip_prefix("http://")
        .expect("the URL is http")
        .to_string();
    let _wedged = wedge_http_connection(server.url()).await;

    let outcome = server.shutdown().expect("shutdown returns");
    assert_eq!(
        outcome,
        Termination::Forced,
        "the wedged connection forces the stop"
    );
    let refused = tokio::net::TcpStream::connect(&address).await;
    assert!(
        refused.is_err(),
        "the stopped barrier resolves only after serving has ended, yet {address} accepted"
    );
}

#[tokio::test]
async fn shutdown_releases_the_bound_port() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let server = spawn_with_grace(test_config("127.0.0.1:0", dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");
    let address = server
        .url()
        .strip_prefix("http://")
        .expect("the URL is http")
        .to_string();
    server.shutdown().expect("graceful shutdown succeeds");

    let listener = tokio::net::TcpListener::bind(&address)
        .await
        .expect("the port is free after shutdown");
    drop(listener);
}

#[test]
fn a_bind_conflict_fails_spawn_with_io_error() {
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
    let address = blocker.local_addr().expect("blocker address");
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = test_config(&address.to_string(), dir.path());
    let error = spawn_with_grace(config, SHUTDOWN_GRACE).expect_err("a taken port must fail spawn");
    assert!(
        matches!(error, SpawnError::Io(_)),
        "expected Io, got {error:?}"
    );
}
