//! The heartbeat loop's behavior against the real status, catalog, and
//! menu buses: transition announcements, refresh-on-reconnect, startup
//! convergence, and the backoff's anti-flap rule. These tests compose
//! `workshop-gateway`'s heartbeat with `workshop-status` and
//! `workshop-menu`'s buses through the registry's push facade - the
//! composition only the server can make, so they sit in its integration
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
use tokio::sync::{broadcast, watch};

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
    let status_regs = workshop_status::register(&registry, status);
    let menu_regs = workshop_menu::register(&registry, catalog, menu);
    (
        registry.push(),
        (
            status_regs.channel,
            status_regs.sink,
            status_regs.state,
            menu_regs.catalog_sink,
            menu_regs.menu_sink,
            menu_regs.state,
        ),
    )
}

/// Fast enough that transitions, and the probes a quiet check counts,
/// arrive without real waiting.
const TEST_INTERVAL: Duration = Duration::from_millis(25);

const CATALOG: &str =
    r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

/// A mock `/health` answer under test control, and the count of probes
/// served. The loop is sequential - wait, probe, announce, refresh - so
/// a served probe proves every earlier probe's announcement and refresh
/// are done.
#[derive(Clone)]
struct MockHealth {
    healthy: Arc<AtomicBool>,
    probes: watch::Sender<usize>,
}

/// A mock `/health` whose answer flips under test control.
async fn flippable_health(State(mock): State<MockHealth>) -> Response {
    mock.probes.send_modify(|served| *served += 1);
    if mock.healthy.load(Ordering::Relaxed) {
        StatusCode::OK.into_response()
    } else {
        StatusCode::SERVICE_UNAVAILABLE.into_response()
    }
}

/// A router serving only the flippable `/health`, and the count of the
/// probes it serves.
fn health_only(healthy: Arc<AtomicBool>) -> (Router, watch::Receiver<usize>) {
    let (probes, served) = watch::channel(0);
    let router = Router::new()
        .route("/health", get(flippable_health))
        .with_state(MockHealth { healthy, probes });
    (router, served)
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
    spawn_probed_gateway(healthy).await.0
}

/// [`spawn_gateway`], also returning the count of health probes served.
async fn spawn_probed_gateway(healthy: Arc<AtomicBool>) -> (String, watch::Receiver<usize>) {
    let (health, probes) = health_only(healthy);
    let app = health
        .route("/v1/models", get(mock_models))
        .route("/admin/profiles", get(mock_profiles))
        .route("/admin/status", get(mock_profile_status));
    (serve(app).await, probes)
}

/// Binds a loopback listener that accepts each connection and drops it
/// unanswered: an unreachable gateway whose failed probes are counted
/// the way [`flippable_health`] counts answered ones.
async fn spawn_silent_gateway() -> (String, watch::Receiver<usize>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("a loopback port binds");
    let addr = listener.local_addr().expect("the listener has an address");
    let (probes, served) = watch::channel(0);
    drop(tokio::spawn(async move {
        while let Ok((connection, _)) = listener.accept().await {
            probes.send_modify(|count| *count += 1);
            drop(connection);
        }
    }));
    (format!("http://{addr}"), served)
}

/// Binds `app` on a free loopback port and returns its base URL.
async fn serve(app: Router) -> String {
    let (addr, _handle) = workshop_support::fixtures::serve(app).await;
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
    let heartbeat = workshop_gateway::spawn_heartbeat(
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

/// Waits until the mock serves `count` more probes, so every probe
/// before the last of them has finished its announcement and refresh.
async fn await_probes(probes: &mut watch::Receiver<usize>, count: usize) {
    let target = *probes.borrow_and_update() + count;
    tokio::time::timeout(
        Duration::from_secs(5),
        probes.wait_for(|served| *served >= target),
    )
    .await
    .expect("the probes are served within the deadline")
    .expect("the mock gateway keeps counting");
}

/// Asserts that a steady state re-emits nothing: once three more probes
/// are served, at least two probes after the last transition have
/// finished without an update.
async fn assert_quiet(
    rx: &mut broadcast::Receiver<StatusBarUpdate>,
    probes: &mut watch::Receiver<usize>,
) {
    await_probes(probes, 3).await;
    let quiet = rx.try_recv();
    assert!(
        matches!(quiet, Err(broadcast::error::TryRecvError::Empty)),
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
    let (base_url, mut probes) = spawn_probed_gateway(Arc::clone(&healthy)).await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    let update = next_update(&mut rx).await;
    assert_eq!(update.label, "Connected to gateway");
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, workshop_protocol::Activity::General);
    assert!(health.is_reachable(), "the probe published reachable");
    assert_quiet(&mut rx, &mut probes).await;
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_gateway_fires_unreachable_once_and_stays_quiet() {
    let (base_url, mut probes) = spawn_silent_gateway().await;
    let status = StatusBus::new();
    let catalog = CatalogBus::new();
    let mut rx = status.subscribe();
    let (heartbeat, health, _menu, _backoff, _guards) = heartbeat_on(&base_url, &status, &catalog);

    let update = next_update(&mut rx).await;
    assert_eq!(update.label, "Gateway unreachable");
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, workshop_protocol::Activity::General);
    assert!(!health.is_reachable(), "the probe published unreachable");
    assert_quiet(&mut rx, &mut probes).await;
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn each_transition_fires_exactly_one_update() {
    let healthy = Arc::new(AtomicBool::new(true));
    let (base_url, mut probes) = spawn_probed_gateway(Arc::clone(&healthy)).await;
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
    assert_quiet(&mut rx, &mut probes).await;
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
    // The loop is handed the only reachability sender, so this watch
    // closes exactly when the loop ends.
    let mut verdict = health.subscribe();
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
        workshop_gateway::spawn_heartbeat(gateway, push, health, TEST_INTERVAL, backoff);

    assert_eq!(next_update(&mut rx).await.label, "Gateway unreachable");
    let report = next_update(&mut rx).await;
    assert_eq!(report.label, "Gateway reconnect stopped");
    assert_eq!(report.severity, Severity::Error);
    // The gateway coming back after the give-up changes nothing: the
    // loop has ended, so no probe ever notices.
    healthy.store(true, Ordering::Relaxed);
    tokio::time::timeout(Duration::from_secs(5), async {
        while verdict.changed().await.is_ok() {}
    })
    .await
    .expect("the loop ends with its give-up report");
    let after = rx.try_recv();
    assert!(
        matches!(after, Err(broadcast::error::TryRecvError::Empty)),
        "an ended loop reports nothing more, got {after:?}"
    );
    assert!(
        !*verdict.borrow(),
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
    let heartbeat = workshop_gateway::spawn_heartbeat(
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
mod refresh_on_reconnect;
mod startup_convergence;
