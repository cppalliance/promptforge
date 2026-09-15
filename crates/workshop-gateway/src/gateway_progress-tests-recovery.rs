use super::*;

#[tokio::test]
async fn an_endpoint_replacement_moves_the_progress_subscription_immediately() {
    let original = Arc::new(MockProgress::new());
    let original_url = spawn_gateway(Arc::clone(&original).router()).await;
    let replacement = Arc::new(MockProgress::new());
    let replacement_url = spawn_gateway(Arc::clone(&replacement).router()).await;
    let gateway = binding(&original_url);
    let hub = Arc::new(ProgressHub::new());
    let subscriber = spawn(gateway.clone(), Arc::clone(&hub), GatewayHealth::new());

    wait_for_connections(&original, 1).await;
    gateway
        .replace(&replacement_url, "")
        .expect("the replacement publishes");
    wait_for_connections(&replacement, 1).await;
    replacement.send(event_json(
        "replacement-download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    let snapshot = snapshot_where(&hub, |snapshot| {
        snapshot.iter().any(|operation| {
            operation
                .nodes
                .iter()
                .any(|node| node.path == "replacement-download")
        })
    })
    .await;
    assert_eq!(snapshot.len(), 1, "only the replacement import remains");
    assert_eq!(
        original.connections.load(Ordering::Relaxed),
        1,
        "the old endpoint is never retried after publication"
    );
    subscriber.shutdown().await;
}
