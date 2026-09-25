//! Subscriber tests against a mock `GET /admin/progress`: snapshots
//! reach the status bar as busy frames under the anti-flicker policy,
//! a malformed snapshot is skipped, a closed stream resubscribes after
//! the delay, an unreachable gateway holds no subscription, and a
//! reconnect rests the bar and resubscribes once. A lost subscription
//! rests the bar under the presenter's minimum-visible hold, which the
//! loop keeps ticking between subscriptions. The presenter's timing
//! rules are pinned separately with explicit instants; here the policy
//! runs at millisecond scale so the mock round trips stay fast.

use super::*;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use tokio::sync::broadcast;

use workshop_protocol::StatusBarUpdate;
use workshop_registry::{Registration, Registry, StatusSink, StatusSinkAdapter};

/// The anti-flicker policy the mock-gateway tests run under: short
/// enough that a test waits milliseconds, long enough that a snapshot
/// still has to outlive the show delay to reach the bar.
const FAST_POLICY: Policy = Policy {
    show_delay: Duration::from_millis(40),
    min_visible: Duration::from_millis(20),
};

/// The production resubscribe delay is seconds; the tests inject this.
const FAST_TIMING: Timing = Timing {
    resubscribe_delay: Duration::from_millis(50),
    policy: FAST_POLICY,
};

/// Binds `app` as a mock gateway on a free loopback port and returns its
/// base URL.
async fn spawn_gateway(app: axum::Router) -> String {
    let (addr, _handle) = workshop_support::fixtures::serve(app).await;
    format!("http://{addr}")
}

/// A replaceable binding for one mock gateway.
fn binding(base_url: &str) -> GatewayBinding {
    GatewayBinding::new(base_url, "").expect("the test binding builds")
}

/// A recording status sink behind a [`Push`]: every frame the subscriber
/// pushes lands in `frames`, in order.
struct Recorder {
    push: Push,
    frames: Arc<Mutex<Vec<StatusBarUpdate>>>,
    _guard: Registration,
}

fn recorder() -> Recorder {
    let registry = Registry::new();
    let frames: Arc<Mutex<Vec<StatusBarUpdate>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&frames);
    let guard =
        registry.register_sink::<dyn StatusSink>(Arc::new(StatusSinkAdapter::new(move |update| {
            sink.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(update);
        })));
    Recorder {
        push: registry.push(),
        frames,
        _guard: guard,
    }
}

impl Recorder {
    /// The `(label, busy)` of every frame pushed so far.
    fn pushed(&self) -> Vec<(String, bool)> {
        self.frames
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|update| (update.label.clone(), update.busy))
            .collect()
    }

    /// Polls until `accept` holds over the pushed frames, within a
    /// generous deadline (the heartbeat tests' snapshot_where pattern).
    async fn frames_where(
        &self,
        accept: impl Fn(&[(String, bool)]) -> bool,
    ) -> Vec<(String, bool)> {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let pushed = self.pushed();
                if accept(&pushed) {
                    return pushed;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("matching frames arrive within the deadline")
    }
}

/// A mock `GET /admin/progress`: every payload published to the feed
/// streams to every connected subscriber as an SSE `data:` frame, and
/// `connections` counts how often the endpoint was hit. The receiver
/// is created before the count increments, so a test that observes a
/// connection can publish without losing the frame. [`close`](Self::close)
/// ends every live stream, so a test can drive the resubscribe path.
struct MockProgress {
    connections: AtomicUsize,
    feeds: Mutex<broadcast::Sender<String>>,
}

impl MockProgress {
    fn new() -> Self {
        Self {
            connections: AtomicUsize::new(0),
            feeds: Mutex::new(broadcast::channel(16).0),
        }
    }

    fn router(self: Arc<Self>) -> axum::Router {
        axum::Router::new()
            .route("/admin/progress", axum::routing::get(serve_feed))
            .with_state(self)
    }

    /// Publishes one snapshot to every connected subscriber.
    fn send(&self, busy: bool, text: &str) {
        self.send_raw(serde_json::json!({"busy": busy, "text": text}).to_string());
    }

    /// Publishes one raw payload to every connected subscriber.
    fn send_raw(&self, payload: String) {
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
async fn a_busy_snapshot_reaches_the_status_bar_as_a_busy_frame_with_the_gateway_text() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let recorder = recorder();
    // The flag starts optimistic, so the subscriber connects at once.
    let subscriber = spawn_with_timing(
        binding(&base_url),
        recorder.push.clone(),
        GatewayHealth::new(),
        FAST_TIMING,
    );

    wait_for_connections(&mock, 1).await;
    mock.send(true, "Downloading qwen3-8b.gguf 45%");

    let pushed = recorder.frames_where(|frames| !frames.is_empty()).await;
    assert_eq!(
        pushed,
        [("Downloading qwen3-8b.gguf 45%".to_owned(), true)],
        "the gateway's text is the bar's label and the frame is busy"
    );

    mock.send(false, "");
    let pushed = recorder.frames_where(|frames| frames.len() == 2).await;
    assert_eq!(
        pushed[1],
        ("Ready".to_owned(), false),
        "the idle snapshot rests the bar once the minimum visible time passes"
    );
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_malformed_snapshot_is_skipped_and_the_stream_continues() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let recorder = recorder();
    let subscriber = spawn_with_timing(
        binding(&base_url),
        recorder.push.clone(),
        GatewayHealth::new(),
        FAST_TIMING,
    );

    wait_for_connections(&mock, 1).await;
    // One undecodable `data:` block ahead of a valid snapshot: the
    // subscriber warns and continues rather than dropping the stream.
    mock.send_raw("{not valid json".to_owned());
    mock.send(true, "Loading profile");

    let pushed = recorder.frames_where(|frames| !frames.is_empty()).await;
    assert_eq!(
        pushed,
        [("Loading profile".to_owned(), true)],
        "the snapshot after the malformed one still reaches the bar"
    );
    assert_eq!(
        mock.connections.load(Ordering::Relaxed),
        1,
        "a malformed snapshot never drops the subscription"
    );
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_stream_that_ends_while_reachable_rests_the_bar_and_resubscribes_after_the_delay() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let recorder = recorder();
    let subscriber = spawn_with_timing(
        binding(&base_url),
        recorder.push.clone(),
        GatewayHealth::new(),
        FAST_TIMING,
    );

    wait_for_connections(&mock, 1).await;
    mock.send(true, "Downloading");
    recorder.frames_where(|frames| frames.len() == 1).await;

    // The stream ends while the gateway still reads reachable: the bar
    // rests once its minimum visible time lapses, which falls inside the
    // resubscribe wait, and a fresh subscription follows the delay.
    let closed = std::time::Instant::now();
    mock.close();
    let pushed = recorder.frames_where(|frames| frames.len() == 2).await;
    assert_eq!(
        pushed[1],
        ("Ready".to_owned(), false),
        "a lost subscription rests the bar: its progress is stale"
    );
    wait_for_connections(&mock, 2).await;
    assert!(
        closed.elapsed() >= FAST_TIMING.resubscribe_delay,
        "the resubscribe waits out the delay rather than spinning"
    );
    subscriber.shutdown().await;
}

#[tokio::test]
async fn an_unreachable_gateway_holds_no_subscription_and_pushes_nothing() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let recorder = recorder();
    let health = GatewayHealth::new();
    health.publish(false);
    let subscriber = spawn_with_timing(
        binding(&base_url),
        recorder.push.clone(),
        health.clone(),
        FAST_TIMING,
    );

    let quiet = tokio::time::timeout(Duration::from_millis(200), async {
        wait_for_connections(&mock, 1).await;
    })
    .await;
    assert!(
        quiet.is_err(),
        "an unreachable gateway must not be subscribed"
    );
    assert!(
        recorder.pushed().is_empty(),
        "a subscriber that never connected pushes nothing"
    );

    health.publish(true);
    wait_for_connections(&mock, 1).await;
    subscriber.shutdown().await;
}

#[tokio::test]
async fn a_reconnect_rests_the_bar_and_resubscribes_once() {
    let mock = Arc::new(MockProgress::new());
    let base_url = spawn_gateway(Arc::clone(&mock).router()).await;
    let recorder = recorder();
    let health = GatewayHealth::new();
    let subscriber = spawn_with_timing(
        binding(&base_url),
        recorder.push.clone(),
        health.clone(),
        FAST_TIMING,
    );

    wait_for_connections(&mock, 1).await;
    mock.send(true, "Downloading");
    recorder.frames_where(|frames| frames.len() == 1).await;

    health.publish(false);
    let pushed = recorder.frames_where(|frames| frames.len() == 2).await;
    assert_eq!(
        pushed[1],
        ("Ready".to_owned(), false),
        "an unreachable verdict rests the bar"
    );

    health.publish(true);
    wait_for_connections(&mock, 2).await;
    mock.send(true, "Starting models");
    let pushed = recorder.frames_where(|frames| frames.len() == 3).await;
    assert_eq!(
        pushed[2],
        ("Starting models".to_owned(), true),
        "the fresh subscription's snapshots reach the bar"
    );
    assert_eq!(
        mock.connections.load(Ordering::Relaxed),
        2,
        "the reconnect resubscribes exactly once"
    );
    subscriber.shutdown().await;
}

mod presenter;
mod recovery;
