//! Continuous supervision, recovery, and joined-shutdown coverage.

use std::cell::{Cell, RefCell};
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use anyhow::Context as _;
use shared_sidecar::{CancellationToken, GatewayDiscoveryFile, Resolution, ValidatedConnection};

use super::{get, live_file, validated_gateway};
use crate::gateway::boot::RecoveryLaunch;
use crate::gateway::supervisor::{
    GatewaySupervisor, RecoveryCandidate, RecoveryOwnership, SUPERVISION_MAX_DELAY,
    SupervisedGatewayIdentity, SupervisionProbe, SupervisorShutdown,
    launch_and_attach_cancellable_with, run_effect_if_active, run_supervision, validate_recovery,
    wait_for_launched_file_cancellable_with,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct TestIdentity {
    file: GatewayDiscoveryFile,
    process_boot: u64,
}

impl SupervisedGatewayIdentity for TestIdentity {
    fn same_boot(&self, other: &Self) -> bool {
        self.process_boot == other.process_boot
            && self.file.pid == other.file.pid
            && self.file.epoch == other.file.epoch
            && self.file.started_at == other.file.started_at
    }
}

fn test_identity(file: GatewayDiscoveryFile) -> TestIdentity {
    TestIdentity {
        process_boot: u64::from(file.pid),
        file,
    }
}

#[test]
fn supervision_lives_past_sixty_seconds_then_propagates_a_configured_key_edit_atomically() {
    let gateway = validated_gateway("new-key");
    let original = gateway.gateway_discovery_file("old-key", 1_757_000_000, "2026-09-03T12:00:00Z");
    let replacement =
        gateway.gateway_discovery_file("new-key", 1_757_000_001, "2026-09-03T12:00:01Z");
    let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
    let server = workshop_server::fixtures::spawn(workshop_server::Config {
        gateway: workshop_server::GatewayConfig {
            base_url: format!("http://127.0.0.1:{}", original.port),
            api_key: original.api_key.clone(),
        },
        server: workshop_server::ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            open_browser: false,
            state_dir: state_dir.path().to_owned(),
        },
        agents: workshop_server::AgentsConfig::default(),
    })
    .expect("spawn Workshop against the original same-port key");
    let updater = server.gateway_updater();
    let publish_replacement = |file: &GatewayDiscoveryFile| -> anyhow::Result<()> {
        let validated = ValidatedConnection::validate(file.clone())
            .context("validate the named local Gateway")?;
        updater
            .replace_sidecar(&validated)
            .context("publish through the production updater")
    };
    let elapsed = Cell::new(Duration::ZERO);
    let recoveries = Cell::new(0_u8);
    let published = RefCell::new(Vec::new());
    let cancellation = CancellationToken::new();

    run_supervision(
        test_identity(original.clone()),
        |current, _| {
            if !published.borrow().is_empty() || elapsed.get() <= Duration::from_secs(65) {
                SupervisionProbe::Replacement(current.clone())
            } else {
                SupervisionProbe::Missing
            }
        },
        |_| {
            recoveries.set(recoveries.get() + 1);
            if recoveries.get() < 3 {
                anyhow::bail!("injected launch failure");
            }
            Ok(test_identity(replacement.clone()))
        },
        |identity, _| {
            publish_replacement(&identity.file)?;
            published.borrow_mut().push(identity.file.clone());
            Ok::<(), anyhow::Error>(())
        },
        |delay, _| {
            assert!(
                delay <= SUPERVISION_MAX_DELAY,
                "every supervision wait is capped: {delay:?}"
            );
            elapsed.set(elapsed.get() + delay);
            !published.borrow().is_empty()
        },
        &cancellation,
    );

    assert!(
        elapsed.get() > Duration::from_secs(60),
        "the supervisor remains live beyond one minute"
    );
    assert_eq!(recoveries.get(), 3, "failed launches retry under backoff");
    assert_eq!(
        published.borrow().as_slice(),
        [replacement],
        "one successful relaunch publishes its exact discovery-file pair"
    );
    assert_eq!(
        published.borrow()[0].port,
        original.port,
        "an OS-assigned port may be reused"
    );
    assert_ne!(
        published.borrow()[0].api_key,
        original.api_key,
        "a configured key edit propagates with the replacement identity"
    );
    let response = get(server.url(), "/gateway/api/admin/status");
    assert!(
        response.starts_with("HTTP/1.1 200"),
        "the real publisher replaces the bearer on the reused port: {response}"
    );
    server.shutdown().expect("stop the Workshop fixture");
}

#[test]
fn reused_pid_and_file_metadata_still_publish_a_new_validated_process_boot() {
    let file = live_file(54_375, "stable-key");
    let original = TestIdentity {
        file: file.clone(),
        process_boot: 41,
    };
    let replacement = TestIdentity {
        file: file.clone(),
        process_boot: 42,
    };
    let published = RefCell::new(Vec::new());
    let cancellation = CancellationToken::new();

    run_supervision(
        original.clone(),
        |current, _| {
            if published.borrow().is_empty() {
                SupervisionProbe::Replacement(replacement.clone())
            } else {
                SupervisionProbe::Replacement(current.clone())
            }
        },
        |_| -> anyhow::Result<TestIdentity> {
            panic!("a validated replacement does not need a relaunch")
        },
        |identity, _| {
            published.borrow_mut().push(identity.clone());
            Ok(())
        },
        |_, _| !published.borrow().is_empty(),
        &cancellation,
    );

    assert_eq!(
        published.borrow().as_slice(),
        [replacement],
        "the stable OS boot token prevents identical file fields from hiding pid reuse"
    );
    assert_eq!(published.borrow()[0].file, original.file);
}

fn assert_bounded_supervisor_shutdown(supervisor: GatewaySupervisor, finished: &AtomicBool) {
    let started = Instant::now();
    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Joined);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "Workshop exit joins the cancelled supervisor within its budget"
    );
    assert!(
        finished.load(Ordering::SeqCst),
        "shutdown returns only after the supervisor thread exits"
    );
}

#[test]
fn an_exact_spawned_pid_authenticates_late_child_cleanup() {
    let mut gateway = validated_gateway("owned-key");
    let validated = gateway.validate("owned-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let RecoveryOwnership::Owned(candidate) =
        RecoveryCandidate::authenticate(validated.pid(), validated)
    else {
        panic!("the validated file names the spawned child");
    };

    drop(candidate);

    assert!(
        gateway.received_shutdown(Duration::from_secs(1)),
        "an unpublished owned child receives authenticated shutdown"
    );
}

#[test]
fn explicit_candidate_shutdown_delivers_and_disarms_the_drop_signal() {
    let mut gateway = validated_gateway("owned-key");
    let validated = gateway.validate("owned-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let RecoveryOwnership::Owned(candidate) =
        RecoveryCandidate::authenticate(validated.pid(), validated)
    else {
        panic!("the validated file names the spawned child");
    };

    candidate
        .shutdown()
        .expect("the fixture accepts the authenticated shutdown");

    assert!(
        gateway.received_shutdown(Duration::from_secs(1)),
        "the explicit path signals the unpublished child"
    );
    assert!(
        !gateway.received_shutdown(Duration::from_millis(100)),
        "the disarmed drop sends no second signal"
    );
}

#[test]
fn explicit_candidate_shutdown_reports_a_delivery_failure() {
    let validated = {
        let gateway = validated_gateway("owned-key");
        gateway.validate("owned-key", 1_778_000_001, "2026-09-08T18:00:01Z")
    };
    let RecoveryOwnership::Owned(candidate) =
        RecoveryCandidate::authenticate(validated.pid(), validated)
    else {
        panic!("the validated file names the spawned child");
    };

    let error = candidate
        .shutdown()
        .expect_err("a dead child cannot accept the shutdown");
    assert!(
        matches!(error, shared_sidecar::ShutdownError::Io { .. }),
        "the explicit path reports the delivery failure: {error}"
    );
}

#[test]
fn dropping_a_candidate_signals_without_waiting_for_an_unresponsive_child() {
    // The fixture child lends its validatable pid; the hanging listener
    // answers the validation probe, then parks the shutdown connection.
    let gateway = validated_gateway("hanging-key");
    let reference = gateway.validate("hanging-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let hang = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the hanging fixture");
    let port = listener.local_addr().expect("the fixture address").port();
    std::thread::spawn({
        let hang = Arc::clone(&hang);
        move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0_u8; 1024];
                if hang.load(Ordering::SeqCst) {
                    let _ = stream.read(&mut buffer);
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
                while let Ok(read) = stream.read(&mut buffer) {
                    if read == 0 {
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
        }
    });
    let file = GatewayDiscoveryFile {
        port,
        api_key: "hanging-key".to_owned(),
        pid: reference.pid(),
        epoch: 1_778_000_001,
        version: "test".to_owned(),
        started_at: "2026-09-08T18:00:01Z".to_owned(),
    };
    let validated = ValidatedConnection::validate(file).expect("the hanging endpoint validates");
    hang.store(true, Ordering::SeqCst);
    let RecoveryOwnership::Owned(candidate) =
        RecoveryCandidate::authenticate(validated.pid(), validated)
    else {
        panic!("the validated file names the spawned child");
    };

    let started = Instant::now();
    drop(candidate);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "drop signals on a detached thread instead of waiting out the late-child budget"
    );
}

#[test]
fn a_mismatched_spawned_pid_never_claims_or_cleans_the_validated_process() {
    let mut gateway = validated_gateway("unowned-key");
    let validated = gateway.validate("unowned-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let run = tempfile::TempDir::new().expect("create run directory");
    gateway
        .gateway_discovery_file("unowned-key", 1_778_000_001, "2026-09-08T18:00:01Z")
        .write_to(run.path())
        .expect("retain the uncertain connection record");

    let RecoveryOwnership::Unowned(unowned) =
        RecoveryCandidate::authenticate(validated.pid() + 1, validated)
    else {
        panic!("a different validated pid cannot prove ownership");
    };
    drop(unowned);

    assert!(
        !gateway.received_shutdown(Duration::from_millis(100)),
        "an uncertain process never receives destructive cleanup"
    );
    assert!(
        shared_sidecar::gateway_discovery_file_path(run.path()).exists(),
        "uncertain cleanup retains the connection record"
    );
}

#[test]
fn successful_publication_disarms_late_child_cleanup() {
    let mut gateway = validated_gateway("published-key");
    let validated = gateway.validate("published-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let RecoveryOwnership::Owned(mut candidate) =
        RecoveryCandidate::authenticate(validated.pid(), validated)
    else {
        panic!("the validated file names the spawned child");
    };

    candidate.published();
    drop(candidate);

    assert!(
        !gateway.received_shutdown(Duration::from_millis(100)),
        "an authoritative published child remains running"
    );
}

#[test]
fn launched_recovery_retains_the_spawned_pid_and_releases_launch_lock() {
    let run = tempfile::TempDir::new().expect("create run directory");
    let decision = shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
        .expect("acquire launch election");
    let file = live_file(54_375, "candidate-key");
    let cancellation = CancellationToken::new();

    let recovery = launch_and_attach_cancellable_with(
        run.path(),
        Path::new("unused-gateway"),
        &cancellation,
        |_, _, _| Ok(decision),
        |_, _| Ok(4242),
        |_, _, _| Ok(file.clone()),
    )
    .expect("complete injected launch");

    match recovery {
        RecoveryLaunch::Launched {
            child_pid,
            file: observed,
        } => {
            assert_eq!(child_pid, 4242);
            assert_eq!(observed, file);
        }
        RecoveryLaunch::Attached(_) => panic!("the elected launcher retains child ownership"),
    }
    assert!(
        matches!(
            shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
                .expect("reacquire launch election"),
            shared_sidecar::LaunchDecision::Launch(_)
        ),
        "returning the candidate releases LaunchLock"
    );
}

#[test]
fn failed_validation_never_claims_or_removes_a_spawned_process_record() {
    let run = tempfile::TempDir::new().expect("create run directory");
    let file = GatewayDiscoveryFile {
        pid: super::dead_pid(),
        ..live_file(54_375, "unvalidated-key")
    };
    file.write_to(run.path()).expect("write uncertain record");
    let cancellation = CancellationToken::new();

    let result = validate_recovery(
        RecoveryLaunch::Launched {
            child_pid: file.pid,
            file,
        },
        &cancellation,
    );

    assert!(
        result.is_err(),
        "failed validation creates no owned candidate"
    );
    assert!(
        shared_sidecar::gateway_discovery_file_path(run.path()).exists(),
        "failed validation retains an uncertain process record"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_in_resolve_before_recovery() {
    let (entered, blocked) = mpsc::channel();
    let recoveries = Arc::new(AtomicUsize::new(0));
    let worker_recoveries = Arc::clone(&recoveries);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            test_identity(live_file(54_375, "stable-key")),
            |_, cancellation| {
                entered.send(()).expect("announce blocked resolve");
                let _ = cancellation.wait_timeout(Duration::from_secs(30));
                SupervisionProbe::Missing
            },
            |_| -> anyhow::Result<TestIdentity> {
                worker_recoveries.fetch_add(1, Ordering::SeqCst);
                anyhow::bail!("recovery must not start after cancellation")
            },
            |_, _| Ok::<(), anyhow::Error>(()),
            |delay, cancellation| cancellation.wait_timeout(delay),
            &cancellation,
        );
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the resolve phase blocks deterministically");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        recoveries.load(Ordering::SeqCst),
        0,
        "cancellation prevents every later recovery launch"
    );
}

#[test]
fn exit_wakes_the_supervision_wait_without_a_later_probe() {
    let (entered, blocked) = mpsc::channel();
    let probes = Arc::new(AtomicUsize::new(0));
    let worker_probes = Arc::clone(&probes);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            test_identity(live_file(54_375, "stable-key")),
            |current, _| {
                worker_probes.fetch_add(1, Ordering::SeqCst);
                SupervisionProbe::Replacement(current.clone())
            },
            |_| -> anyhow::Result<TestIdentity> { panic!("a healthy Gateway does not recover") },
            |_, _| Ok::<(), anyhow::Error>(()),
            |delay, cancellation| {
                entered.send(()).expect("announce supervision wait");
                cancellation.wait_timeout(delay)
            },
            &cancellation,
        );
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the supervision wait blocks deterministically");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        probes.load(Ordering::SeqCst),
        1,
        "cancellation prevents every later liveness probe"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_in_validation_before_publication() {
    let (entered, blocked) = mpsc::channel();
    let publications = Arc::new(AtomicUsize::new(0));
    let worker_publications = Arc::clone(&publications);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            test_identity(live_file(54_375, "stable-key")),
            |current, cancellation| {
                entered.send(()).expect("announce blocked validation");
                let _ = cancellation.wait_timeout(Duration::from_secs(30));
                SupervisionProbe::Replacement(current.clone())
            },
            |_| -> anyhow::Result<TestIdentity> {
                panic!("cancelled validation cannot start recovery")
            },
            |_, _| {
                worker_publications.fetch_add(1, Ordering::SeqCst);
                Ok::<(), anyhow::Error>(())
            },
            |delay, cancellation| cancellation.wait_timeout(delay),
            &cancellation,
        );
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the validation phase blocks deterministically");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        publications.load(Ordering::SeqCst),
        0,
        "cancelled validation cannot publish into the snapshot"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_in_health_wait_before_resolve() {
    let run = tempfile::TempDir::new().expect("tempdir");
    live_file(54_375, "stable-key")
        .write_to(run.path())
        .expect("write candidate");
    let run_dir = run.path().to_owned();
    let (entered, blocked) = mpsc::channel();
    let resolves = Arc::new(AtomicUsize::new(0));
    let worker_resolves = Arc::clone(&resolves);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        let result = wait_for_launched_file_cancellable_with(
            &run_dir,
            Duration::from_secs(30),
            &cancellation,
            |_, _, cancellation| {
                entered.send(()).expect("announce blocked health wait");
                let _ = cancellation.wait_timeout(Duration::from_secs(30));
                Err(shared_sidecar::HealthError::Cancelled)
            },
            |_, _| {
                worker_resolves.fetch_add(1, Ordering::SeqCst);
                Ok(Resolution::Absent)
            },
        );
        assert!(result.is_err(), "the cancelled health wait is rejected");
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the health-wait phase blocks deterministically");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        resolves.load(Ordering::SeqCst),
        0,
        "cancelled health waiting cannot start a later resolve"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_in_launch_race_before_spawn() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let decision = shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
        .expect("acquire a launch decision");
    let run_dir = run.path().to_owned();
    let (entered, blocked) = mpsc::channel();
    let launches = Arc::new(AtomicUsize::new(0));
    let worker_launches = Arc::clone(&launches);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        let mut decision = Some(decision);
        let result = launch_and_attach_cancellable_with(
            &run_dir,
            Path::new("unused-gateway"),
            &cancellation,
            |_, _, cancellation| {
                entered.send(()).expect("announce blocked launch race");
                let _ = cancellation.wait_timeout(Duration::from_secs(30));
                Ok(decision.take().expect("one launch decision"))
            },
            |_, _| {
                worker_launches.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            },
            |_, _, _| anyhow::bail!("the cancelled launch cannot wait for health"),
        );
        assert!(result.is_err(), "the cancelled launch race is rejected");
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the launch race blocks deterministically");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        launches.load(Ordering::SeqCst),
        0,
        "cancelled launch racing cannot create a process"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_inside_launch_without_process_creation() {
    let run = tempfile::TempDir::new().expect("tempdir");
    let decision = shared_sidecar::launch_or_attach(run.path(), Duration::from_secs(1))
        .expect("acquire a launch decision");
    let run_dir = run.path().to_owned();
    let (entered, blocked) = mpsc::channel();
    let launches = Arc::new(AtomicUsize::new(0));
    let worker_launches = Arc::clone(&launches);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        let mut decision = Some(decision);
        let result = launch_and_attach_cancellable_with(
            &run_dir,
            Path::new("unused-gateway"),
            &cancellation,
            |_, _, _| Ok(decision.take().expect("one launch decision")),
            |_, cancellation| {
                entered.send(()).expect("announce blocked launch");
                if cancellation.wait_timeout(Duration::from_secs(30)) {
                    return Err(std::io::Error::from(std::io::ErrorKind::Interrupted));
                }
                worker_launches.fetch_add(1, Ordering::SeqCst);
                Ok(42)
            },
            |_, _, _| anyhow::bail!("the cancelled launch cannot wait for health"),
        );
        assert!(result.is_err(), "the cancelled launch is rejected");
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the process launch blocks deterministically inside its effect gate");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        launches.load(Ordering::SeqCst),
        0,
        "the cancelled launch has no post-cancel effect"
    );
}

#[test]
fn exit_joins_a_supervisor_blocked_inside_publication_without_replacement() {
    let (entered, blocked) = mpsc::channel();
    let publications = Arc::new(AtomicUsize::new(0));
    let worker_publications = Arc::clone(&publications);
    let finished = Arc::new(AtomicBool::new(false));
    let worker_finished = Arc::clone(&finished);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        let result = run_effect_if_active(&cancellation, "gateway publication", |cancellation| {
            entered.send(()).expect("announce blocked publication");
            if cancellation.wait_timeout(Duration::from_secs(30)) {
                anyhow::bail!("gateway publication was cancelled");
            }
            worker_publications.fetch_add(1, Ordering::SeqCst);
            Ok(())
        });
        assert!(result.is_err(), "the cancelled publication is rejected");
        worker_finished.store(true, Ordering::SeqCst);
    })
    .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("snapshot publication blocks deterministically inside its effect gate");

    assert_bounded_supervisor_shutdown(supervisor, &finished);
    assert_eq!(
        publications.load(Ordering::SeqCst),
        0,
        "cancellation prevents authoritative snapshot replacement"
    );
}
