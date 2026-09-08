//! Bounded Gateway supervisor and late-publication shutdown coverage.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use super::validated_gateway;
use crate::gateway::supervisor::{GatewaySupervisor, SupervisorShutdown};

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
fn dropping_a_supervisor_uses_the_same_bounded_detach_path() {
    let (release, blocked) = mpsc::channel();
    let supervisor = GatewaySupervisor::spawn_with_budget(Duration::from_millis(50), move |_| {
        let _ = blocked.recv();
    })
    .expect("spawn test supervisor");
    let started = Instant::now();

    drop(supervisor);

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "Drop cannot wait beyond the supervisor shutdown budget"
    );
    release.send(()).expect("release the detached test worker");
}

#[test]
fn shutdown_revokes_publication_before_waking_a_late_worker() {
    let state_dir = tempfile::TempDir::new().expect("create Workshop state directory");
    let server = workshop_server::fixtures::spawn(workshop_server::Config {
        gateway: workshop_server::GatewayConfig {
            base_url: "http://127.0.0.1:54375".to_owned(),
            api_key: "old-key".to_owned(),
        },
        server: workshop_server::ServerConfig {
            bind: "127.0.0.1:0".to_owned(),
            open_browser: false,
            state_dir: state_dir.path().to_owned(),
        },
        agents: workshop_server::AgentsConfig::default(),
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
                Err(workshop_server::GatewayPublicationError::PublicationClosed)
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
