use super::*;

#[tokio::test]
async fn publishing_with_no_subscribers_is_a_no_op() {
    let bus = CatalogBus::new();
    bus.publish(vec![serde_json::json!({"id": "test-model"})]);
}

#[test]
fn the_newest_push_is_retained_for_the_connect_snapshot() {
    let bus = CatalogBus::new();
    assert!(bus.latest().is_none(), "an untouched bus has no snapshot");
    bus.publish(vec![serde_json::json!({"id": "old"})]);
    bus.publish(vec![serde_json::json!({"id": "new"})]);
    let latest = bus.latest().expect("the bus retains the newest push");
    assert_eq!(
        latest.models[0]["id"], "new",
        "a session connecting now snapshots the newest catalog"
    );
}

#[tokio::test]
async fn a_lagged_receiver_skips_ahead_instead_of_blocking() {
    let bus = CatalogBus::new();
    let mut receiver = bus.subscribe();
    for index in 0..=CATALOG_CHANNEL_CAPACITY {
        bus.publish(vec![serde_json::json!({"id": format!("model-{index}")})]);
    }
    match receiver.recv().await {
        Err(broadcast::error::RecvError::Lagged(1)) => {}
        other => panic!("expected a lag report of one, got {other:?}"),
    }
    let resumed = receiver.recv().await.expect("the ring still holds pushes");
    assert_eq!(
        resumed.models[0]["id"], "model-1",
        "receiving resumes at the oldest retained push"
    );
}
