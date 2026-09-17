//! The heartbeat loop's behavior against the real status, catalog, and
//! menu buses: transition announcements, refresh-on-reconnect, startup
//! convergence, and the backoff's anti-flap rule. These tests compose
//! `workshop-gateway`'s heartbeat with `workshop-status` and
//! `workshop-menu`'s buses through the registry's push facade - the
//! composition only the shell can make, so they live in its integration
//! binary rather than in any one subsystem crate.

// clippy.toml's allow-expect-in-tests covers #[test] functions and
// #[cfg(test)] modules only, not integration-test helpers; failing a test
// by panicking with the invariant named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use tokio::sync::broadcast;

use workshop_gateway::{GatewayBinding, GatewayHealth, Heartbeat};
use workshop_menu::{CatalogBus, MenuBus};
use workshop_protocol::{CatalogPush, Severity, StatusBarUpdate, WorkbenchSnapshot};
use workshop_registry::{Push, Registration, Registry};
use workshop_status::StatusBus;
use workshop_support::ReconnectBackoff;

/// The registration guards keeping the test's contributions alive.
type Guards = (
    Registration,
    Registration,
    Registration,
    Registration,
    Registration,
    Registration,
);

/// Wires the buses into a fresh registry and returns the push facade
/// plus the guards keeping the registrations alive.
fn wired_push(status: &StatusBus, catalog: &CatalogBus, menu: &MenuBus) -> (Push, Guards) {
    let registry = Registry::new();
    let (status_channel, status_sink, status_state) = workshop_status::register(&registry, status);
    let (catalog_sink, menu_sink, menu_state) = workshop_menu::register(&registry, catalog, menu);
    (
        registry.push(),
        (
            status_channel,
            status_sink,
            status_state,
            catalog_sink,
            menu_sink,
            menu_state,
        ),
    )
}

/// Fast enough to observe transitions without real waiting, slow
/// enough that a 200 ms quiet window spans several ticks and so proves
/// the loop does not re-emit a steady state.
const TEST_INTERVAL: Duration = Duration::from_millis(25);

const CATALOG: &str =
    r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

/// A mock `/health` whose answer flips under test control.
async fn flippable_health(State(healthy): State<Arc<AtomicBool>>) -> Response {
    if healthy.load(Ordering::Relaxed) {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

/// A static mock catalog for the refresh-on-reconnect tests.
async fn mock_models() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        CATALOG,
    )
        .into_response()
}

/// A static mock profile list for the profile-populate tests.
async fn mock_profiles() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"profiles":["coding","main"]}"#,
    )
        .into_response()
}

/// A static mock gateway status naming the active profile.
async fn mock_profile_status() -> Response {
    (
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        r#"{"profile":"main","models":["test-model"]}"#,
    )
        .into_response()
}

/// Binds a mock gateway whose `/health` flips with `healthy`, with a
/// static `/v1/models` and the profile endpoints beside it.
async fn spawn_gateway(healthy: Arc<AtomicBool>) -> String {
    let app = Router::new()
        .route("/health", get(flippable_health))
        .route("/v1/models", get(mock_models))
        .route("/admin/profiles", get(mock_profiles))
        .route("/admin/status", get(mock_profile_status))
        .with_state(healthy);
    serve(app).await
}

/// Binds `app` on a free loopback port and returns its base URL.
async fn serve(app: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind mock gateway");
    let addr = listener.local_addr().expect("mock gateway address");
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("mock gateway serves");
    });
    format!("http://{addr}")
}

/// A backoff fast enough that a down-phase probe retries within a few
/// ticks, with a budget no test exhausts by accident.
fn test_backoff() -> ReconnectBackoff {
    ReconnectBackoff::with_schedule(
        Duration::from_millis(10),
        Duration::from_millis(40),
        Duration::from_secs(60),
    )
}

/// Starts a heartbeat against `base_url` on the fast interval, wired to
/// `status` and `catalog`; returns the handle, the shared health flag,
/// the menu bus the heartbeat feeds, its reconnect backoff, and the
/// guards keeping the sink registrations alive.
fn heartbeat_on(
    base_url: &str,
    status: &StatusBus,
    catalog: &CatalogBus,
) -> (Heartbeat, GatewayHealth, MenuBus, ReconnectBackoff, Guards) {
    let gateway =
        GatewayBinding::new_with_identity(base_url, "", None).expect("binding builds in tests");
    let health = GatewayHealth::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let backoff = test_backoff();
    let (push, guards) = wired_push(status, catalog, &menu);
    let heartbeat = workshop_gateway::heartbeat::spawn(
        gateway,
        push,
        health.clone(),
        TEST_INTERVAL,
        backoff.clone(),
    );
    (heartbeat, health, menu, backoff, guards)
}

/// Receives the next status update within a generous deadline.
async fn next_update(rx: &mut broadcast::Receiver<StatusBarUpdate>) -> StatusBarUpdate {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a status update arrives within the deadline")
        .expect("the status bus is open")
}

/// Asserts no update arrives within a window spanning several ticks.
async fn assert_quiet(rx: &mut broadcast::Receiver<StatusBarUpdate>) {
    let quiet = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await;
    assert!(
        quiet.is_err(),
        "a steady state must not re-emit, got {quiet:?}"
    );
}

/// Polls the retained menu snapshot until `accept` holds, within a
/// generous deadline. Polling the retained copy rather than
/// subscribing sidesteps the race between the heartbeat's publishes
/// and the test's subscription.
async fn snapshot_where(
    menu: &MenuBus,
    accept: impl Fn(&WorkbenchSnapshot) -> bool,
) -> WorkbenchSnapshot {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(snapshot) = menu.latest()
                && accept(&snapshot)
            {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a matching snapshot is retained within the deadline")
}

#[tokio::test]
async fn a_healthy_gateway_fires_connected_once_and_stays_quiet() {
    let healthy = Arc::new(AtomicBool::new(true));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    let update = next_update(&mut rx).await;
    assert_eq!(update.label, "Connected to gateway");
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, workshop_protocol::Activity::General);
    assert!(health.is_reachable(), "the probe published reachable");
    assert_quiet(&mut rx).await;
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_gateway_fires_unreachable_once_and_stays_quiet() {
    // Nothing listens on port 1, so the connect fails deterministically.
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, _backoff, _guards) =
        heartbeat_on("http://127.0.0.1:1", &status, &catalog);

    let update = next_update(&mut rx).await;
    assert_eq!(update.label, "Gateway unreachable");
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, workshop_protocol::Activity::General);
    assert!(!health.is_reachable(), "the probe published unreachable");
    assert_quiet(&mut rx).await;
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn each_transition_fires_exactly_one_update() {
    let healthy = Arc::new(AtomicBool::new(true));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(next_update(&mut rx).await.label, "Connected to gateway");
    healthy.store(false, Ordering::Relaxed);
    assert_eq!(next_update(&mut rx).await.label, "Gateway unreachable");
    assert!(!health.is_reachable());
    healthy.store(true, Ordering::Relaxed);
    assert_eq!(next_update(&mut rx).await.label, "Connected to gateway");
    assert!(health.is_reachable());
    assert_quiet(&mut rx).await;
    heartbeat.shutdown().await;
}

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
        "the push carries every chat-capable gateway model"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_reconnect_whose_refresh_is_declined_pushes_no_catalog() {
    // No /v1/models route: the refresh is declined with a 404, and a
    // declined refresh is skipped rather than pushed - pushing it
    // would empty pickers that still hold a usable list.
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = serve(
        Router::new()
            .route("/health", get(flippable_health))
            .with_state(Arc::clone(&healthy)),
    )
    .await;
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
    let quiet = tokio::time::timeout(Duration::from_millis(200), catalog_rx.recv()).await;
    assert!(quiet.is_err(), "a declined refresh is skipped, not pushed");
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
    let base_url = serve(
        Router::new()
            .route("/health", get(flippable_health))
            .with_state(Arc::clone(&healthy)),
    )
    .await;
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

#[tokio::test]
async fn reachability_transitions_flip_chat_ready() {
    let healthy = Arc::new(AtomicBool::new(true));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    // Readiness needs a non-empty catalog and a selection; the mock
    // catalog holds test-model, so a reconnect's refresh keeps it.
    catalog.publish(vec![serde_json::json!({"id": "test-model"})]);
    let mut status_rx = status.subscribe();
    let (heartbeat, _health, menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(
        next_update(&mut status_rx).await.label,
        "Connected to gateway"
    );
    menu.set_selected("test-model")
        .expect("the id is in the catalog");
    snapshot_where(&menu, |snapshot| snapshot.chat_ready).await;

    healthy.store(false, Ordering::Relaxed);
    let down = snapshot_where(&menu, |snapshot| !snapshot.chat_ready).await;
    assert_eq!(
        down.selected_model.as_deref(),
        Some("test-model"),
        "only reachability flipped; the selection survives the outage"
    );

    healthy.store(true, Ordering::Relaxed);
    snapshot_where(&menu, |snapshot| snapshot.chat_ready).await;
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_mere_connect_keeps_the_backoff_escalated() {
    // The anti-flap rule this step exists for: an outage escalates the
    // backoff, and a gateway that answers its probe again (connects
    // without delivering any useful work) must leave the escalation
    // standing, so the next outage keeps the slow schedule.
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    assert_eq!(next_update(&mut rx).await.label, "Gateway unreachable");
    healthy.store(true, Ordering::Relaxed);
    assert_eq!(next_update(&mut rx).await.label, "Connected to gateway");
    assert!(health.is_reachable());
    assert!(
        backoff.is_escalated_for_test(),
        "reconnecting without useful work must not reset the backoff"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn an_exhausted_budget_stops_reconnect_probes_with_a_give_up_report() {
    let healthy = Arc::new(AtomicBool::new(false));
    let base_url = spawn_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let gateway =
        GatewayBinding::new_with_identity(&base_url, "", None).expect("binding builds in tests");
    let health = GatewayHealth::new();
    let menu = MenuBus::new(catalog.clone(), None);
    // A budget of a few schedule steps: exhausted within a handful of
    // failed probes, well inside the test deadline.
    let backoff = ReconnectBackoff::with_schedule(
        Duration::from_millis(10),
        Duration::from_millis(20),
        Duration::from_millis(50),
    );
    let (push, _guards) = wired_push(&status, &catalog, &menu);
    let heartbeat =
        workshop_gateway::heartbeat::spawn(gateway, push, health.clone(), TEST_INTERVAL, backoff);

    assert_eq!(next_update(&mut rx).await.label, "Gateway unreachable");
    let report = next_update(&mut rx).await;
    assert_eq!(report.label, "Gateway reconnect stopped");
    assert_eq!(report.severity, Severity::Error);
    // The gateway coming back after the give-up changes nothing: the
    // loop has ended, so no probe ever notices.
    healthy.store(true, Ordering::Relaxed);
    assert_quiet(&mut rx).await;
    assert!(
        !health.is_reachable(),
        "an ended loop leaves the last verdict standing"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn shutdown_stops_the_task_without_waiting_out_the_interval() {
    // A long interval: if the stop signal did not win the select, the
    // shutdown would block for the whole minute.
    let status = StatusBus::new();
    let gateway = GatewayBinding::new_with_identity("http://127.0.0.1:1", "", None)
        .expect("binding builds in tests");
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let (push, _guards) = wired_push(&status, &catalog, &menu);
    let heartbeat = workshop_gateway::heartbeat::spawn(
        gateway,
        push,
        GatewayHealth::new(),
        Duration::from_secs(60),
        ReconnectBackoff::new(),
    );
    tokio::time::timeout(Duration::from_secs(5), heartbeat.shutdown())
        .await
        .expect("shutdown does not wait out the interval");
}

mod recovery;
mod startup_convergence;
