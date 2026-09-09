//! Shared fixtures for the split Gateway lifecycle tests.

use std::io::{Read, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use shared_sidecar::{GatewayDiscoveryFile, Resolution, SidecarError};

use super::boot as gateway_boot;

mod boot;
mod identity;
mod recovery;
mod shutdown;

/// Resolves against the test process's own image.
fn probe_own_image(run_dir: &Path) -> Result<Resolution, SidecarError> {
    let image = std::env::current_exe()
        .expect("current exe")
        .file_name()
        .expect("the exe has a file name")
        .to_string_lossy()
        .into_owned();
    shared_sidecar::resolve_for_test(run_dir, &image)
}

/// Plants an unreadable discovery-file path before resolving.
fn probe_read_failure(run_dir: &Path) -> Result<Resolution, SidecarError> {
    std::fs::create_dir(shared_sidecar::gateway_discovery_file_path(run_dir))
        .expect("plant the unreadable file");
    probe_own_image(run_dir)
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

/// Returns a pid whose short-lived child has been reaped.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
        .arg("--list")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a short-lived child");
    let pid = child.id();
    child.wait().expect("the child exits");
    pid
}

/// Starts a lightweight same-process health and bearer fixture.
fn fixture_gateway(expected_key: impl Into<String>) -> u16 {
    let expected_key = Arc::new(expected_key.into());
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("fixture address").port();
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let expected_key = Arc::clone(&expected_key);
            std::thread::spawn(move || {
                loop {
                    let mut buffer = [0_u8; 1024];
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let accepted = request.starts_with("GET /health ")
                        || accepts_bearer(&request, &expected_key);
                    let response = if accepted {
                        &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                    } else {
                        &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                    };
                    if stream.write_all(response).is_err() {
                        break;
                    }
                }
            });
        }
    });
    port
}

fn accepts_bearer(request: &str, expected_key: &str) -> bool {
    request.lines().any(|line| {
        let Some((name, value)) = line.split_once(':') else {
            return false;
        };
        let Some((scheme, credential)) = value.trim_start().split_once(' ') else {
            return false;
        };
        name.eq_ignore_ascii_case("authorization")
            && scheme.eq_ignore_ascii_case("bearer")
            && credential == expected_key
    })
}

/// Starts the shared named-process Gateway fixture in this test binary.
fn validated_gateway(expected_key: &str) -> workshop_server::fixtures::ValidatedGateway {
    workshop_server::fixtures::ValidatedGateway::spawn_in(
        expected_key,
        "gateway::tests::validated_gateway_fixture_process",
    )
}

#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    workshop_server::fixtures::run_validated_gateway_fixture_process();
}

/// Sends one plain GET to the Workshop fixture.
fn get(url: &str, path: &str) -> String {
    let url = url::Url::parse(url).expect("the Workshop URL parses");
    let host = url.host_str().expect("the Workshop URL has a host");
    let port = url.port().expect("the Workshop URL has a port");
    let mut stream = TcpStream::connect((host, port)).expect("connect to Workshop");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n"
    )
    .expect("send Workshop request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read Workshop response");
    response
}

/// An executable directory, with or without the sibling Gateway.
fn exe_dir(with_gateway: bool) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().expect("tempdir");
    if with_gateway {
        std::fs::write(dir.path().join(gateway_boot::GATEWAY_EXE_NAME), b"")
            .expect("plant the sibling exe");
    }
    let path = dir.path().to_owned();
    (dir, path)
}
