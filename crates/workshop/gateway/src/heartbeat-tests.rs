//! Heartbeat unit tests: the join-line recompute and the probe bound.
//! The bus-coupled loop behavior (transitions, refreshes, convergence)
//! is pinned by the workshop-server integration tests, which compose the
//! heartbeat with the real status, catalog, and menu buses.

use super::*;
use crate::gateway::GatewayClient;

fn retained(label: &str) -> StatusBarUpdate {
    StatusBarUpdate {
        label: label.to_owned(),
        description: String::new(),
        progress: None,
        severity: Severity::Info,
        activity: Activity::General,
    }
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
fn a_join_replays_a_retained_frame_carrying_real_work() {
    let health = GatewayHealth::new();
    let working = Some(StatusBarUpdate {
        label: "Downloading model".to_owned(),
        description: "ggml-large-v3.bin".to_owned(),
        progress: Some(workshop_protocol::Progress {
            current: 1,
            total: 2,
        }),
        severity: Severity::Info,
        activity: Activity::General,
    });
    let update = join_status(working, &health).expect("the work frame replays as-is");
    assert_eq!(update.label, "Downloading model");
    assert!(update.progress.is_some());
}

#[test]
fn a_join_with_no_retained_frame_sends_nothing() {
    let health = GatewayHealth::new();
    assert!(join_status(None, &health).is_none());
}

#[test]
fn the_probe_bound_is_shorter_than_the_heartbeat_interval() {
    assert!(
        crate::gateway::HEALTH_PROBE_TIMEOUT < HEARTBEAT_INTERVAL,
        "a probe outlasting the interval would back the heartbeat up \
         behind a stalled gateway"
    );
}

#[tokio::test]
async fn a_stalled_gateway_reads_unreachable_within_the_probe_bound() {
    // A stub that completes TCP handshakes and never answers: without
    // a bounded probe, the first probe would hang forever and the
    // heartbeat would never report at all.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stalled stub");
    let addr = listener.local_addr().expect("stalled stub address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    let client = GatewayClient::new(&format!("http://{addr}"), "")
        .expect("client builds in tests")
        .with_timeouts_for_test(Duration::from_millis(100), Duration::from_millis(100));
    // A recording status sink stands in for the status bus.
    let registry = workshop_registry::Registry::new();
    let (status_tx, mut rx) = tokio::sync::broadcast::channel(16);
    let _sink = registry.register_sink::<dyn workshop_registry::StatusSink>(std::sync::Arc::new(
        workshop_registry::StatusSinkAdapter::new(move |update| {
            let _ = status_tx.send(update);
        }),
    ));
    let heartbeat = spawn(
        GatewayBinding::from_client(client),
        registry.push(),
        GatewayHealth::new(),
        Duration::from_millis(25),
        ReconnectBackoff::with_schedule(
            Duration::from_millis(10),
            Duration::from_millis(40),
            Duration::from_secs(60),
        ),
    );
    let update = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("a status update arrives within the deadline")
        .expect("the recording sink is open");
    assert_eq!(update.label, "Gateway unreachable");
    heartbeat.shutdown().await;
}
