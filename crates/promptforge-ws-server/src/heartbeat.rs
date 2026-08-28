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
//! spams the status bar. Every transition also feeds the Model menu's
//! reachability (so `chat_ready` flips with the gateway), and a
//! transition to reachable - boot's first probe included - refreshes the
//! gateway's profile state and model catalog into their buses and then
//! restores a model selection when none is applied, so a fresh boot
//! lands ready to chat without a manual pick.
//!
//! The task stops through its [`Heartbeat`] handle: the signal wins the
//! loop's selects, so shutdown never waits out a tick or an in-flight
//! probe. The server runs the shutdown inside its graceful-shutdown future.

use std::time::Duration;

use tokio::sync::{oneshot, watch};

use crate::gateway::GatewayClient;
use crate::protocol::Activity;
use crate::push::Push;

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
    pub(crate) fn is_reachable(&self) -> bool {
        *self.reachable.borrow()
    }

    /// Subscribes to reachability changes. The current value is visible
    /// immediately through the receiver; each later publish that flips the
    /// flag notifies. The provisioning task waits on this to run its cache
    /// calls only while the gateway answers.
    pub(crate) fn subscribe(&self) -> watch::Receiver<bool> {
        self.reachable.subscribe()
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
/// through `push` and publishing reachability to `health` and to the
/// menu behind `push`, which recomputes `chat_ready` from it. A
/// transition to reachable - boot's first probe included - refreshes the
/// gateway's profile state and model catalog through the same handle,
/// then restores a model selection when none is applied. The first probe
/// runs immediately, before the first interval elapses.
pub(crate) fn spawn(
    client: GatewayClient,
    push: Push,
    health: GatewayHealth,
    interval: Duration,
) -> Heartbeat {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&client, &push, &health, interval, &mut stopped).await;
    });
    Heartbeat {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The probe loop: one probe per interval, a status update per transition,
/// and the stop signal wins over the tick, an in-flight probe, an
/// in-flight profile refresh, and an in-flight catalog refresh.
async fn run(
    client: &GatewayClient,
    push: &Push,
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
        // The menu recomputes chat_ready from reachability, so the
        // verdict feeds it before any slower refresh work below.
        push.menu().set_gateway_reachable(reachable);
        if reachable {
            push.push_status_update(
                "Connected to gateway",
                "the gateway answers its health probe",
                Activity::General,
            );
            // All menu state is server-owned and reaches the UI via
            // socket pushes - the UI fetches nothing on boot - so every
            // transition into reachable, boot's first probe included,
            // (re)populates the profile state and the model catalog. A
            // gateway that was down and answers again may also serve a
            // different catalog than before the outage. The refreshes
            // are independent fetches, joined as the profile-switch
            // task joins them.
            tokio::select! {
                _ = &mut *stop => break,
                () = async {
                    tokio::join!(refresh_profiles(client, push), refresh_catalog(client, push));
                } => {}
            }
            // A fresh boot has no selection, so restore the remembered
            // model for the now-known active profile (else the first
            // catalog model); a reconnect whose selection survived the
            // outage is a no-op.
            push.menu().restore_selection();
        } else {
            push.push_status_update(
                "Gateway unreachable",
                "the gateway does not answer its health probe",
                Activity::General,
            );
        }
    }
}

/// Re-fetches the gateway's model catalog and pushes it to every session.
///
/// A failed, declined, or malformed catalog is logged and skipped rather
/// than pushed: pushing a bad snapshot would clear pickers that still hold
/// a usable list. Runs on every transition into reachable (boot and
/// reconnect) and is shared with the profile-switch task in
/// [`crate::chat_ws`], which refetches after a switch settles.
pub(crate) async fn refresh_catalog(client: &GatewayClient, push: &Push) {
    let response = match client.list_models().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "catalog refresh failed");
            return;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "catalog refresh was declined");
        return;
    }
    let body: serde_json::Value = match serde_json::from_slice(&response.body) {
        Ok(body) => body,
        Err(error) => {
            tracing::warn!(%error, "catalog refresh was not JSON");
            return;
        }
    };
    let Some(models) = body.get("data").and_then(serde_json::Value::as_array) else {
        tracing::warn!("catalog refresh carried no data array");
        return;
    };
    push.push_models_catalog(models.clone());
}

/// The decoded body of `GET /admin/profiles`.
#[derive(serde::Deserialize)]
struct ProfileList {
    /// Every profile name the gateway can serve, in gateway order.
    profiles: Vec<String>,
}

/// The decoded body of `GET /admin/status`, reduced to the one field the
/// menu needs.
#[derive(serde::Deserialize)]
struct ProfileStatus {
    /// The profile the gateway is serving.
    #[serde(default)]
    profile: Option<String>,
}

/// Fetches the gateway's profile list and active profile and publishes
/// them into the workbench snapshot.
///
/// A gateway without profile support is a state, not an error: a failed,
/// declined, or malformed answer degrades that half to empty (logged by
/// its fetcher), so the menu shows no profiles rather than stale names.
/// Shared with the profile-switch task in [`crate::chat_ws`], which
/// refetches after a switch settles.
pub(crate) async fn refresh_profiles(client: &GatewayClient, push: &Push) {
    let (profiles, active) = tokio::join!(fetch_profile_list(client), fetch_active_profile(client));
    push.menu()
        .set_profiles(profiles.unwrap_or_default(), active);
}

/// The gateway's profile names from `GET /admin/profiles`, or `None`
/// when the request fails, is declined, or answers malformed JSON - each
/// logged and tolerated.
async fn fetch_profile_list(client: &GatewayClient) -> Option<Vec<String>> {
    let response = match client.list_profiles().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "profile list fetch failed");
            return None;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "profile list was declined");
        return None;
    }
    match serde_json::from_slice::<ProfileList>(&response.body) {
        Ok(list) => Some(list.profiles),
        Err(error) => {
            tracing::warn!(%error, "profile list was not the expected JSON");
            None
        }
    }
}

/// The active profile name from `GET /admin/status`, or `None` when the
/// request fails, is declined, or answers malformed JSON - each logged
/// and tolerated.
async fn fetch_active_profile(client: &GatewayClient) -> Option<String> {
    let response = match client.profile_status().await {
        Ok(response) => response,
        Err(error) => {
            tracing::warn!(%error, "profile status fetch failed");
            return None;
        }
    };
    if !response.status.is_success() {
        tracing::warn!(status = %response.status, "profile status was declined");
        return None;
    }
    match serde_json::from_slice::<ProfileStatus>(&response.body) {
        Ok(status) => status.profile,
        Err(error) => {
            tracing::warn!(%error, "profile status was not the expected JSON");
            None
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

    use crate::catalog::CatalogBus;
    use crate::menu::MenuBus;
    use crate::protocol::{CatalogPush, Severity, StatusBarUpdate, WorkbenchSnapshot};
    use crate::status::StatusBus;

    /// Fast enough to observe transitions without real waiting, slow
    /// enough that a 200 ms quiet window spans several ticks and so proves
    /// the loop does not re-emit a steady state.
    const TEST_INTERVAL: Duration = Duration::from_millis(25);

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","owned_by":"promptforge"}]}"#;

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

    /// Starts a heartbeat against `base_url` on the fast interval, wired to
    /// `status` and `catalog`; returns the handle, the shared health flag,
    /// and the menu bus the heartbeat feeds.
    fn heartbeat_on(
        base_url: &str,
        status: &StatusBus,
        catalog: &CatalogBus,
    ) -> (Heartbeat, GatewayHealth, MenuBus) {
        let client = GatewayClient::new(base_url, "").expect("client builds in tests");
        let health = GatewayHealth::new();
        let menu = MenuBus::new(catalog.clone(), None);
        let heartbeat = spawn(
            client,
            Push::new(status.clone(), catalog.clone(), menu.clone()),
            health.clone(),
            TEST_INTERVAL,
        );
        (heartbeat, health, menu)
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
        let (heartbeat, health, _menu) = heartbeat_on(&base_url, &status, &catalog);

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Connected to gateway");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::General);
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
        let (heartbeat, health, _menu) = heartbeat_on("http://127.0.0.1:1", &status, &catalog);

        let update = next_update(&mut rx).await;
        assert_eq!(update.label, "Gateway unreachable");
        assert_eq!(update.severity, Severity::Info);
        assert_eq!(update.activity, Activity::General);
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
        let (heartbeat, health, _menu) = heartbeat_on(&base_url, &status, &catalog);

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
        let (heartbeat, _health, _menu) = heartbeat_on(&base_url, &status, &catalog);

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
            "the push carries the gateway's data array verbatim"
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
        let (heartbeat, _health, _menu) = heartbeat_on(&base_url, &status, &catalog);

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
    async fn the_initial_connect_pushes_the_catalog_and_readies_chat() {
        // Boot populate: all state reaches the UI via socket pushes, so
        // the first reachable probe fetches the catalog and restores a
        // model selection - a workshop booted against a live gateway is
        // ready to chat with no user interaction.
        let healthy = Arc::new(AtomicBool::new(true));
        let base_url = spawn_gateway(Arc::clone(&healthy)).await;
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        let mut catalog_rx = catalog.subscribe();
        let (heartbeat, _health, menu) = heartbeat_on(&base_url, &status, &catalog);

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
            "the push carries the gateway's data array verbatim"
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
    async fn the_initial_connect_populates_the_profile_state() {
        // Boot populate: the first reachable probe fetches the profile
        // endpoints, so a workshop started against a live gateway shows
        // its profiles without waiting for an outage cycle.
        let healthy = Arc::new(AtomicBool::new(true));
        let base_url = spawn_gateway(Arc::clone(&healthy)).await;
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        let (heartbeat, _health, menu) = heartbeat_on(&base_url, &status, &catalog);

        let populated = snapshot_where(&menu, |snapshot| !snapshot.profiles.is_empty()).await;
        assert_eq!(populated.profiles, ["coding", "main"]);
        assert_eq!(populated.active.as_deref(), Some("main"));
        heartbeat.shutdown().await;
    }

    #[tokio::test]
    async fn a_down_to_up_transition_publishes_a_populated_snapshot() {
        let healthy = Arc::new(AtomicBool::new(false));
        let base_url = spawn_gateway(Arc::clone(&healthy)).await;
        let status = StatusBus::new();
        let catalog = CatalogBus::new();
        let mut status_rx = status.subscribe();
        let (heartbeat, _health, menu) = heartbeat_on(&base_url, &status, &catalog);

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
        let (heartbeat, _health, menu) = heartbeat_on(&base_url, &status, &catalog);

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
        let (heartbeat, _health, menu) = heartbeat_on(&base_url, &status, &catalog);

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
    async fn shutdown_stops_the_task_without_waiting_out_the_interval() {
        // A long interval: if the stop signal did not win the select, the
        // shutdown would block for the whole minute.
        let status = StatusBus::new();
        let client = GatewayClient::new("http://127.0.0.1:1", "").expect("client builds in tests");
        let catalog = CatalogBus::new();
        let menu = crate::menu::MenuBus::new(catalog.clone(), None);
        let heartbeat = spawn(
            client,
            Push::new(status, catalog, menu),
            GatewayHealth::new(),
            Duration::from_secs(60),
        );
        tokio::time::timeout(Duration::from_secs(5), heartbeat.shutdown())
            .await
            .expect("shutdown does not wait out the interval");
    }
}
