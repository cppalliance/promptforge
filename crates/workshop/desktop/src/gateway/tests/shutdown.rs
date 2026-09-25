//! Bounded Gateway supervisor and late-publication shutdown coverage.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::validated_gateway;
use crate::gateway::supervisor::{
    GatewaySupervisor, SupervisedGatewayIdentity, SupervisionProbe, SupervisorShutdown,
    run_supervision,
};

/// A supervised identity reduced to its process boot.
#[derive(Clone)]
struct Boot(u64);

impl SupervisedGatewayIdentity for Boot {
    fn same_boot(&self, other: &Self) -> bool {
        self.0 == other.0
    }
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
    let started = Instant::now();

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Detached);
    assert!(
        started.elapsed() < Duration::from_millis(250),
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
    let started = Instant::now();

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Detached);
    assert!(
        started.elapsed() < Duration::from_millis(250),
        "the stop signal cannot join an admitted effect before the bounded completion wait"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn a_spurious_completion_wake_cannot_trigger_a_blocking_join() {
    let supervisor = GatewaySupervisor::spawn_with_budget(Duration::from_millis(50), move |_| {
        std::thread::sleep(Duration::from_millis(200));
    })
    .expect("spawn test supervisor");
    supervisor.wake_completion_for_test();
    let started = Instant::now();

    assert_eq!(supervisor.shutdown(), SupervisorShutdown::Detached);
    assert!(
        started.elapsed() < Duration::from_millis(150),
        "a wake without completion must keep waiting only to the original deadline"
    );
}

#[test]
fn dropping_a_supervisor_signals_and_detaches_without_waiting() {
    let (release, blocked) = mpsc::channel();
    let supervisor = GatewaySupervisor::spawn_with_budget(Duration::from_secs(1), move |_| {
        let _ = blocked.recv();
    })
    .expect("spawn test supervisor");
    let started = Instant::now();

    drop(supervisor);

    assert!(
        started.elapsed() < Duration::from_millis(250),
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
