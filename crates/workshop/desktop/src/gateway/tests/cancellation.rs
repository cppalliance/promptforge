//! Exit cancellation of a supervisor blocked inside each recovery phase.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use gateway_api_discovery::Resolution;

use super::{TestIdentity, assert_bounded_supervisor_shutdown, live_file, test_identity};
use crate::gateway::supervisor::{
    GatewaySupervisor, SupervisionProbe, SystemClock, launch_and_attach_cancellable_with,
    run_effect_if_active, run_supervision, wait_for_launched_file_cancellable_with,
};

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
            &SystemClock,
            |_, _, cancellation| {
                entered.send(()).expect("announce blocked health wait");
                let _ = cancellation.wait_timeout(Duration::from_secs(30));
                Err(gateway_api_discovery::HealthError::Cancelled)
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
    let decision = gateway_api_discovery::launch_or_attach(run.path(), Duration::from_secs(1))
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
    let decision = gateway_api_discovery::launch_or_attach(run.path(), Duration::from_secs(1))
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
