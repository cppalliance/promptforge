//! The gateway progress subscriber: a background task that imports the
//! gateway's `GET /admin/progress` event stream into the workshop
//! [`ProgressHub`] as a [`RemoteOperation`], so gateway-side work (model
//! downloads, profile switches) renders on the status bar through the same
//! renderer task as local operations.
//!
//! The task follows the heartbeat's lifecycle posture: spawned with the
//! server, stopped through its [`Subscriber`] handle inside the same
//! graceful-shutdown signal, and driven by the shared [`GatewayHealth`]
//! verdict rather than by probes of its own. It subscribes while the
//! gateway reads reachable and idles while it does not; a reconnect
//! resubscribes, and each subscription attaches a fresh import, so a
//! gateway that flaps never stacks duplicate remote state on the hub.
//! When the subscription drops - a lost connection or an unreachable
//! verdict - the import detaches with it, because progress from a gateway
//! the workshop can no longer hear is stale, not informative.

use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::oneshot;

use promptforge_model_client::model::subscribe_progress;
use promptforge_progress::{ProgressHub, RemoteOperation};

use crate::heartbeat::GatewayHealth;

/// How long a resubscribe waits when the stream ended while the gateway
/// still reads reachable, so an endpoint that accepts and immediately
/// closes cannot spin the loop. A reachability flip restarts at once;
/// matched to the heartbeat's probe cadence.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

/// A running subscriber task.
///
/// [`Subscriber::shutdown`] signals the task to stop and awaits it.
/// Dropping the handle without shutting down still stops the task at its
/// next select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub(crate) struct Subscriber {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Subscriber {
    /// Signals the subscriber to stop and waits for its task to finish.
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the subscriber task against the gateway at `base_url`,
/// importing its progress events into `hub` while `health` reads
/// reachable.
#[must_use]
pub(crate) fn spawn(
    base_url: String,
    api_key: String,
    hub: Arc<ProgressHub>,
    health: GatewayHealth,
) -> Subscriber {
    spawn_with_delay(base_url, api_key, hub, health, RESUBSCRIBE_DELAY)
}

/// [`spawn`] with the resubscribe delay injected, so tests can shorten it.
fn spawn_with_delay(
    base_url: String,
    api_key: String,
    hub: Arc<ProgressHub>,
    health: GatewayHealth,
    resubscribe_delay: Duration,
) -> Subscriber {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(
            &base_url,
            &api_key,
            &hub,
            &health,
            resubscribe_delay,
            &mut stopped,
        )
        .await;
    });
    Subscriber {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The subscription loop: idle while the gateway is unreachable, and while
/// reachable hold one subscription whose events drive one
/// [`RemoteOperation`]. The stop signal wins every select, so shutdown
/// never waits out a stream read, a connect, or a resubscribe delay.
async fn run(
    base_url: &str,
    api_key: &str,
    hub: &Arc<ProgressHub>,
    health: &GatewayHealth,
    resubscribe_delay: Duration,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut reachable = health.subscribe();
    loop {
        while !*reachable.borrow_and_update() {
            tokio::select! {
                _ = &mut *stop => return,
                changed = reachable.changed() => {
                    // The sender lives in AppState for the process
                    // lifetime, so a closed watch means shutdown.
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        let stream = tokio::select! {
            _ = &mut *stop => return,
            _ = reachable.changed() => continue,
            result = subscribe_progress(base_url, api_key) => match result {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "gateway progress subscription failed");
                    tokio::select! {
                        _ = &mut *stop => return,
                        _ = reachable.changed() => {}
                        () = tokio::time::sleep(resubscribe_delay) => {}
                    }
                    continue;
                }
            },
        };
        let remote = RemoteOperation::attach(hub);
        tokio::pin!(stream);
        loop {
            tokio::select! {
                _ = &mut *stop => return,
                _ = reachable.changed() => break,
                item = stream.next() => match item {
                    Some(Ok(event)) => remote.apply(&event),
                    // One malformed event or a terminal read failure; the
                    // stream itself decides which by continuing or ending.
                    Some(Err(error)) => {
                        tracing::warn!(%error, "gateway progress event skipped");
                    }
                    None => break,
                }
            }
        }
        drop(remote);
        if *reachable.borrow_and_update() {
            tokio::select! {
                _ = &mut *stop => return,
                _ = reachable.changed() => {}
                () = tokio::time::sleep(resubscribe_delay) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // Fractions are fixed-point millionths, so equality comparisons are exact
    // (the promptforge-progress remote.rs test precedent).
    #![expect(clippy::float_cmp, reason = "fixed-point fractions compare exactly")]

    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::extract::State;
    use axum::response::{IntoResponse, Response};
    use tokio::sync::broadcast;

    use promptforge_progress::OperationSnapshot;

    use crate::app::fixtures::spawn_gateway;

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
        let subscriber = spawn(
            base_url,
            String::new(),
            Arc::clone(&hub),
            GatewayHealth::new(),
        );

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
        let subscriber = spawn(
            base_url,
            String::new(),
            Arc::clone(&hub),
            GatewayHealth::new(),
        );

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
            base_url,
            String::new(),
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
        let subscriber = spawn(base_url, String::new(), Arc::clone(&hub), health.clone());

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
        let subscriber = spawn(base_url, String::new(), Arc::clone(&hub), health.clone());

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
}
