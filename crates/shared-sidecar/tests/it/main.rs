//! End-to-end tests against the public API: the gateway discovery file
//! lifecycle, stale detection with cleanup, the launch-race lock, and the
//! health probe against a fixture listener.
#![expect(
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

use std::time::Duration;

use shared_sidecar::{
    GatewayDiscoveryFile, GatewayInstanceLease, LaunchDecision, Resolution, SidecarError,
    StaleReason, gateway_discovery_file_path, instance_lock_file_path, launch_or_attach,
    lock_file_path, resolve, wait_for_health,
};

const LEASE_CHILD_RUN_DIR: &str = "PROMPTFORGE_LEASE_CHILD_RUN_DIR";

struct BoundedChild(std::process::Child);

impl BoundedChild {
    fn stop(&mut self, timeout: Duration) {
        let _ = self.0.kill();
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if self
                .0
                .try_wait()
                .expect("observe lease fixture process")
                .is_some()
            {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the lease fixture did not stop within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for BoundedChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while self.0.try_wait().ok().flatten().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A valid gateway discovery file; the pid is the test process's own.
fn valid_file() -> GatewayDiscoveryFile {
    GatewayDiscoveryFile {
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
fn a_write_then_read_round_trips_the_gateway_discovery_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = valid_file();
    file.write_to(dir.path()).expect("write");
    assert_eq!(
        GatewayDiscoveryFile::read(dir.path()).expect("read"),
        Some(file)
    );
}

#[test]
fn a_write_creates_a_missing_run_directory() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let run_dir = dir.path().join("nested").join("run");
    valid_file().write_to(&run_dir).expect("write");
    assert!(gateway_discovery_file_path(&run_dir).exists());
}

#[cfg(unix)]
#[test]
fn the_written_file_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::TempDir::new().expect("tempdir");
    valid_file().write_to(dir.path()).expect("write");
    let mode = gateway_discovery_file_path(dir.path())
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
        gateway_discovery_file_path(dir.path()).exists(),
        "a foreign pid's removal spares the file"
    );

    shared_sidecar::remove_if_mine(dir.path(), file.pid).expect("the owning pid removes");
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
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
    std::fs::write(gateway_discovery_file_path(dir.path()), b"not json").expect("write fixture");
    assert_eq!(
        resolve(dir.path()).expect("resolve"),
        Resolution::Stale(StaleReason::Invalid),
        "a corrupt file is stale"
    );
    assert!(
        !gateway_discovery_file_path(dir.path()).exists(),
        "the corrupt file was deleted"
    );
}

#[test]
fn resolve_cleans_a_dead_pid_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let file = GatewayDiscoveryFile {
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
        !gateway_discovery_file_path(dir.path()).exists(),
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
fn a_process_lifetime_lease_recovers_after_its_owner_is_terminated() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let ready = dir.path().join("lease-ready");
    let mut child = BoundedChild(
        std::process::Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "gateway_instance_lease_fixture_process",
                "--ignored",
            ])
            .env(LEASE_CHILD_RUN_DIR, dir.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn lease fixture process"),
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !ready.is_file() {
        assert!(
            child
                .0
                .try_wait()
                .expect("observe lease fixture process")
                .is_none(),
            "the lease fixture exited before readiness"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the lease fixture did not acquire its lock"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(
        GatewayInstanceLease::try_acquire(dir.path())
            .expect("contend for the process lease")
            .is_none(),
        "a second process cannot acquire the live owner's lease"
    );
    assert!(
        instance_lock_file_path(dir.path()).is_file(),
        "the process lease uses its dedicated run-directory path"
    );

    child.stop(Duration::from_secs(5));
    assert!(
        GatewayInstanceLease::try_acquire(dir.path())
            .expect("recover the dead owner's process lease")
            .is_some(),
        "the operating system releases the lease when its process dies"
    );
}

#[test]
#[ignore = "spawned by the process-lifetime lease parent"]
fn gateway_instance_lease_fixture_process() {
    let Some(run_dir) = std::env::var_os(LEASE_CHILD_RUN_DIR) else {
        return;
    };
    let run_dir = std::path::PathBuf::from(run_dir);
    let _lease = GatewayInstanceLease::try_acquire(&run_dir)
        .expect("acquire the fixture process lease")
        .expect("the fixture process owns the lease");
    std::fs::write(run_dir.join("lease-ready"), b"ready").expect("announce lease readiness");
    std::thread::sleep(Duration::from_secs(60));
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
