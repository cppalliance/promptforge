//! Heartbeat refresh-on-reconnect: a transition to reachable repopulates the catalog and profile state.

use super::*;

#[tokio::test]
async fn a_reconnect_pushes_the_refreshed_catalog() {
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut status_rx = status.subscribe();
    let mut catalog_rx = catalog.subscribe();
    let (heartbeat, _health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Gateway unreachable"
    );
    healthy.store(true, Ordering::Relaxed);
    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Connected to gateway"
    );
    let push: CatalogPush = tokio::time::timeout(Duration::from_secs(5), catalog_rx.recv())
        .await
        .expect("the refreshed catalog arrives within the deadline")
        .expect("the catalog bus is open");
    assert_eq!(
        push.models,
        serde_json::json!([{"id": "test-model", "object": "model", "owned_by": "promptforge"}])
            .as_array()
            .expect("the fixture is an array")
            .clone(),
        "the push includes every chat-capable gateway model"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_reconnect_whose_refresh_is_declined_pushes_no_catalog() {
    // No /v1/models route: the refresh is declined with a 404, and a
    // declined refresh is skipped rather than pushed - pushing it
    // would empty pickers that still hold a usable list.
    let healthy = Arc::new(AtomicBool::new(false));
    let (health, mut probes) = health_only(Arc::clone(&healthy));
    let base_url = serve(health).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut status_rx = status.subscribe();
    let mut catalog_rx = catalog.subscribe();
    let (heartbeat, _health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Gateway unreachable"
    );
    healthy.store(true, Ordering::Relaxed);
    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Connected to gateway"
    );
    // Two more probes: the reconnect's declined refresh and one healthy
    // retry have both finished.
    await_probes(&mut probes, 2).await;
    let quiet = catalog_rx.try_recv();
    assert!(
        matches!(quiet, Err(broadcast::error::TryRecvError::Empty)),
        "a declined refresh is skipped, not pushed, got {quiet:?}"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_down_to_up_transition_publishes_a_populated_snapshot() {
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut status_rx = status.subscribe();
    let (heartbeat, _health, menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Gateway unreachable"
    );
    healthy.store(true, Ordering::Relaxed);
    let populated = snapshot_where(&menu, |snapshot| !snapshot.profiles.is_empty()).await;
    assert_eq!(populated.profiles, ["coding", "main"]);
    assert_eq!(populated.active.as_deref(), Some("main"));
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_gateway_without_profile_support_publishes_an_empty_list() {
    // Only /health exists: the profile endpoints answer 404, which
    // is a state, not an error - the reconnect publishes an empty
    // list rather than keeping the stale names.
    let healthy = Arc::new(AtomicBool::new(false));
    let (health, _probes) = health_only(Arc::clone(&healthy));
    let base_url = serve(health).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut status_rx = status.subscribe();
    let (heartbeat, _health, menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Gateway unreachable"
    );
    menu.set_profiles(vec!["stale".to_string()], Some("stale".to_string()));
    healthy.store(true, Ordering::Relaxed);
    let emptied = snapshot_where(&menu, |snapshot| snapshot.profiles.is_empty()).await;
    assert_eq!(emptied.active, None, "the stale active profile clears");
    heartbeat.shutdown().await;
}
