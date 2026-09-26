//! Server lifecycle tests: readiness, graceful shutdown under held connections, and port release.

use super::*;

use std::path::{Path, PathBuf};

use workshop_support::{AgentsConfig, GatewayConfig, ServerConfig};
fn test_config(bind: &str, state_dir: &Path) -> Config {
    Config {
        gateway: GatewayConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            bind: bind.to_string(),
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
        "the URL names the bound loopback address: {url}"
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

/// The `-wal` sidecar the workspace file's engine keeps beside `path`
/// while the file is open.
fn wal_of(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push("-wal");
    PathBuf::from(name)
}

/// Authors a workspace file at `file` granting `root` in a previous
/// "run", lets go of it, and points `state_dir`'s `last-workspace` at
/// it. Returns the granted path.
async fn point_at_authored_workspace(state_dir: &Path, file: &Path, root: &Path) -> PathBuf {
    let author = workshop_workspace::Workspace::new();
    let granted = author.grant(root).expect("grant the root");
    author.save_as(file).await.expect("save as creates");
    author.close_backing_for_test().await;
    std::fs::write(
        state_dir.join("last-workspace"),
        file.to_string_lossy().as_bytes(),
    )
    .expect("the pointer writes");
    granted
}

/// The boot wiring itself, not a re-implementation of it: `serve_thread`
/// must follow the `last-workspace` pointer before it signals readiness,
/// so the first request over the real listener already sees the reopened
/// file and its grants. Deleting the `reopen_last_workspace` call in
/// `serve_thread` fails this test and nothing else.
#[tokio::test]
async fn spawn_reopens_the_pointed_workspace_before_readiness() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let root = tempfile::TempDir::new().expect("tempdir");
    let file = home.path().join("mine.pfwork");
    let granted = point_at_authored_workspace(state_dir.path(), &file, root.path()).await;

    let server = spawn_with_grace(test_config("127.0.0.1:0", state_dir.path()), SHUTDOWN_GRACE)
        .expect("server spawns");
    let url = server.url().to_string();

    let response = reqwest::get(format!("{url}/workspace/file/current"))
        .await
        .expect("the workspace file endpoint answers once spawn returns");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let current: serde_json::Value = response.json().await.expect("the body is JSON");
    assert_eq!(current["path"], serde_json::json!(file));
    assert_eq!(current["name"], "mine");
    assert_eq!(
        current["grants"],
        serde_json::json!([{ "path": granted, "exists": true }]),
        "the file's grants are live on the first request after readiness"
    );

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
    let gateway = workshop_gateway::test_gateway::ValidatedGateway::spawn_in(
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

/// The listener binds before the pointed workspace reopens, so a taken
/// port fails boot without ever opening the file: nothing is left
/// holding it, and no `-wal` sidecar is stranded beside it.
#[tokio::test]
async fn a_bind_conflict_leaves_the_pointed_workspace_unopened() {
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").expect("bind blocker");
    let address = blocker.local_addr().expect("blocker address");
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let root = tempfile::TempDir::new().expect("tempdir");
    let file = home.path().join("mine.pfwork");
    point_at_authored_workspace(state_dir.path(), &file, root.path()).await;

    let config = test_config(&address.to_string(), state_dir.path());
    let error = spawn_with_grace(config, SHUTDOWN_GRACE).expect_err("a taken port must fail spawn");
    assert!(
        matches!(error, SpawnError::Io(_)),
        "expected Io, got {error:?}"
    );
    assert!(
        !wal_of(&file).exists(),
        "a failed bind opened the pointed workspace and stranded its wal sidecar"
    );
}

/// `spawn` refuses a non-loopback bind address before the server thread
/// starts: no state is composed, so the boot temp sweep never runs and
/// the pointed workspace file is never opened.
#[tokio::test]
async fn a_non_loopback_bind_fails_spawn_before_state_is_composed() {
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let home = tempfile::TempDir::new().expect("tempdir");
    let root = tempfile::TempDir::new().expect("tempdir");
    let file = home.path().join("mine.pfwork");
    point_at_authored_workspace(state_dir.path(), &file, root.path()).await;
    // Residue the boot sweep would remove if state were composed.
    let orphan = state_dir.path().join("workshop-state.json.42-7.pf-tmp");
    std::fs::write(&orphan, "partial").expect("the simulated crash residue writes");

    let config = test_config("0.0.0.0:0", state_dir.path());
    let error =
        spawn_with_grace(config, SHUTDOWN_GRACE).expect_err("a non-loopback bind must fail spawn");
    assert!(
        matches!(&error, SpawnError::Io(io) if io.kind() == std::io::ErrorKind::InvalidInput),
        "expected an InvalidInput Io error, got {error:?}"
    );
    assert!(
        orphan.exists(),
        "the boot temp sweep ran before the loopback check"
    );
    assert!(
        !wal_of(&file).exists(),
        "the pointed workspace was opened before the loopback check"
    );
}

/// The server may only ever bind to loopback: a wildcard or LAN address
/// would expose the workshop to other hosts. `reuse_bind` refuses those
/// before it creates a socket, so the error is `InvalidInput` rather than
/// a late bind failure.
#[tokio::test]
async fn non_loopback_binds_are_refused_with_invalid_input() {
    for address in ["0.0.0.0:0", "[::]:0", "192.168.1.10:0"] {
        let error = reuse_bind(address).expect_err("a non-loopback address must be refused");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "refusing {address} must be an InvalidInput error"
        );
    }
}

#[tokio::test]
async fn a_loopback_address_binds() {
    let listener = reuse_bind("127.0.0.1:0").expect("a loopback address binds");
    drop(listener);
}

/// A runner without IPv6 may fail an `[::1]` bind for a platform reason,
/// but the refusal itself must never be the loopback check, so the error
/// kind is anything but `InvalidInput`.
#[tokio::test]
async fn an_ipv6_loopback_bind_is_not_refused_with_invalid_input() {
    match reuse_bind("[::1]:0") {
        Ok(listener) => drop(listener),
        Err(error) => assert_ne!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "an IPv6 loopback bind must not be refused with InvalidInput"
        ),
    }
}
