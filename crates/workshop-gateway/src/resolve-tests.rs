use super::*;

use std::io::{Read, Write as _};
use std::net::TcpListener;

use shared_sidecar::GatewayDiscoveryFile;

/// The test process's own image name, so the probe's pid and image
/// checks pass and the test reaches the liveness probes.
fn own_image_name() -> String {
    std::env::current_exe()
        .expect("current exe")
        .file_name()
        .expect("the exe has a file name")
        .to_string_lossy()
        .into_owned()
}

/// A probe running the real liveness gauntlet against the test
/// binary's own image.
fn probe_own_image(run_dir: &Path) -> Result<Resolution, SidecarError> {
    shared_sidecar::resolve_for_test(run_dir, &own_image_name())
}

/// A gateway discovery file pointing at the test process itself.
fn live_file(port: u16, api_key: &str) -> GatewayDiscoveryFile {
    GatewayDiscoveryFile {
        port,
        api_key: api_key.to_owned(),
        pid: std::process::id(),
        epoch: 1_757_000_000,
        version: "0.2.0".to_owned(),
        started_at: "2026-09-03T12:00:00Z".to_owned(),
    }
}

/// A pid guaranteed dead: a short-lived child, reaped and dropped so
/// no handle keeps the process object alive.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a short-lived child");
    let pid = child.id();
    child.wait().expect("the child exits");
    drop(child);
    pid
}

/// A fixture gateway: answers `GET /health` with 200 and the key
/// probe with 200 only when the bearer matches `expected_key`.
fn fixture_gateway(expected_key: &'static str) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("fixture address").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            for _ in 0..2 {
                let mut buffer = [0u8; 1024];
                let Ok(read) = stream.read(&mut buffer) else {
                    break;
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let accepted = request.starts_with("GET /health ")
                    || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"));
                let response = if accepted {
                    &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                } else {
                    &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                };
                if stream.write_all(response).is_err() {
                    break;
                }
            }
        }
    });
    port
}

/// An explicit gateway config: a LAN URL, never the built-in default.
fn explicit_config() -> GatewayConfig {
    GatewayConfig {
        base_url: "http://gateway.lan:9999".to_owned(),
        api_key: "config-key".to_owned(),
    }
}

#[test]
fn a_live_gateway_discovery_file_wins_over_explicit_config() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let gateway = crate::test_gateway::ValidatedGateway::spawn("file-key");
    let port = gateway.port();
    gateway
        .gateway_discovery_file("file-key", 1_757_000_000, "2026-09-03T12:00:00Z")
        .write_to(dir.path())
        .expect("write");

    let resolved =
        resolve_with(Some(dir.path()), &explicit_config(), probe).expect("a live file resolves");
    assert_eq!(resolved.source(), GatewaySource::GatewayDiscoveryFile);
    assert_eq!(resolved.base_url(), format!("http://127.0.0.1:{port}"));
    assert_eq!(resolved.api_key(), "file-key");
    assert_eq!(
        resolved.identity().map(ValidatedConnection::port),
        Some(port),
        "the winning sidecar retains its validated identity for the initial snapshot"
    );
    assert_eq!(resolved.stale(), None);
}

#[test]
fn a_stale_file_is_cleaned_and_explicit_config_wins() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = GatewayDiscoveryFile {
        pid: dead_pid(),
        ..live_file(1, "k")
    };
    file.write_to(dir.path()).expect("write");

    let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
        .expect("explicit config is the fallback");
    assert_eq!(resolved.source(), GatewaySource::Config);
    assert_eq!(resolved.base_url(), "http://gateway.lan:9999");
    assert_eq!(resolved.api_key(), "config-key");
    assert_eq!(resolved.stale(), Some(StaleReason::ProcessDead));
    assert!(
        !shared_sidecar::gateway_discovery_file_path(dir.path()).exists(),
        "the stale file was removed"
    );
}

#[test]
fn a_wrong_key_is_reported_distinctly_from_a_dead_pid() {
    // Wrong key: the pid, image, and health checks pass; the key
    // probe rejects.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let port = fixture_gateway("right");
    live_file(port, "wrong")
        .write_to(dir.path())
        .expect("write");
    let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
        .expect("explicit config is the fallback");
    assert_eq!(resolved.stale(), Some(StaleReason::KeyRejected));

    // Dead pid: the process check fails before any probe runs.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = GatewayDiscoveryFile {
        pid: dead_pid(),
        ..live_file(1, "k")
    };
    file.write_to(dir.path()).expect("write");
    let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
        .expect("explicit config is the fallback");
    assert_eq!(resolved.stale(), Some(StaleReason::ProcessDead));
}

#[test]
fn no_file_and_no_explicit_config_is_the_plain_error() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = GatewayConfig {
        base_url: String::new(),
        api_key: String::new(),
    };
    let error = resolve_with(Some(dir.path()), &config, probe_own_image)
        .expect_err("an empty base_url is not explicit config");
    assert!(
        error
            .to_string()
            .contains("no gateway configured or running"),
        "the error says it plainly: {error}"
    );
    assert_eq!(error.stale(), None, "no file existed to condemn");
}

#[test]
fn an_explicitly_configured_default_url_is_honored() {
    // The well-known default URL written by hand is explicit config:
    // it names a gateway discovery cannot see (an SSH-tunneled remote,
    // a gateway whose discovery-file write failed), so it must
    // resolve, not read as an unset value.
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = GatewayConfig {
        base_url: "http://127.0.0.1:8081".to_owned(),
        api_key: "config-key".to_owned(),
    };
    let resolved = resolve_with(Some(dir.path()), &config, probe_own_image)
        .expect("an explicitly configured URL resolves");
    assert_eq!(resolved.source(), GatewaySource::Config);
    assert_eq!(resolved.base_url(), "http://127.0.0.1:8081");
    assert_eq!(resolved.api_key(), "config-key");
}

#[test]
fn a_stale_file_with_no_explicit_config_carries_the_reason_into_the_error() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let port = fixture_gateway("right");
    live_file(port, "wrong")
        .write_to(dir.path())
        .expect("write");
    let config = GatewayConfig {
        base_url: String::new(),
        api_key: String::new(),
    };
    let error = resolve_with(Some(dir.path()), &config, probe_own_image)
        .expect_err("no explicit config remains");
    assert_eq!(error.stale(), Some(StaleReason::KeyRejected));
    let message = error.to_string();
    assert!(
        message.contains("no gateway configured or running"),
        "the error says it plainly: {message}"
    );
    assert!(
        message.contains("key was rejected"),
        "the condemned file's reason is named: {message}"
    );
}

#[test]
fn no_file_and_explicit_config_uses_the_config() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_own_image)
        .expect("explicit config resolves");
    assert_eq!(resolved.source(), GatewaySource::Config);
    assert_eq!(resolved.stale(), None);
}

/// A probe whose discovery-file read fails: a directory sits where
/// `gateway.json` belongs, so the read errors instead of answering.
fn probe_read_failure(run_dir: &Path) -> Result<Resolution, SidecarError> {
    std::fs::create_dir(run_dir.join("gateway.json")).expect("the unreadable file plants");
    shared_sidecar::resolve_for_test(run_dir, &own_image_name())
}

#[test]
fn a_probe_io_failure_degrades_to_the_config_fallback() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let resolved = resolve_with(Some(dir.path()), &explicit_config(), probe_read_failure)
        .expect("a probe failure never fails startup on its own");
    assert_eq!(resolved.source(), GatewaySource::Config);
    assert_eq!(resolved.base_url(), "http://gateway.lan:9999");
    assert_eq!(resolved.stale(), None, "nothing was condemned");
}

#[test]
fn a_probe_io_failure_with_no_explicit_config_is_the_plain_error() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let config = GatewayConfig {
        base_url: String::new(),
        api_key: String::new(),
    };
    let error = resolve_with(Some(dir.path()), &config, probe_read_failure)
        .expect_err("no explicit config remains after a probe failure");
    assert!(
        error
            .to_string()
            .contains("no gateway configured or running"),
        "the error says it plainly: {error}"
    );
    assert_eq!(error.stale(), None, "a probe failure is not a condemnation");
}

#[test]
fn no_run_directory_skips_discovery() {
    // The production probe stands in: with no run directory it is
    // never called, so resolution is the config fallback or the plain
    // error, and the real run directory is never consulted.
    let resolved = resolve_with(None, &explicit_config(), probe)
        .expect("explicit config resolves without a run directory");
    assert_eq!(resolved.source(), GatewaySource::Config);
    assert_eq!(resolved.stale(), None);

    let config = GatewayConfig {
        base_url: String::new(),
        api_key: String::new(),
    };
    let error = resolve_with(None, &config, probe)
        .expect_err("no run directory and no explicit config is the plain error");
    assert!(
        error
            .to_string()
            .contains("no gateway configured or running"),
        "the error says it plainly: {error}"
    );
}

#[test]
fn the_report_names_the_winning_source_and_a_condemned_file() {
    // A recording status sink stands in for the status bus: the report's
    // frames land on it through the registry's push facade.
    let registry = workshop_registry::Registry::new();
    let (status_tx, mut receiver) = tokio::sync::broadcast::channel(16);
    let _sink = registry.register_sink::<dyn workshop_registry::StatusSink>(std::sync::Arc::new(
        workshop_registry::StatusSinkAdapter::new(move |update| {
            let _ = status_tx.send(update);
        }),
    ));
    let push = registry.push();

    let resolved = ResolvedGateway {
        base_url: "http://127.0.0.1:4000".to_owned(),
        api_key: "k".to_owned(),
        identity: None,
        source: GatewaySource::Config,
        stale: Some(StaleReason::KeyRejected),
    };
    report(&resolved, &push);

    let stale_frame = receiver.try_recv().expect("the stale note is reported");
    assert_eq!(stale_frame.label, "Stale gateway discovery file");
    assert!(
        stale_frame.description.contains("key was rejected"),
        "the stale reason is named: {}",
        stale_frame.description
    );
    let connecting = receiver.try_recv().expect("the winning source is reported");
    assert_eq!(connecting.label, "Connecting to gateway");
    assert!(
        connecting.description.contains("http://127.0.0.1:4000")
            && connecting.description.contains("workshop.toml"),
        "the endpoint and its source are named: {}",
        connecting.description
    );
}
