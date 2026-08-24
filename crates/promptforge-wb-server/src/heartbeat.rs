//! The gateway heartbeat: a background task polling the gateway's
//! `GET /health` endpoint and publishing reachability to the rest of the
//! server.
//!
//! One task is spawned with the server ([`spawn`]) and loops on the fixed
//! [`HEARTBEAT_INTERVAL`]: each tick probes the gateway through
//! [`GatewayClient::health`] and publishes the outcome to the shared
//! [`GatewayHealth`] flag the gateway-dependent routes read. The observer
//! hears about transitions only - the first probe reports the initial state
//! ("Connected to gateway" or "Gateway unreachable"), and after that a
//! status update fires when the answer changes, so a steady state never
//! spams the status bar.
//!
//! The task stops through its [`Heartbeat`] handle: the signal wins the
//! loop's selects, so shutdown never waits out a tick or an in-flight
//! probe. The server runs the shutdown inside its graceful-shutdown future.

use std::time::Duration;

use tokio::sync::{oneshot, watch};

use crate::gateway::GatewayClient;
use crate::status::{Activity, StatusBus};

/// How often the heartbeat probes the gateway. Hardcoded for now; a
/// configuration knob may follow once someone needs one.
pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Shared gateway reachability, written by the heartbeat and read by the
/// gateway-dependent routes.
///
/// The flag starts optimistic (`true`): until the first probe lands, a
/// request flows to the gateway and fails or succeeds on its own merits,
/// which keeps a server running without a heartbeat (every router-only
/// test) behaving exactly as it did before the heartbeat existed.
#[derive(Debug, Clone)]
pub(crate) struct GatewayHealth {
    reachable: watch::Sender<bool>,
}

impl GatewayHealth {
    /// Starts the flag optimistic; see the type docs for why.
    pub(crate) fn new() -> Self {
        Self {
            reachable: watch::channel(true).0,
        }
    }

    /// Whether the gateway is currently believed reachable.
    // The chat and catalog routes read this from the graceful-degradation
    // commit; today only the heartbeat's own tests do. Those tests make an
    // `expect` unfulfilled in the test target, so this is an `allow`.
    #[allow(
        dead_code,
        reason = "the chat and catalog routes read it two commits on"
    )]
    pub(crate) fn is_reachable(&self) -> bool {
        *self.reachable.borrow()
    }

    /// Publishes one probe outcome. The heartbeat is the only production
    /// writer; tests publish directly to pin the degraded paths.
    pub(crate) fn publish(&self, reachable: bool) {
        self.reachable.send_if_modified(|current| {
            let changed = *current != reachable;
            *current = reachable;
            changed
        });
    }
}

/// A running heartbeat task.
///
/// [`Heartbeat::shutdown`] signals the loop to stop and awaits the task.
/// Dropping the handle without shutting down still stops the task at its
/// next select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub(crate) struct Heartbeat {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Heartbeat {
    /// Signals the heartbeat to stop and waits for its task to finish.
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the heartbeat loop against `client`, reporting transitions
/// through `status` and publishing reachability to `health`. The first
/// probe runs immediately, before the first interval elapses.
pub(crate) fn spawn(
    client: GatewayClient,
    status: StatusBus,
    health: GatewayHealth,
    interval: Duration,
) -> Heartbeat {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&client, &status, &health, interval, &mut stopped).await;
    });
    Heartbeat {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The probe loop: one probe per interval, a status update per transition,
/// and the stop signal wins over both the tick and an in-flight probe.
async fn run(
    client: &GatewayClient,
    status: &StatusBus,
    health: &GatewayHealth,
    interval: Duration,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut ticks = tokio::time::interval(interval);
    // A probe slower than the interval (the health timeout bounds it at two
    // seconds) must not bunch the missed ticks into a catch-up burst.
    ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut last: Option<bool> = None;
    loop {
        tokio::select! {
            _ = &mut *stop => break,
            _ = ticks.tick() => {}
        }
        let reachable = tokio::select! {
            _ = &mut *stop => break,
            reachable = client.health() => reachable,
        };
        health.publish(reachable);
        if last == Some(reachable) {
            continue;
        }
        last = Some(reachable);
        if reachable {
            status.info(
                "Connected to gateway",
                "the gateway answers its health probe",
                Activity::Gateway,
            );
        } else {
            status.info(
                "Gateway unreachable",
                "the gateway does not answer its health probe",
                Activity::Gateway,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use axum::Router;
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Response};
    use axum::routing::get;
    use tokio::sync::broadcast;

    use crate::status::{Severity, StatusBarUpdate};

    /// Fast enough to observe transitions without real waiting, slow
    /// enough that a 200 ms quiet window spans several ticks and so proves
    /// the loop does not re-emit a steady state.
    const TEST_INTERVAL: Duration = Duration::from_millis(25);

    /// A mock `/health` whose answer flips under test control.
    async fn flippable_health(State(healthy): State<Arc<AtomicBool>>) -> Response {
        if healthy.load(Ordering::Relaxed) {
            StatusCode::OK.into_response()
        } else {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        }
    }

    /// Binds a mock gateway whose `/health` flips with `healthy`.
    async fn spawn_gateway(healthy: Arc<AtomicBool>) -> String {
        let app = Router::new()
            .route("/health", get(flippable_health))
            .with_state(healthy);
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

    /// Starts a heartbeat against `base_url` on the fast interval, wired to
    /// `status`; returns the handle and the shared health flag.
    fn heartbeat_on(base_url: &str, status: &StatusBus) -> (Heartbeat, GatewayHealth) {
        let client = GatewayClient::new(base_url, "").expect("client builds in tests");
        let health = GatewayHealth::new();
        let heartbeat = spawn(client, status.clone(), health.clone(), TEST_INTERVAL);
        (heartbeat, health)
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

    #[tokio::test]
    async fn a_healthy_gateway_fires_connected_once_and_stays_quiet() {
        let healthy = Arc::new(AtomicBool::new(true));
        let base_url = spawn_gateway(Arc::clone(&healthy)).await;
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let (heartbeat, health) = heartbeat_on(&base_url, &status);

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Connected to gateway");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::Gateway);
        assert!(health.is_reachable(), "the probe published reachable");
        assert_quiet(&mut rx).await;
        heartbeat.shutdown().await;
    }

    #[tokio::test]
    async fn an_unreachable_gateway_fires_unreachable_once_and_stays_quiet() {
        // Nothing listens on port 1, so the connect fails deterministically.
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let (heartbeat, health) = heartbeat_on("http://127.0.0.1:1", &status);

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Gateway unreachable");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::Gateway);
        assert!(!health.is_reachable(), "the probe published unreachable");
        assert_quiet(&mut rx).await;
        heartbeat.shutdown().await;
    }

    #[tokio::test]
    async fn each_transition_fires_exactly_one_update() {
        let healthy = Arc::new(AtomicBool::new(true));
        let base_url = spawn_gateway(Arc::clone(&healthy)).await;
        let status = StatusBus::new();
        let mut rx = status.subscribe();
        let (heartbeat, health) = heartbeat_on(&base_url, &status);

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
    async fn shutdown_stops_the_task_without_waiting_out_the_interval() {
        // A long interval: if the stop signal did not win the select, the
        // shutdown would block for the whole minute.
        let status = StatusBus::new();
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let heartbeat = spawn(
            client,
            status,
            GatewayHealth::new(),
            Duration::from_secs(60),
        );
        tokio::time::timeout(Duration::from_secs(5), heartbeat.shutdown())
            .await
            .expect("shutdown does not wait out the interval");
    }
}
