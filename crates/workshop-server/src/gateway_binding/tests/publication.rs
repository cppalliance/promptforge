use super::*;

use std::sync::PoisonError;
use std::time::{Duration, Instant};

#[test]
fn cancellation_wakes_a_replacement_contending_on_the_publication_lock() {
    let binding = GatewayBinding::new("http://127.0.0.1:54375", "old-key").expect("binding builds");
    let original = binding.snapshot();
    let gateway = crate::test_gateway::ValidatedGateway::spawn("new-key");
    let validated =
        validated_connection(&gateway, "new-key", 1_778_000_001, "2026-09-07T18:00:01Z");
    let held = binding
        .replacement
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
