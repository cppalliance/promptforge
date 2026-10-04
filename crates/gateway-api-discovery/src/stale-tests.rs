//! Unit tests for stale detection and stale-file cleanup.

use super::*;

use std::io::{Read, Write as _};
use std::net::TcpListener;
use std::sync::mpsc;

use crate::paths::gateway_discovery_file_path;

/// The test process's own image name, so the pid and image checks
/// pass and the test reaches the probe under test.
fn own_image_name() -> String {
    std::env::current_exe()
        .expect("current exe")
        .file_name()
        .expect("the exe has a file name")
        .to_string_lossy()
        .into_owned()
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

#[test]
fn a_dead_pid_is_stale_and_cleaned() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = live_file(1, "key");
    let file = GatewayDiscoveryFile {
        pid: dead_pid(),
        ..file
    };
    file.write_to(dir.path()).expect("write");

    let resolution = resolve(dir.path()).expect("resolve");
    assert_eq!(resolution, Resolution::Stale(StaleReason::ProcessDead));
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
        "the stale file was deleted"
    );
}

#[test]
fn an_alive_pid_with_a_foreign_image_is_stale() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // The test binary is never named promptforge-gateway, so the real
    // image name must reject the test process's own pid.
    live_file(1, "key").write_to(dir.path()).expect("write");

    let resolution = resolve(dir.path()).expect("resolve");
    assert_eq!(resolution, Resolution::Stale(StaleReason::ImageMismatch));
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
        "the stale file was deleted"
    );
}

#[test]
fn a_silent_health_endpoint_is_stale() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // Port 1 never listens, so every probe is refused until the
    // liveness budget elapses.
    live_file(1, "key").write_to(dir.path()).expect("write");

    let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
    assert_eq!(resolution, Resolution::Stale(StaleReason::HealthFailed));
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
        "the stale file was deleted"
    );
}

#[test]
fn a_transiently_silent_health_endpoint_is_not_stale() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    // The first probe connection is hung up without a response; only
    // a retried probe learns the gateway is live.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let port = listener.local_addr().expect("fixture address").port();
    std::thread::spawn(move || {
        let mut first = true;
        while let Ok((mut stream, _)) = listener.accept() {
            if std::mem::take(&mut first) {
                drop(stream);
                continue;
            }
            for _ in 0..2 {
                let mut buffer = [0u8; 1024];
                if stream.read(&mut buffer).is_err() {
                    break;
                }
                if stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                    .is_err()
                {
                    break;
                }
            }
        }
    });
    let file = live_file(port, "key");
    file.write_to(dir.path()).expect("write");

    let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
    assert_eq!(
        resolution,
        Resolution::Attach(file),
        "one dropped probe must not reject a live gateway"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "a live file is left in place"
    );
}

#[test]
fn a_rejected_key_is_stale_and_cleaned() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let port = fixture_gateway("right");
    live_file(port, "wrong")
        .write_to(dir.path())
        .expect("write");

    let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
    assert_eq!(resolution, Resolution::Stale(StaleReason::KeyRejected));
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
        "the stale file was deleted"
    );
}

#[test]
fn a_fully_live_file_attaches() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let port = fixture_gateway("right");
    let file = live_file(port, "right");
    file.write_to(dir.path()).expect("write");

    let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
    assert_eq!(resolution, Resolution::Attach(file));
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "a live file is left in place"
    );
}

#[test]
fn cancellation_during_resolve_leaves_the_gateway_discovery_file_untouched() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    live_file(1, "key").write_to(dir.path()).expect("write");
    let cancellation = crate::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let run_dir = dir.path().to_owned();
    let image_name = own_image_name();
    let (entered, blocked) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        resolve_named_cancellable_with(
            &run_dir,
            &image_name,
            &worker_cancellation,
            |_, _, cancellation| {
                entered.send(()).expect("announce blocked resolve");
                let _ = cancellation.wait_timeout(std::time::Duration::from_secs(30));
                Err(crate::ValidationError::Cancelled)
            },
        )
    });
    blocked
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("the resolve phase blocks deterministically");

    let started = std::time::Instant::now();
    cancellation.cancel();
    let result = worker.join().expect("the resolve worker joins");

    assert!(
        started.elapsed() < std::time::Duration::from_millis(250),
        "cancellation bounds the blocked resolve"
    );
    assert!(matches!(result, Err(SidecarError::Cancelled)));
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "cancellation never classifies or deletes the gateway discovery file"
    );
}

#[test]
fn cancellation_immediately_before_invalid_file_removal_preserves_the_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    fs::write(gateway_discovery_file_path(dir.path()), b"not json").expect("write invalid fixture");
    let cancellation = crate::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let run_dir = dir.path().to_owned();
    let (entered, blocked) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let removals = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_removals = std::sync::Arc::clone(&removals);
    let worker = std::thread::spawn(move || {
        resolve_named_cancellable_with_effects(
            &run_dir,
            "unused",
            &worker_cancellation,
            |_, _, _| -> Result<ValidatedConnection, ValidationError> {
                panic!("an invalid file never reaches validation")
            },
            || {
                entered.send(()).expect("announce invalid-file removal");
                released.recv().expect("release invalid-file removal");
            },
            |run_dir| {
                worker_removals.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                remove_stale(run_dir)
            },
        )
    });
    blocked
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("resolution pauses immediately before invalid-file removal");

    cancellation.cancel();
    release.send(()).expect("release invalid-file removal");
    let result = worker.join().expect("resolve worker joins");

    assert!(matches!(result, Err(SidecarError::Cancelled)));
    assert_eq!(
        removals.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "invalid-file cleanup cannot begin after cancellation returns"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "the cancelled invalid file remains untouched"
    );
}

#[test]
fn cancellation_immediately_before_failed_validation_removal_preserves_the_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    live_file(1, "key").write_to(dir.path()).expect("write");
    let cancellation = crate::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let run_dir = dir.path().to_owned();
    let (entered, blocked) = mpsc::channel();
    let (release, released) = mpsc::channel();
    let removals = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let worker_removals = std::sync::Arc::clone(&removals);
    let worker = std::thread::spawn(move || {
        resolve_named_cancellable_with_effects(
            &run_dir,
            "unused",
            &worker_cancellation,
            |_, _, _| Err(ValidationError::Stale(StaleReason::HealthFailed)),
            || {
                entered.send(()).expect("announce stale-file removal");
                released.recv().expect("release stale-file removal");
            },
            |run_dir| {
                worker_removals.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                remove_stale(run_dir)
            },
        )
    });
    blocked
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("resolution pauses immediately before stale-file removal");

    cancellation.cancel();
    release.send(()).expect("release stale-file removal");
    let result = worker.join().expect("resolve worker joins");

    assert!(matches!(result, Err(SidecarError::Cancelled)));
    assert_eq!(
        removals.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "failed-validation cleanup cannot begin after cancellation returns"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "the cancelled stale file remains untouched"
    );
}

#[test]
fn is_running_reports_a_live_gateway_without_touching_the_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let port = fixture_gateway("right");
    live_file(port, "right")
        .write_to(dir.path())
        .expect("write");

    assert!(
        is_running_named(dir.path(), &own_image_name()),
        "a fully live file reads as running"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "the read-only check never deletes"
    );
}

#[test]
fn is_running_leaves_a_stale_file_in_place() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = GatewayDiscoveryFile {
        pid: dead_pid(),
        ..live_file(1, "key")
    };
    file.write_to(dir.path()).expect("write");

    assert!(
        !is_running_named(dir.path(), &own_image_name()),
        "a dead pid reads as not-running"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "stale-file deletion is the prospective owner's privilege"
    );
}

#[test]
fn is_running_reads_absent_and_corrupt_files_as_not_running() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    assert!(
        !is_running_named(dir.path(), &own_image_name()),
        "no gateway discovery file reads as not-running"
    );
    fs::write(gateway_discovery_file_path(dir.path()), b"not json").expect("write fixture");
    assert!(
        !is_running_named(dir.path(), &own_image_name()),
        "a corrupt file reads as not-running and is left alone"
    );
    assert!(
        gateway_discovery_file_path(dir.path()).exists(),
        "the corrupt file was not deleted"
    );
}
