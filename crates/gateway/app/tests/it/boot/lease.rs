//! Single-instance lease: a second launch hands off without rotating the log, a
//! silent or unreadable owner fails console-only, and a terminated owner's lease
//! is recovered.

use std::time::Duration;

use serde_json::Value;

use super::{race_config, write_config};
use crate::support::{GatewayProcess, wait_for_connection};

/// A second launch hands off to the running gateway and exits: under
/// `--print-url` it prints the running gateway's own Settings URL. Because
/// the handoff runs before logging starts, the running gateway's log is
/// never rotated and gains no second startup line.
#[test]
fn a_second_instance_hands_off_without_rotating_the_log() {
    let temp = tempfile::tempdir().unwrap();
    let path = write_config(
        &temp,
        "config-version = 0\n\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\n\
         [[profile]]\nname = \"main\"\nmodels = []\n"
            .to_string(),
    );
    let logs = temp.path().join(".promptforge").join("logs");
    let connection = temp
        .path()
        .join(".promptforge")
        .join("run")
        .join("gateway.json");
    let mut first = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--config")
        .arg(&path)
        .arg("--profile")
        .arg("main")
        .arg("--no-tray")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the first gateway spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !connection.is_file() {
        assert!(
            std::time::Instant::now() < deadline,
            "the first gateway bound and wrote {}",
            connection.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let file: Value = serde_json::from_str(
        &std::fs::read_to_string(&connection).expect("read the gateway discovery file"),
    )
    .expect("the gateway discovery file is JSON");
    let port = file["port"].as_u64().expect("the file holds a port");

    // The second launch: the handoff prints the running gateway's URL and
    // exits. A regression to a normal boot would serve instead, so the
    // exit wait is bounded and the kill is the failure path.
    let mut second = std::process::Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"))
        .arg("--config")
        .arg(&path)
        .arg("--profile")
        .arg("main")
        .arg("--print-url")
        .env("USERPROFILE", temp.path())
        .env("HOME", temp.path())
        .env_remove("RUST_LOG")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("the second gateway spawns");
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = second.try_wait().expect("poll the second instance") {
            break status;
        }
        if std::time::Instant::now() >= deadline {
            let _ = second.kill();
            panic!("the second instance booted a duplicate server instead of handing off");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "the handoff exits successfully: {status}");
    let mut stdout = String::new();
    std::io::Read::read_to_string(
        &mut second.stdout.take().expect("piped stdout"),
        &mut stdout,
    )
    .expect("read the second instance's stdout");
    assert!(
        stdout.contains(&format!("http://127.0.0.1:{port}/auth?key=")),
        "the printed URL is the running gateway's own handoff URL: {stdout}"
    );

    let _ = first.kill();
    let _ = first.wait();
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the handoff never rotated the running gateway's log"
    );
    let log = std::fs::read_to_string(logs.join("gateway.log")).expect("read the log");
    assert_eq!(
        log.matches("logging to").count(),
        1,
        "only the serving instance wrote a startup line: {log}"
    );
}

/// A process that loses the lifetime lease to a silent owner exits nonzero
/// after the bounded publication wait. The failure stays on stderr and never
/// initializes or rotates canonical logging.
#[test]
fn a_silent_process_lease_owner_causes_a_bounded_console_only_failure() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the owner's canonical log");
    let _owner = gateway_api_discovery::GatewayInstanceLease::try_acquire(&run_dir)
        .expect("acquire the silent owner lease")
        .expect("the test owns the process lease");
    let connection_path = gateway_api_discovery::gateway_discovery_file_path(&run_dir);
    std::fs::write(&connection_path, b"owner-is-still-publishing")
        .expect("seed an unreadable owner record");

    let mut loser = GatewayProcess::spawn(&config, temp.path());
    let status = loser.wait_for_exit(Duration::from_secs(15));
    assert!(
        !status.success(),
        "a lease loser without a validated owner record exits nonzero"
    );
    let stderr = loser.stderr();
    assert!(
        stderr.contains("process owner published no validated connection"),
        "the console error names the bounded ownership failure: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded canonical log"),
        "owner-log-sentinel",
        "the losing process never initializes canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the losing process never rotates the canonical log"
    );
    assert_eq!(
        std::fs::read(&connection_path).expect("read the seeded owner record"),
        b"owner-is-still-publishing",
        "the losing process never cleans or rewrites shared connection state"
    );
}

/// A lease holder that cannot read existing shared state exits through the
/// console-only startup path, preserving the source chain and canonical log.
#[test]
fn a_connection_resolution_failure_is_console_only() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let state_dir = temp.path().join(".promptforge");
    let run_dir = state_dir.join("run");
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs).expect("create seeded log directory");
    let log_path = logs.join("gateway.log");
    std::fs::write(&log_path, "owner-log-sentinel").expect("seed the canonical log");
    let connection_path = gateway_api_discovery::gateway_discovery_file_path(&run_dir);
    std::fs::create_dir_all(&connection_path).expect("create unreadable connection fixture");

    let mut gateway = GatewayProcess::spawn(&config, temp.path());
    let status = gateway.wait_for_exit(Duration::from_secs(5));

    assert!(
        !status.success(),
        "an unresolved connection record prevents boot"
    );
    let stderr = gateway.stderr();
    assert!(
        stderr.contains("resolve existing Gateway connection before startup")
            && stderr.contains("caused by: read"),
        "stderr retains the resolution failure and source chain: {stderr}"
    );
    assert_eq!(
        std::fs::read_to_string(&log_path).expect("read the seeded log"),
        "owner-log-sentinel",
        "the failure never initializes or rotates canonical logging"
    );
    assert!(
        !logs.join("gateway.log.1").exists(),
        "the failure creates no retained log"
    );
    assert!(
        connection_path.is_dir(),
        "the failure leaves uncertain connection state untouched"
    );
}

/// Terminating the real owner leaves its connection record behind but
/// releases the operating-system lease, so a later direct launch cleans the
/// stale record and becomes the sole owner.
#[test]
fn a_direct_launch_recovers_the_lease_from_a_terminated_owner() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let mut first = GatewayProcess::spawn(&config, temp.path());
    let first_connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    assert_eq!(first_connection.pid, first.id());
    assert!(
        gateway_api_discovery::GatewayInstanceLease::try_acquire(&run_dir)
            .expect("contend for the first owner's process lease")
            .is_none(),
        "the first process owns the lifetime lease"
    );
    first.stop(Duration::from_secs(5));

    let mut replacement = GatewayProcess::spawn(&config, temp.path());
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    let replacement_connection = loop {
        if let Some(connection) = gateway_api_discovery::GatewayDiscoveryFile::read(&run_dir)
            .expect("read replacement connection")
            && connection.pid == replacement.id()
        {
            break connection;
        }
        assert!(
            replacement.try_wait().is_none(),
            "the replacement exited before taking the dead owner's lease"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "the replacement did not publish after the owner was terminated"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_ne!(
        replacement_connection.pid, first_connection.pid,
        "the stale owner record was replaced"
    );
    replacement.stop(Duration::from_secs(5));
}
