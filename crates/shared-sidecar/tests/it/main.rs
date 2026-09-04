//! End-to-end tests against the public API: the connection-file
//! lifecycle, stale detection with cleanup, the launch-race lock, and the
//! health probe against a fixture listener.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

use std::time::Duration;

use shared_sidecar::{
    ConnectionFile, LaunchDecision, Resolution, SidecarError, StaleReason, connection_file_path,
    launch_or_attach, lock_file_path, resolve, wait_for_health,
};

/// A valid connection file; the pid is the test process's own.
fn valid_file() -> ConnectionFile {
    ConnectionFile {
        port: 8081,
        api_key: "key".to_owned(),
        pid: std::process::id(),
        epoch: 1_757_000_000,
        version: "0.2.0".to_owned(),
        started_at: "2026-09-03T12:00:00Z".to_owned(),
    }
}

/// A pid guaranteed dead: a short-lived child, reaped and dropped so no
/// handle keeps the process object alive.
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

#[test]
fn a_write_then_read_round_trips_the_connection_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = valid_file();
    file.write_to(dir.path()).expect("write");
    assert_eq!(ConnectionFile::read(dir.path()).expect("read"), Some(file));
}

#[test]
fn a_write_creates_a_missing_run_directory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let run_dir = dir.path().join("nested").join("run");
    valid_file().write_to(&run_dir).expect("write");
    assert!(connection_file_path(&run_dir).exists());
}

#[cfg(unix)]
#[test]
fn the_written_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::TempDir::new().expect("tempdir");
    valid_file().write_to(dir.path()).expect("write");
    let mode = connection_file_path(dir.path())
        .metadata()
        .expect("metadata")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "the bearer key file is owner-only");
}

#[test]
fn remove_if_mine_spares_a_foreign_pid_and_removes_the_owning_one() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = valid_file();
    file.write_to(dir.path()).expect("write");

    shared_sidecar::remove_if_mine(dir.path(), file.pid + 1).expect("a foreign pid is tolerated");
    assert!(
        connection_file_path(dir.path()).exists(),
        "a foreign pid's removal spares the file"
    );

    shared_sidecar::remove_if_mine(dir.path(), file.pid).expect("the owning pid removes");
    assert!(
        !connection_file_path(dir.path()).exists(),
        "the owning pid's removal deletes the file"
    );
}

#[test]
fn resolve_reports_absent_without_a_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    assert_eq!(resolve(dir.path()).expect("resolve"), Resolution::Absent);
}

#[test]
fn resolve_cleans_a_corrupt_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    std::fs::write(connection_file_path(dir.path()), b"not json").expect("write fixture");
    assert_eq!(
        resolve(dir.path()).expect("resolve"),
        Resolution::Stale(StaleReason::Invalid),
        "a corrupt file is stale"
    );
    assert!(
        !connection_file_path(dir.path()).exists(),
        "the corrupt file was deleted"
    );
}

#[test]
fn resolve_cleans_a_dead_pid_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = ConnectionFile {
        pid: dead_pid(),
        ..valid_file()
    };
    file.write_to(dir.path()).expect("write");
    assert_eq!(
        resolve(dir.path()).expect("resolve"),
        Resolution::Stale(StaleReason::ProcessDead),
        "a dead pid is stale"
    );
    assert!(
        !connection_file_path(dir.path()).exists(),
        "the stale file was deleted"
    );
}

#[test]
fn launch_or_attach_elects_one_launcher_and_times_out_the_loser() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let decision = launch_or_attach(dir.path(), Duration::from_secs(5)).expect("the race settles");
    assert!(
        matches!(decision, LaunchDecision::Launch(_)),
        "an empty run dir elects a launcher"
    );
    assert!(
        lock_file_path(dir.path()).exists(),
        "the lock file was created"
    );

    let error = launch_or_attach(dir.path(), Duration::from_millis(150))
        .expect_err("a held lock with no live file starves the loser");
    assert!(
        matches!(error, SidecarError::LaunchTimeout { .. }),
        "the loser reports the timeout: {error}"
    );
}

#[test]
fn wait_for_health_answers_against_a_fixture_listener() {
    use std::io::Write as _;
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    std::thread::spawn(move || {
        while let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
        }
    });

    wait_for_health(&format!("http://{address}"), Duration::from_secs(5))
        .expect("the fixture answers 200");

    let error = wait_for_health("http://127.0.0.1:1", Duration::from_millis(150))
        .expect_err("a dead port never satisfies the probe");
    assert!(
        error.to_string().contains("did not answer"),
        "the error names the timeout: {error}"
    );
}
