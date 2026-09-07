use super::*;

/// Catalog route that accepts only the replacement sidecar bearer.
async fn replacement_models(headers: axum::http::HeaderMap) -> Response {
    if headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some("Bearer replacement-key")
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    mock_models().await
}

#[tokio::test]
async fn a_replaced_endpoint_wakes_the_heartbeat_and_refreshes_with_its_new_key() {
    let replacement = serve(
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/v1/models", get(replacement_models))
            .route("/admin/profiles", get(mock_profiles))
            .route("/admin/status", get(mock_profile_status)),
    )
    .await;
    let gateway = GatewayBinding::new("http://127.0.0.1:1", "old-key").expect("binding builds");
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let health = GatewayHealth::new();
    let mut status_rx = status.subscribe();
    let mut catalog_rx = catalog.subscribe();
    let heartbeat = spawn(
        gateway.clone(),
        Push::new(status, catalog, menu),
        health.clone(),
        Duration::from_secs(60),
        test_backoff(),
    );

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Gateway unreachable"
    );
    gateway
        .replace(&replacement, "replacement-key")
        .expect("the replacement publishes");
    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Connected to gateway",
        "publication wakes the heartbeat without waiting out its backoff"
    );
    let models = tokio::time::timeout(Duration::from_secs(5), catalog_rx.recv())
        .await
        .expect("the replacement catalog arrives")
        .expect("the catalog channel stays open");
    assert_eq!(models.models[0]["id"], "test-model");
    assert!(health.is_reachable());
    heartbeat.shutdown().await;
}
