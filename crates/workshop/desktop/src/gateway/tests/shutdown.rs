//! Bounded Gateway supervisor and late-publication shutdown coverage.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use super::validated_gateway;
use crate::gateway::supervisor::{
    GatewaySupervisor, SUPERVISOR_SHUTDOWN_BUDGET, SupervisedGatewayIdentity, SupervisionProbe,
    SupervisorShutdown, run_supervision,
};

/// How long a test waits for a teardown that must finish while its worker
/// is still parked. Shorter than [`SUPERVISOR_SHUTDOWN_BUDGET`], so a
/// shutdown that ignores its injected budget and waits out the production
/// one fails instead of passing late.
const TEARDOWN_DEADLINE: Duration = Duration::from_secs(2);
const _: () = assert!(TEARDOWN_DEADLINE.as_millis() < SUPERVISOR_SHUTDOWN_BUDGET.as_millis());

/// A shutdown budget longer than [`TEARDOWN_DEADLINE`], so a teardown that
/// waits it out fails instead of passing late.
const UNREACHED_BUDGET: Duration = Duration::from_secs(3600);

/// A supervised identity reduced to its process boot.
#[derive(Clone)]
struct Boot(u64);

impl SupervisedGatewayIdentity for Boot {
    fn same_boot(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

/// Runs `teardown` on its own thread while the caller keeps the worker
/// parked. A teardown that waits for the worker cannot finish, so it
/// reports a timeout instead of hanging the test.
fn teardown_while_parked<T: Send + 'static>(
    teardown: impl FnOnce() -> T + Send + 'static,
) -> Result<T, mpsc::RecvTimeoutError> {
    let (finished, outcome) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = finished.send(teardown());
    });
    outcome.recv_timeout(TEARDOWN_DEADLINE)
}

#[test]
fn supervisor_shutdown_reports_a_worker_panic_without_unwinding_teardown() {
    let supervisor = GatewaySupervisor::spawn(|_| panic!("injected supervisor panic"))
        .expect("spawn test supervisor");

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Panicked);
}

#[test]
fn supervisor_shutdown_detaches_an_uncooperative_worker_at_one_deadline() {
    let (release, blocked) = mpsc::channel();
    let supervisor = GatewaySupervisor::spawn_with_budget(Duration::from_millis(50), move |_| {
        let _ = blocked.recv();
    })
    .expect("spawn test supervisor");

    assert_eq!(
        teardown_while_parked(move || supervisor.shutdown()),
        Ok(SupervisorShutdown::Detached),
        "an uncooperative worker cannot outlive the shutdown budget"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn stop_signaling_never_waits_for_an_in_progress_effect_gate() {
    let (entered, blocked) = mpsc::channel();
    let (release, await_release) = mpsc::channel();
    let supervisor =
        GatewaySupervisor::spawn_with_budget(Duration::from_millis(50), move |cancellation| {
            let _ = cancellation.run_if_active(|| {
                entered.send(()).expect("announce admitted effect");
                let _ = await_release.recv();
            });
        })
        .expect("spawn test supervisor");
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("the worker enters the effect gate");

    assert_eq!(
        teardown_while_parked(move || supervisor.shutdown()),
        Ok(SupervisorShutdown::Detached),
        "the stop signal cannot join an admitted effect before the bounded completion wait"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn a_spurious_completion_wake_cannot_trigger_a_blocking_join() {
    let (release, blocked) = mpsc::channel();
    let supervisor = GatewaySupervisor::spawn_with_budget(Duration::from_millis(50), move |_| {
        let _ = blocked.recv();
    })
    .expect("spawn test supervisor");
    let wake = supervisor.completion_waker_for_test();
    let waking = Arc::new(AtomicBool::new(true));
    let waker = std::thread::spawn({
        let waking = Arc::clone(&waking);
        move || {
            while waking.load(Ordering::SeqCst) {
                wake();
                std::thread::yield_now();
            }
        }
    });

    let outcome = teardown_while_parked(move || supervisor.shutdown());
    waking.store(false, Ordering::SeqCst);
    waker.join().expect("the waker thread stops");

    assert_eq!(
        outcome,
        Ok(SupervisorShutdown::Detached),
        "a wake without completion must keep waiting only to the original deadline"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn dropping_a_supervisor_signals_and_detaches_without_waiting() {
    let (release, blocked) = mpsc::channel();
    let supervisor = GatewaySupervisor::spawn_with_budget(UNREACHED_BUDGET, move |_| {
        let _ = blocked.recv();
    })
    .expect("spawn test supervisor");

    assert_eq!(
        teardown_while_parked(move || drop(supervisor)),
        Ok(()),
        "Drop signals and detaches instead of waiting out the shutdown budget"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn a_shut_down_supervisor_launches_nothing_when_its_gateway_later_disappears() {
    let (entered, healthy) = mpsc::channel();
    let gone = Arc::new(AtomicBool::new(false));
    let worker_gone = Arc::clone(&gone);
    let launches = Arc::new(AtomicUsize::new(0));
    let worker_launches = Arc::clone(&launches);
    let supervisor = GatewaySupervisor::spawn(move |cancellation| {
        run_supervision(
            Boot(1),
            |current, _| {
                if worker_gone.load(Ordering::SeqCst) {
                    SupervisionProbe::Missing
                } else {
                    SupervisionProbe::Replacement(current.clone())
                }
            },
            |_| -> anyhow::Result<Boot> {
                worker_launches.fetch_add(1, Ordering::SeqCst);
                Ok(Boot(2))
            },
            |_, _| Ok::<(), anyhow::Error>(()),
            |delay, cancellation| {
                let _ = entered.send(());
                cancellation.wait_timeout(delay)
            },
            &cancellation,
        );
    })
    .expect("spawn test supervisor");
    healthy
        .recv_timeout(Duration::from_secs(1))
        .expect("the supervisor observes the healthy gateway");
    gone.store(true, Ordering::SeqCst);

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Joined);
    assert_eq!(
        launches.load(Ordering::SeqCst),
        0,
        "quit stops the gateway only after its supervisor, so nothing relaunches it"
    );
}

#[test]
fn shutdown_revokes_publication_before_waking_a_late_worker() {
    let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
    let server = workshop_server_api::fixtures::spawn(workshop_server_api::Config {
        gateway: workshop_server_api::GatewayConfig {
            base_url: "http://127.0.0.1:54375".to_owned(),
            api_key: "old-key".to_owned(),
        },
        server: workshop_server_api::ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            state_dir: state_dir.path().to_owned(),
        },
        agents: workshop_server_api::AgentsConfig::default(),
    })
    .expect("spawn Workshop");
    let updater = server.gateway_updater();
    let worker_updater = updater.clone();
    let replacement_gateway = validated_gateway("replacement-key");
    let replacement =
        replacement_gateway.validate("replacement-key", 1_778_000_001, "2026-09-08T18:00:01Z");
    let rejected = Arc::new(AtomicBool::new(false));
    let worker_rejected = Arc::clone(&rejected);
    let supervisor = GatewaySupervisor::spawn_with_publication(updater, move |cancellation| {
        let _ = cancellation.wait_timeout(Duration::from_secs(30));
        worker_rejected.store(
            matches!(
                worker_updater.replace_sidecar(&replacement),
                Err(workshop_server_api::GatewayPublicationError::PublicationClosed)
            ),
            Ordering::SeqCst,
        );
    })
    .expect("spawn late publisher");

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Joined);
    assert!(
        rejected.load(Ordering::SeqCst),
        "the worker wakes only after permanent publication closure"
    );
    server.shutdown().expect("stop Workshop");
}
