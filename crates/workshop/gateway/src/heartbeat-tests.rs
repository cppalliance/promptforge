//! Heartbeat unit tests: the join-line recompute, the probe bound, and
//! the stop signal and binding replacement ending an in-flight probe or
//! refresh. The bus-coupled loop behavior (transitions, refreshes,
//! convergence) is pinned by the workshop-server integration tests, which
//! compose the heartbeat with the real status, catalog, and menu buses.

use super::*;
use crate::client::GatewayClient;

use std::sync::Arc;

use axum::http::StatusCode;
use axum::routing::get;
use tokio::sync::{Notify, broadcast};

fn retained(label: &str) -> StatusBarUpdate {
    StatusBarUpdate {
        label: label.to_owned(),
        description: String::new(),
        busy: false,
        severity: Severity::Info,
        activity: Activity::General,
    }
}

/// Binds `app` on a free loopback port and returns its base URL.
async fn serve(app: axum::Router) -> String {
    let (addr, _handle) = workshop_support::fixtures::serve(app).await;
    format!("http://{addr}")
}

/// Binds a stub that completes TCP handshakes and never answers, and
/// notifies `accepted` as it takes each connection.
async fn stalled_stub() -> (String, Arc<Notify>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalled stub");
    let addr = listener.local_addr().expect("stalled stub address");
    let accepted = Arc::new(Notify::new());
    let notify = Arc::clone(&accepted);
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
            notify.notify_one();
        }
    });
    (format!("http://{addr}"), accepted)
}

/// A recording status sink standing in for the status bus: the push the
/// heartbeat reports through, the registration keeping the sink alive,
/// and the receiver the test reads.
fn status_recorder() -> (
    Push,
    workshop_registry::Registration,
    broadcast::Receiver<StatusBarUpdate>,
) {
    let registry = workshop_registry::Registry::new();
    let (status_tx, rx) = broadcast::channel(16);
    let sink = registry.register_sink::<dyn workshop_registry::StatusSink>(Arc::new(
        workshop_registry::StatusSinkAdapter::new(move |update| {
            let _ = status_tx.send(update);
        }),
    ));
    (registry.push(), sink, rx)
}

/// The next status line within a generous deadline.
async fn next_label(rx: &mut broadcast::Receiver<StatusBarUpdate>) -> String {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a status update arrives within the deadline")
        .expect("the recording sink is open")
        .label
}

/// Waits for `notify` within a generous deadline.
async fn notified(notify: &Notify, what: &str) {
    tokio::time::timeout(Duration::from_secs(5), notify.notified())
        .await
        .unwrap_or_else(|_| panic!("{what} within the deadline"));
}

#[test]
fn a_join_recomputes_a_stale_connect_announcement_to_the_resting_line() {
    let health = GatewayHealth::new();
    let update = join_status(Some(retained(CONNECTED_LABEL)), &health)
        .expect("a retained transition still yields a join line");
    assert_eq!(update.label, "Ready");
    assert_eq!(update.severity, Severity::Info);
}

#[test]
fn a_join_recomputes_a_stale_connect_announcement_during_an_outage() {
    let health = GatewayHealth::new();
    health.publish(false);
    let update = join_status(Some(retained(CONNECTED_LABEL)), &health)
        .expect("a retained transition still yields a join line");
    assert_eq!(update.label, UNREACHABLE_LABEL);
    assert_eq!(update.description, UNREACHABLE_DESCRIPTION);
}

#[test]
fn a_join_keeps_a_retained_outage_while_the_gateway_is_down() {
    let health = GatewayHealth::new();
    health.publish(false);
    let update = join_status(Some(retained(UNREACHABLE_LABEL)), &health)
        .expect("the outage line survives the recompute");
    assert_eq!(update.label, UNREACHABLE_LABEL);
}

#[test]
fn a_join_replays_a_retained_frame_describing_real_work() {
    let health = GatewayHealth::new();
    let working = Some(StatusBarUpdate {
        label: "Downloading model".to_owned(),
        description: "ggml-large-v3.bin".to_owned(),
        busy: true,
        severity: Severity::Info,
        activity: Activity::General,
    });
    let update = join_status(working, &health).expect("the work frame replays as-is");
    assert_eq!(update.label, "Downloading model");
    assert!(update.busy, "the busy flag survives the join recompute");
}

#[test]
fn a_join_with_no_retained_frame_sends_nothing() {
    let health = GatewayHealth::new();
    assert!(join_status(None, &health).is_none());
}

#[test]
fn the_probe_bound_is_shorter_than_the_heartbeat_interval() {
    assert!(
        crate::client::HEALTH_PROBE_TIMEOUT < HEARTBEAT_INTERVAL,
        "a probe outlasting the interval would back the heartbeat up \
         behind a stalled gateway"
    );
}

#[tokio::test]
async fn a_wait_with_no_reachability_watch_runs_its_future_to_completion() {
    let (_stop_tx, mut stop) = oneshot::channel();
    let (_gateway_tx, mut gateway_changed) = watch::channel(0_u64);
    let mut signals = Signals {
        stop: &mut stop,
        reachable: None,
        gateway_changed: &mut gateway_changed,
    };
    let output = until(
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            7
        },
        &mut signals,
    )
    .await;
    assert!(
        matches!(output, Ok(7)),
        "an absent reachability watch never ends the wait"
    );
}

#[tokio::test]
async fn a_stalled_gateway_reads_unreachable_within_the_probe_bound() {
    // Without a bounded probe, the first probe would hang forever and the
    // heartbeat would never report at all.
    let (stalled, _accepted) = stalled_stub().await;
    let client = GatewayClient::new(&stalled, "")
        .expect("client builds in tests")
        .with_timeouts_for_test(Duration::from_millis(100), Duration::from_millis(100));
    let (push, _sink, mut rx) = status_recorder();
    let heartbeat = spawn(
        GatewayBinding::from_client(client),
        push,
        GatewayHealth::new(),
        Duration::from_millis(25),
        ReconnectBackoff::with_schedule(
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_secs(60),
        ),
    );
    assert_eq!(next_label(&mut rx).await, "Gateway unreachable");
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn a_binding_replacement_interrupts_a_stalled_probe() {
    let (stalled, accepted) = stalled_stub().await;
    let healthy =
        serve(axum::Router::new().route("/health", get(|| async { StatusCode::OK }))).await;
    // A probe bound far past the test deadline: only the rebind can end
    // the stalled probe in time.
    let client = GatewayClient::new(&stalled, "")
        .expect("client builds in tests")
        .with_timeouts_for_test(Duration::from_secs(60), Duration::from_secs(60));
    let gateway = GatewayBinding::from_client(client);
    let (push, _sink, mut rx) = status_recorder();
    let heartbeat = spawn(
        gateway.clone(),
        push,
        GatewayHealth::new(),
        Duration::from_secs(60),
        ReconnectBackoff::new(),
    );

    notified(&accepted, "the first probe reaches the stalled stub").await;
    gateway
        .replace(&healthy, "")
        .expect("the replacement publishes");
    assert_eq!(
        next_label(&mut rx).await,
        CONNECTED_LABEL,
        "the replacement's probe reports without waiting out the stalled one"
    );
    heartbeat.shutdown().await;
}

#[tokio::test]
async fn shutdown_interrupts_a_stalled_refresh() {
    let refreshing = Arc::new(Notify::new());
    let app = axum::Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route(
            "/v1/models",
            get({
                let refreshing = Arc::clone(&refreshing);
                move || {
                    refreshing.notify_one();
                    std::future::pending::<StatusCode>()
                }
            }),
        )
        .route("/admin/profiles", get(std::future::pending::<StatusCode>))
        .route("/admin/status", get(std::future::pending::<StatusCode>));
    let base_url = serve(app).await;
    let (push, _sink, mut rx) = status_recorder();
    let heartbeat = spawn(
        GatewayBinding::new(&base_url, "").expect("the test binding builds"),
        push,
        GatewayHealth::new(),
        Duration::from_secs(60),
        ReconnectBackoff::new(),
    );

    assert_eq!(next_label(&mut rx).await, CONNECTED_LABEL);
    notified(&refreshing, "the catalog refresh reaches the gateway").await;
    // The stalled refresh holds for the client's request timeout, far
    // past this deadline, unless the stop signal wins the wait.
    tokio::time::timeout(Duration::from_secs(5), heartbeat.shutdown())
        .await
        .expect("shutdown does not wait out the stalled refresh");
}
