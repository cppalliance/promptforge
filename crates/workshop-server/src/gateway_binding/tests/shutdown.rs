use super::*;

use std::{sync::mpsc, time::Duration};

#[derive(Clone, Copy)]
enum PublicationOrder {
    QuitFirst,
    ReplacementFirst,
}

fn assert_shutdown_target(order: PublicationOrder) {
    let mut original_gateway = crate::test_gateway::ValidatedGateway::spawn("original-key");
    let mut replacement_gateway = crate::test_gateway::ValidatedGateway::spawn("replacement-key");
    let original = validated_connection(
        &original_gateway,
        "original-key",
        1_778_000_001,
        "2026-09-07T18:00:01Z",
    );
    let replacement = validated_connection(
        &replacement_gateway,
        "replacement-key",
        1_778_000_002,
        "2026-09-07T18:00:02Z",
    );
    let binding = GatewayBinding::new_with_identity(
        &format!("http://127.0.0.1:{}", original.port()),
        original.api_key(),
        Some(original.clone()),
    )
    .expect("binding builds");
    let updater = binding.updater();
    let (publish, await_publish) = mpsc::sync_channel(0);
    let (completed, await_completion) = mpsc::sync_channel(0);
    let (quit, await_quit) = mpsc::sync_channel(0);

    let shutdown_requested = std::thread::scope(|scope| {
        let publish_updater = updater.clone();
        let worker = scope.spawn(move || {
            await_publish.recv().expect("publication is released");
            publish_updater
                .replace_sidecar(&replacement)
                .expect("the replacement publishes");
            completed.send(()).expect("publication is observed");
        });
        let shutdown_updater = updater;
        let shutdown = scope.spawn(move || {
            await_quit.recv().expect("quit is released");
            shutdown_updater
                .request_shutdown()
                .expect("one coherent generation accepts shutdown")
        });

        let result = match order {
            PublicationOrder::QuitFirst => {
                quit.send(()).expect("release quit");
                let result = shutdown.join().expect("shutdown does not panic");
                publish.send(()).expect("release publication");
                await_completion.recv().expect("observe publication");
                result
            }
            PublicationOrder::ReplacementFirst => {
                publish.send(()).expect("release publication");
                await_completion.recv().expect("observe publication");
                quit.send(()).expect("release quit");
                shutdown.join().expect("shutdown does not panic")
            }
        };
        worker.join().expect("publisher does not panic");
        result
    });

    let original_hit = original_gateway.received_shutdown(Duration::from_millis(250));
    let replacement_hit = replacement_gateway.received_shutdown(Duration::from_millis(250));
    let expect_original = matches!(order, PublicationOrder::QuitFirst);
    assert_eq!(
        (shutdown_requested, original_hit, replacement_hit),
        (true, expect_original, !expect_original),
        "quit targets only the identity current at its controlled snapshot load"
    );
}

#[test]
fn quit_before_replacement_publication_targets_original_identity() {
    assert_shutdown_target(PublicationOrder::QuitFirst);
}

#[test]
fn quit_after_replacement_publication_targets_replacement_identity() {
    assert_shutdown_target(PublicationOrder::ReplacementFirst);
}

#[test]
fn configured_gateway_has_no_shutdown_authority() {
    let binding =
        GatewayBinding::new("http://192.0.2.10:8080", "configured-key").expect("binding builds");

    assert!(
        !binding
            .updater()
            .request_shutdown()
            .expect("a configured Gateway is an intentional no-op"),
        "the absence of validated local identity denies shutdown authority"
    );
}
