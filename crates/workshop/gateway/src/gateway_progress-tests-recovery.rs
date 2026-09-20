//! Progress subscription recovery: an endpoint replacement moves the subscription at once.

use super::*;

#[tokio::test]
async fn an_endpoint_replacement_moves_the_progress_subscription_immediately() {
    let original = Arc::new(MockProgress::new());
    let original_url = spawn_gateway(Arc::clone(&original).router()).await;
    let replacement = Arc::new(MockProgress::new());
    let replacement_url = spawn_gateway(Arc::clone(&replacement).router()).await;
    let gateway = binding(&original_url);
    let recorder = recorder();
    let subscriber = spawn_with_timing(
        gateway.clone(),
        recorder.push.clone(),
        GatewayHealth::new(),
        FAST_TIMING,
    );

    wait_for_connections(&original, 1).await;
    original.send(true, "original-download");
    recorder.frames_where(|frames| frames.len() == 1).await;

    gateway
        .replace(&replacement_url, "")
        .expect("the replacement publishes");
    let pushed = recorder.frames_where(|frames| frames.len() == 2).await;
    assert_eq!(
        pushed[1],
        ("Ready".to_owned(), false),
        "the old endpoint's progress is stale the moment the binding moves"
    );
    wait_for_connections(&replacement, 1).await;
    replacement.send(true, "replacement-download");
    let pushed = recorder.frames_where(|frames| frames.len() == 3).await;
    assert_eq!(
        pushed[2],
        ("replacement-download".to_owned(), true),
        "the replacement's snapshots drive the bar"
    );
    assert_eq!(
        original.connections.load(Ordering::Relaxed),
        1,
        "the old endpoint is never retried after publication"
    );
    subscriber.shutdown().await;
}
