//! Ownership races at the lease rendezvous: simultaneous launches elect one
//! owner, and the default binary ignores the rendezvous environment.

use std::time::Duration;

use super::race_config;
use crate::support::{GatewayProcess, PHASE_TIMEOUT, wait_for_connection};

#[cfg(feature = "test-fixtures")]
fn spawn_at_ownership_rendezvous(
    config: &std::path::Path,
    home: &std::path::Path,
) -> (GatewayProcess, GatewayProcess) {
    let first_ready = home.join("first-before-ownership.ready");
    let second_ready = home.join("second-before-ownership.ready");
    let release = home.join("release-ownership-race");
    let mut first = GatewayProcess::spawn_gated(config, home, &first_ready, &release);
    let mut second = GatewayProcess::spawn_gated(config, home, &second_ready, &release);
    let deadline = std::time::Instant::now() + PHASE_TIMEOUT;
    while !(first_ready.is_file() && second_ready.is_file()) {
        assert!(
            first.try_wait().is_none() && second.try_wait().is_none(),
            "both Gateway processes remain blocked at the ownership rendezvous"
        );
        assert!(
            std::time::Instant::now() < deadline,
            "both Gateway processes reached the ownership rendezvous"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        !gateway_api_discovery::gateway_discovery_file_path(&home.join(".promptforge").join("run"))
            .exists(),
        "neither process can acquire ownership or publish before release"
    );
    std::fs::write(&release, b"release").expect("release both ownership contenders");
    (first, second)
}

#[cfg(feature = "test-fixtures")]
fn assert_exactly_one_process_owns(
    first: &mut GatewayProcess,
    second: &mut GatewayProcess,
    connection: &gateway_api_discovery::GatewayDiscoveryFile,
) -> bool {
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    loop {
        let first_status = first.try_wait();
        let second_status = second.try_wait();
        match (first_status, second_status) {
            (Some(status), None) => {
                assert!(status.success(), "the first launch hands off: {status}");
                assert_eq!(
                    connection.pid,
                    second.id(),
                    "the published connection names the surviving owner"
                );
                let output = first.stdout();
                assert!(
                    output.contains(&format!("http://127.0.0.1:{}/auth?key=", connection.port)),
                    "the first launch printed the owner's handoff URL: {output}"
                );
                return false;
            }
            (None, Some(status)) => {
                assert!(status.success(), "the second launch hands off: {status}");
                assert_eq!(
                    connection.pid,
                    first.id(),
                    "the published connection names the surviving owner"
                );
                let output = second.stdout();
                assert!(
                    output.contains(&format!("http://127.0.0.1:{}/auth?key=", connection.port)),
                    "the second launch printed the owner's handoff URL: {output}"
                );
                return true;
            }
            (Some(first_status), Some(second_status)) => {
                panic!(
                    "both Gateway launches exited instead of leaving one owner: \
                     {first_status}, {second_status}"
                );
            }
            (None, None) => {}
        }
        assert!(
            std::time::Instant::now() < deadline,
            "both Gateway launches kept serving instead of electing one owner"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(feature = "test-fixtures")]
fn assert_one_canonical_log(home: &std::path::Path) {
    let logs = home.join(".promptforge").join("logs");
    let log_path = logs.join("gateway.log");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    let log = loop {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        if log.contains("logging to") {
            break log;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the owner did not write its canonical startup log"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    assert!(
        !logs.join("gateway.log.1").exists(),
        "a losing process never rotates the owner's canonical log"
    );
    assert_eq!(
        log.matches("promptforge-gateway").count(),
        1,
        "only the owner writes the versioned startup record: {log}"
    );
    assert_eq!(
        log.matches("logging to").count(),
        1,
        "only the owner initializes canonical logging: {log}"
    );
}

/// Two direct process launches against an absent record elect one process
/// owner before either can initialize logging or bind.
#[cfg(feature = "test-fixtures")]
#[test]
fn simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let (mut first, mut second) = spawn_at_ownership_rendezvous(&config, temp.path());

    let connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    let first_owns = assert_exactly_one_process_owns(&mut first, &mut second, &connection);
    assert_one_canonical_log(temp.path());
    assert!(
        gateway_api_discovery::GatewayInstanceLease::try_acquire(&run_dir)
            .expect("contend for the live Gateway's process lease")
            .is_none(),
        "the surviving process keeps its lease for its serving lifetime"
    );

    if first_owns {
        first.stop(Duration::from_secs(5));
    } else {
        second.stop(Duration::from_secs(5));
    }
}

/// A Workshop-elected launch keeps the parent-side `LaunchLock` while two
/// real Gateway processes race. Gateway ownership must not contend on that
/// parent lock, and only one child may survive.
#[cfg(feature = "test-fixtures")]
#[test]
fn workshop_launch_lock_and_direct_launch_do_not_deadlock_or_double_boot() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let gateway_api_discovery::LaunchDecision::Launch(workshop_lock) =
        gateway_api_discovery::launch_or_attach(&run_dir, Duration::from_secs(5))
            .expect("Workshop wins the parent launch election")
    else {
        panic!("an empty run directory elects the Workshop launcher");
    };

    let (mut workshop_launch, mut direct_launch) =
        spawn_at_ownership_rendezvous(&config, temp.path());
    let connection = wait_for_connection(&run_dir, Duration::from_secs(30));
    let workshop_owns =
        assert_exactly_one_process_owns(&mut workshop_launch, &mut direct_launch, &connection);
    assert_one_canonical_log(temp.path());
    drop(workshop_lock);

    if workshop_owns {
        workshop_launch.stop(Duration::from_secs(5));
    } else {
        direct_launch.stop(Duration::from_secs(5));
    }
}

/// The ordinary production binary has no compiled rendezvous hook: even
/// environment names used by the feature-enabled fixture are inert.
///
/// Retired from every current runner: the gateway's self dev-dependency
/// (`gateway = { path = ".", features = ["test-fixtures"], ... }`, added so
/// the speech relay suites get test-scaled bounds without a `--features`
/// flag) forces `test-fixtures` into every test-target build, so this
/// `not(test-fixtures)` test compiles out under plain `cargo test -p
/// gateway` just as it does under CI's `--all-features` and `--features
/// test-fixtures` invocations. The property it pins still matters - a
/// default-feature binary must ignore the rendezvous environment - so the
/// test stays. Anything that builds the gateway test targets without
/// `test-fixtures` re-enables it: a `cargo test -p gateway
/// --no-default-features`-shaped invocation once the self dev-dependency no
/// longer forces the feature in, or the dev-dependency's removal.
#[cfg(not(feature = "test-fixtures"))]
#[test]
fn the_default_binary_ignores_test_rendezvous_environment() {
    let temp = tempfile::tempdir().unwrap();
    let config = race_config(&temp);
    let run_dir = temp.path().join(".promptforge").join("run");
    let ready = temp.path().join("default-build-ready");
    let absent_release = temp.path().join("default-build-release");
    let mut gateway =
        GatewayProcess::spawn_with_inert_rendezvous(&config, temp.path(), &ready, &absent_release);

    let connection = wait_for_connection(&run_dir, PHASE_TIMEOUT);
    assert_eq!(
        connection.pid,
        gateway.id(),
        "the default binary serves without consulting test rendezvous state"
    );
    assert!(
        !ready.exists(),
        "the default binary never writes the test synchronization marker"
    );
    gateway.stop(Duration::from_secs(5));
}
