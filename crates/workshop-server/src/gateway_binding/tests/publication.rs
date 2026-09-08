use super::*;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError};
use std::time::{Duration, Instant};

#[test]
fn cancellation_wakes_a_replacement_contending_on_the_publication_lock() {
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let original = binding.snapshot();
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let validated =
        validated_connection(&gateway, "new-key", 1_778_000_001, "2026-09-07T18:00:01Z");
    let held = binding
        .publication
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    let worker_binding = binding.clone();
    let cancellation = shared_sidecar::CancellationToken::new();
    let worker_cancellation = cancellation.clone();
    let (entered, blocked) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let base_url = format!("http://127.0.0.1:{}", validated.port());
        let api_key = validated.api_key().to_owned();
        worker_binding.replace_with_identity_cancellable_with_wait(
            &base_url,
            &api_key,
            validated,
            &worker_cancellation,
            |cancellation, delay| {
                entered
                    .send(())
                    .expect("announce publication lock contention");
                cancellation.wait_timeout(delay)
            },
        )
    });
    blocked
        .recv_timeout(Duration::from_secs(1))
        .expect("replacement blocks inside the real publication boundary");

    let started = Instant::now();
    cancellation.cancel();
    let published = worker
        .join()
        .expect("replacement worker joins")
        .expect("replacement build succeeds");
    drop(held);

    assert!(
        started.elapsed() < Duration::from_millis(250),
        "cancellation wakes publication lock contention"
    );
    assert!(!published, "the cancelled replacement is not published");
    assert_eq!(
        binding.snapshot().generation(),
        original.generation(),
        "the authoritative snapshot remains the pre-cancel generation"
    );
}

#[test]
fn publication_close_is_permanent_for_every_updater_clone() {
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let original = binding.snapshot();
    let changed = binding.subscribe();
    let updater = binding.updater();
    let clone = updater.clone();
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let validated =
        validated_connection(&gateway, "new-key", 1_778_000_001, "2026-09-07T18:00:01Z");

    updater.close_publication();
    for updater in [updater, clone, binding.updater()] {
        let error = updater
            .replace_sidecar(&validated)
            .expect_err("a closed binding rejects every updater clone");
        assert!(matches!(error, GatewayPublicationError::PublicationClosed));
    }
    let cancellation = shared_sidecar::CancellationToken::new();
    cancellation.cancel();
    assert!(matches!(
        binding
            .updater()
            .replace_sidecar_cancellable(&validated, &cancellation),
        Err(GatewayPublicationError::PublicationClosed)
    ));

    assert_eq!(
        binding.snapshot().generation(),
        original.generation(),
        "rejected publication does not advance or store a generation"
    );
    assert!(
        !changed
            .has_changed()
            .expect("the watch channel remains live"),
        "rejected publication does not notify subscribers"
    );
}

#[test]
fn concurrent_publishers_cannot_cross_the_permanent_close_linearization_point() {
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let validated = Arc::new(validated_connection(
        &gateway,
        "new-key",
        1_778_000_001,
        "2026-09-07T18:00:01Z",
    ));
    let closed = Arc::new(AtomicBool::new(false));

    std::thread::scope(|scope| {
        for _ in 0..8 {
            let updater = binding.updater();
            let validated = Arc::clone(&validated);
            let closed = Arc::clone(&closed);
            scope.spawn(move || {
                while !closed.load(Ordering::SeqCst) {
                    let _ = updater.replace_sidecar(&validated);
                }
                for _ in 0..32 {
                    assert!(matches!(
                        updater.replace_sidecar(&validated),
                        Err(GatewayPublicationError::PublicationClosed)
                    ));
                }
            });
        }
        binding.updater().close_publication();
        let closed_generation = binding.generation();
        closed.store(true, Ordering::SeqCst);
        std::thread::yield_now();
        assert_eq!(
            binding.generation(),
            closed_generation,
            "no publisher advances the generation after close returns"
        );
    });
}
