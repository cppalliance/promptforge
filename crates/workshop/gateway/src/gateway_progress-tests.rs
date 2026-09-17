// Fractions are fixed-point millionths, so equality comparisons are exact
// (the shared-progress remote.rs test precedent).
#![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;

use shared_progress::OperationSnapshot;

/// Binds `app` as a mock gateway on a free loopback port and returns its
/// base URL.
async fn spawn_gateway(app: axum::Router) -> String {
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

/// A replaceable binding for one mock Gateway.
fn binding(base_url: &str) -> GatewayBinding {
    GatewayBinding::new(base_url, "").expect("the test binding builds")
}

/// A mock `GET /admin/progress`: every payload published to the feed
/// streams to every connected subscriber as an SSE `data:` frame, and
/// `connections` counts how often the endpoint was hit. The receiver
/// is created before the count increments, so a test that observes a
/// connection can publish without losing the frame. [`close`](Self::close)
/// ends every live stream, so a test can drive the resubscribe path.
struct MockProgress {
    connections: AtomicUsize,
    feeds: std::sync::Mutex<broadcast::Sender<String>>,
}

impl MockProgress {
    fn new() -> Self {
        Self {
            connections: AtomicUsize::new(0),
            feeds: std::sync::Mutex::new(broadcast::channel(16).0),
        }
    }

    fn router(self: Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/admin/progress", axum::routing::get(serve_feed))
            .with_state(self)
    }

    /// Publishes one payload to every connected subscriber.
    fn send(&self, payload: String) {
        self.feeds
            .lock()
            .expect("the feed lock is not poisoned")
            .send(payload)
            .expect("the mock has a subscriber");
    }

    /// Ends every live stream; later connections subscribe to the
    /// fresh feed.
    fn close(&self) {
        *self.feeds.lock().expect("the feed lock is not poisoned") = broadcast::channel(16).0;
    }
}

async fn serve_feed(State(mock): State<Arc<MockProgress>>) -> Response {
    let rx = mock
        .feeds
        .lock()
        .expect("the feed lock is not poisoned")
        .subscribe();
    mock.connections.fetch_add(1, Ordering::Relaxed);
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(payload) => {
                    return Some((
                        Ok::<_, std::convert::Infallible>(format!("data: {payload}\n\n")),
                        rx,
                    ));
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {}
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    (
        [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
        axum::body::Body::from_stream(stream),
    )
        .into_response()
}

/// Serializes a wire-format progress event by hand, so the tests pin
/// the JSON shape the gateway emits rather than the progress crate's
/// constructors (the gateway-client test pattern).
fn event_json(path: &str, state: &serde_json::Value) -> String {
    serde_json::json!({
        "operation": 7,
        "path": path,
        "label": path,
        "state": state,
    })
    .to_string()
}

/// Polls the hub's snapshot until `accept` holds, within a generous
/// deadline (the heartbeat tests' snapshot_where pattern).
async fn snapshot_where(
    hub: &ProgressHub,
    accept: impl Fn(&[OperationSnapshot]) -> bool,
) -> Vec<OperationSnapshot> {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let snapshot = hub.snapshot();
            if accept(&snapshot) {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("a matching snapshot arrives within the deadline")
}

/// Polls the mock's connection count until it reaches `n`.
async fn wait_for_connections(mock: &MockProgress, n: usize) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while mock.connections.load(Ordering::Relaxed) < n {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the subscriber connects within the deadline");
}

#[tokio::test]
async fn events_from_the_gateway_feed_a_remote_operation_on_the_hub() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    // The flag starts optimistic, so the subscriber connects at once.
    let subscriber = spawn(binding(&base_url), Arc::clone(&hub), GatewayHealth::new());

    wait_for_connections(&mock, 1).await;
    mock.send(event_json(
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    mock.send(event_json(
        "download",
        &serde_json::json!({"Updated": {"fraction": 0.5}}),
    ));

    let snapshot = snapshot_where(&hub, |s| {
        s.len() == 1 && s[0].nodes.iter().any(|n| n.fraction == 0.5)
    })
    .await;
    assert_eq!(snapshot[0].nodes[0].path, "download");
    assert_eq!(snapshot[0].nodes[0].label, "download");
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_malformed_event_is_skipped_and_the_stream_continues() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    let subscriber = spawn(binding(&base_url), Arc::clone(&hub), GatewayHealth::new());

    wait_for_connections(&mock, 1).await;
    mock.send(event_json(
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    // One undecodable `data:` block between two valid events: the
    // subscriber warns and continues rather than dropping the stream.
    mock.send("{not valid json".to_owned());
    mock.send(event_json(
        "download",
        &serde_json::json!({"Updated": {"fraction": 0.5}}),
    ));

    let snapshot = snapshot_where(&hub, |s| {
        s.len() == 1 && s[0].nodes.iter().any(|n| n.fraction == 0.5)
    })
    .await;
    assert_eq!(
        snapshot[0].nodes[0].path, "download",
        "the event after the malformed one still lands on the hub"
    );
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_stream_that_ends_while_reachable_resubscribes_after_the_delay() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    let delay = Duration::from_millis(50);
    let subscriber = spawn_with_delay(
        binding(&base_url),
        Arc::clone(&hub),
        GatewayHealth::new(),
        delay,
    );

    wait_for_connections(&mock, 1).await;
    mock.send(event_json(
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    snapshot_where(&hub, |s| s.len() == 1).await;

    // The stream ends while the gateway still reads reachable: the
    // import detaches, and a fresh subscription follows the delay.
    let closed = std::time::Instant::now();
    mock.close();
    snapshot_where(&hub, <[OperationSnapshot]>::is_empty).await;
    wait_for_connections(&mock, 2).await;
    assert!(
        closed.elapsed() >= delay,
        "the resubscribe waits out the delay rather than spinning"
    );
    subscriber.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_gateway_holds_no_subscription_and_no_remote_state() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    let health = GatewayHealth::new();
    health.publish(false);
    let subscriber = spawn(binding(&base_url), Arc::clone(&hub), health.clone());

    let quiet = tokio::time::timeout(Duration::from_millis(200), async {
        wait_for_connections(&mock, 1).await;
    })
    .await;
    assert!(
        quiet.is_err(),
        "an unreachable gateway must not be subscribed"
    );
    assert!(hub.snapshot().is_empty());

    health.publish(true);
    wait_for_connections(&mock, 1).await;
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_reconnect_resubscribes_without_duplicating_state() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let hub = Arc::new(ProgressHub::new());
    let health = GatewayHealth::new();
    let subscriber = spawn(binding(&base_url), Arc::clone(&hub), health.clone());

    wait_for_connections(&mock, 1).await;
    mock.send(event_json(
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    let first = snapshot_where(&hub, |s| s.len() == 1).await;

    health.publish(false);
    snapshot_where(&hub, <[OperationSnapshot]>::is_empty).await;

    health.publish(true);
    wait_for_connections(&mock, 2).await;
    mock.send(event_json(
        "download",
        &serde_json::json!({"Begun": {"weight": 1.0}}),
    ));
    mock.send(event_json(
        "download",
        &serde_json::json!({"Updated": {"fraction": 0.5}}),
    ));
    let reconnected = snapshot_where(&hub, |s| {
        s.len() == 1 && s[0].nodes.iter().any(|n| n.fraction == 0.5)
    })
    .await;
    assert_eq!(
        reconnected.len(),
        1,
        "the reconnect replaces the import, never stacks a second one"
    );
    assert_ne!(
        first[0].operation, reconnected[0].operation,
        "the resubscription attaches a fresh import under a new local id"
    );
    subscriber.shutdown().await;
}

#[path = "gateway_progress-tests-lifecycle.rs"]
mod lifecycle;
#[path = "gateway_progress-tests-recovery.rs"]
mod recovery;
