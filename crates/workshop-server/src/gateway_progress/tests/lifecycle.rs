use super::*;

fn operation_event_json(operation: u64, path: &str, state: &serde_json::Value) -> String {
    serde_json::json!({
        "operation": operation,
        "path": path,
        "label": path,
        "state": state,
    })
    .to_string()
}

#[tokio::test]
async fn a_multi_stage_operation_detaches_only_when_the_operation_finishes() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    let subscriber = spawn(binding(&base_url), Arc::clone(&hub), GatewayHealth::new());

    wait_for_connections(&mock, 1).await;
    mock.send(event_json(
        "loading-profile",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    snapshot_where(&hub, |snapshot| snapshot.len() == 1).await;
    mock.send(event_json(
        "loading-profile",
        &serde_json::json!({"Finished": {"ok": true}}),
    ));
    snapshot_where(&hub, |snapshot| {
        snapshot.len() == 1
            && snapshot[0]
                .nodes
                .iter()
                .any(|node| node.path == "loading-profile" && node.finished)
    })
    .await;
    mock.send(operation_event_json(
        8,
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    mock.send(event_json(
        "starting-models",
        &serde_json::json!({"Begun": {"weight": 5.0}}),
    ));
    snapshot_where(&hub, |snapshot| {
        snapshot.len() == 2
            && snapshot.iter().any(|operation| {
                operation
                    .nodes
                    .iter()
                    .any(|node| node.path == "starting-models" && !node.finished)
            })
    })
    .await;
    mock.send(operation_event_json(
        7,
        "",
        &serde_json::json!("OperationFinished"),
    ));
    let remaining = snapshot_where(&hub, |snapshot| snapshot.len() == 1).await;
    assert_eq!(remaining[0].nodes[0].path, "download");

    assert_eq!(
        mock.connections.load(Ordering::Relaxed),
        1,
        "operation completion detaches only its import, not the open SSE stream"
    );
    subscriber.shutdown().await;
}
