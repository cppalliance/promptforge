use super::*;

/// Startup state whose health is continuously true while its catalog
/// becomes ready later.
struct DelayedCatalog {
    ready: AtomicBool,
    requests: AtomicUsize,
}

/// Startup state whose catalog is ready before both profile endpoints.
struct DelayedProfiles {
    ready: AtomicBool,
    list_requests: AtomicUsize,
    status_requests: AtomicUsize,
}

/// A catalog that is empty until the test publishes readiness.
async fn delayed_models(State(state): State<Arc<DelayedCatalog>>) -> Response {
    state.requests.fetch_add(1, Ordering::Relaxed);
    let body = if state.ready.load(Ordering::Relaxed) {
        CATALOG
    } else {
        r#"{"object":"list","data":[]}"#
    };
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

async fn delayed_profiles(State(state): State<Arc<DelayedProfiles>>) -> Response {
    state.list_requests.fetch_add(1, Ordering::Relaxed);
    let body = if state.ready.load(Ordering::Relaxed) {
        r#"{"profiles":["coding","main"]}"#
    } else {
        r#"{"profiles":[]}"#
    };
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

async fn delayed_profile_status(State(state): State<Arc<DelayedProfiles>>) -> Response {
    state.status_requests.fetch_add(1, Ordering::Relaxed);
    let body = if state.ready.load(Ordering::Relaxed) {
        r#"{"profile":"main","models":["test-model"]}"#
    } else {
        r#"{"profile":null,"models":[]}"#
    };
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body,
    )
        .into_response()
}

#[tokio::test]
async fn the_initial_connect_populates_the_profile_state() {
    let healthy = Arc::new(AtomicBool::new(true));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let (heartbeat, _health, menu, _backoff) = heartbeat_on(&base_url, &status, &catalog);

    let populated = snapshot_where(&menu, |snapshot| !snapshot.profiles.is_empty()).await;
    assert_eq!(populated.profiles, ["coding", "main"]);
    assert_eq!(populated.active.as_deref(), Some("main"));
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn the_initial_connect_pushes_the_catalog_and_readies_chat() {
    let healthy = Arc::new(AtomicBool::new(true));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut catalog_rx = catalog.subscribe();
    let (heartbeat, _health, menu, _backoff) = heartbeat_on(&base_url, &status, &catalog);

    let push: CatalogPush = tokio::time::timeout(Duration::from_secs(5), catalog_rx.recv())
        .await
        .expect("the boot catalog arrives within the deadline")
        .expect("the catalog bus is open");
    assert_eq!(
        push.models,
        serde_json::json!([{"id": "test-model", "object": "model", "owned_by": "promptforge"}])
            .as_array()
            .expect("the fixture is an array")
            .clone(),
        "the push carries every chat-capable gateway model"
    );
    let ready = snapshot_where(&menu, |snapshot| snapshot.chat_ready).await;
    assert_eq!(
        ready.selected_model.as_deref(),
        Some("test-model"),
        "boot restores a selection without any user interaction"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_healthy_gateway_retries_refresh_until_its_catalog_is_ready() {
    let state = Arc::new(DelayedCatalog {
        ready: AtomicBool::new(false),
        requests: AtomicUsize::new(0),
    });
    let base_url = serve(
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/v1/models", get(delayed_models))
            .route("/admin/profiles", get(mock_profiles))
            .route("/admin/status", get(mock_profile_status))
            .with_state(Arc::clone(&state)),
    )
    .await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let (heartbeat, health, menu, _backoff) = heartbeat_on(&base_url, &status, &catalog);

    tokio::time::timeout(Duration::from_secs(5), async {
        while state.requests.load(Ordering::Relaxed) < 1 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first empty refresh completes");
    assert!(health.is_reachable(), "health stays continuously true");
    assert!(
        menu.latest()
            .is_some_and(|snapshot| snapshot.selected_model.is_none()),
        "an empty first catalog cannot restore a selection"
    );

    state.ready.store(true, Ordering::Relaxed);
    let ready = snapshot_where(&menu, |snapshot| snapshot.chat_ready).await;
    assert_eq!(ready.selected_model.as_deref(), Some("test-model"));
    assert!(
        state.requests.load(Ordering::Relaxed) >= 2,
        "readiness changed without a health transition, so refresh had to retry"
    );

    let requests_after_restore = state.requests.load(Ordering::Relaxed);
    tokio::time::sleep(TEST_INTERVAL * 4).await;
    assert_eq!(
        state.requests.load(Ordering::Relaxed),
        requests_after_restore,
        "selection restoration ends refresh retries"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_healthy_gateway_waits_for_profiles_after_its_catalog_is_ready() {
    let state = Arc::new(DelayedProfiles {
        ready: AtomicBool::new(false),
        list_requests: AtomicUsize::new(0),
        status_requests: AtomicUsize::new(0),
    });
    let base_url = serve(
        Router::new()
            .route("/health", get(|| async { StatusCode::OK }))
            .route("/v1/models", get(mock_models))
            .route("/admin/profiles", get(delayed_profiles))
            .route("/admin/status", get(delayed_profile_status))
            .with_state(Arc::clone(&state)),
    )
    .await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let (heartbeat, health, menu, _backoff) = heartbeat_on(&base_url, &status, &catalog);

    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = menu.latest();
            if state.list_requests.load(Ordering::Relaxed) >= 1
                && state.status_requests.load(Ordering::Relaxed) >= 1
                && catalog
                    .latest()
                    .is_some_and(|catalog| !catalog.models.is_empty())
                && snapshot.is_some_and(|snapshot| snapshot.profiles.is_empty())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the first empty profile refresh completes");
    assert!(health.is_reachable(), "health stays continuously true");
    assert_eq!(
        menu.latest().and_then(|snapshot| snapshot.selected_model),
        None,
        "catalog readiness alone cannot end startup convergence"
    );

    state.ready.store(true, Ordering::Relaxed);
    let ready = snapshot_where(&menu, |snapshot| {
        snapshot.profiles == ["coding", "main"]
            && snapshot.active.as_deref() == Some("main")
            && snapshot.chat_ready
    })
    .await;
    assert_eq!(ready.selected_model.as_deref(), Some("test-model"));
    assert!(
        state.list_requests.load(Ordering::Relaxed) >= 2
            && state.status_requests.load(Ordering::Relaxed) >= 2,
        "profile readiness changed without a health transition, so both endpoints had to retry"
    );
    heartbeat.shutdown().await;
}
