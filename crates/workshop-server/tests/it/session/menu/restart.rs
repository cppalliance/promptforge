//! The sidecar restart ladder of a `switch_profile` event: a selection
//! the gateway must restart to load shuts the supervised sidecar down,
//! waits for the supervisor's replacement generation, and refreshes
//! through it; a LAN gateway settles with the restart notice instead;
//! a replacement that never appears fails within the bound.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::Router;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite;

use workshop_server::fixtures::{
    ValidatedGateway, gateway_updater, replace_gateway, run_validated_gateway_fixture_process,
    state_with_gateway, state_with_gateway_and_restart_bound,
};
use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, ResolvedGateway, ServerConfig, router,
};

use crate::common::spawn_gateway;

use super::super::{frames_until, spawn_session_server};
use super::{profile_routes, progress_ladder, recording_switch};

/// The bearer the sidecar fixture expects and the workshop presents.
const SIDECAR_KEY: &str = "sidecar-key";

/// How long the fixture may take to report the shutdown request.
const SHUTDOWN_OBSERVATION: Duration = Duration::from_secs(10);

/// The named child-process half of [`ValidatedGateway`]: the fixture
/// spawns a copy of this test binary with this test's name, so the name
/// must stay in sync with the `spawn_in` call below.
#[test]
#[ignore = "runs only as a named child process"]
fn validated_gateway_fixture_process() {
    run_validated_gateway_fixture_process();
}

/// A gateway API mock that dies with the switch: the selection answers
/// `restart_required: true` and flips `dead`, after which every other
/// route answers 503, so a catalog or status read against the old
/// generation after the shutdown would fail loudly.
fn dying_sidecar_api() -> Router {
    let dead = Arc::new(AtomicBool::new(false));
    let unless_dead = |dead: &Arc<AtomicBool>, body: serde_json::Value| -> Response {
        if dead.load(Ordering::Relaxed) {
            StatusCode::SERVICE_UNAVAILABLE.into_response()
        } else {
            axum::Json(body).into_response()
        }
    };
    let on_switch = Arc::clone(&dead);
    let on_status = Arc::clone(&dead);
    let on_profiles = Arc::clone(&dead);
    let on_models = Arc::clone(&dead);
    Router::new()
        .route(
            "/admin/switch-profile",
            post(move || {
                let dead = Arc::clone(&on_switch);
                async move {
                    dead.store(true, Ordering::Relaxed);
                    axum::Json(serde_json::json!({"profile": "beta", "restart_required": true}))
                }
            }),
        )
        .route(
            "/admin/status",
            get(move || {
                let dead = Arc::clone(&on_status);
                async move { unless_dead(&dead, serde_json::json!({"profile": "main"})) }
            }),
        )
        .route(
            "/admin/profiles",
            get(move || {
                let dead = Arc::clone(&on_profiles);
                async move { unless_dead(&dead, serde_json::json!({"profiles": ["main", "beta"]})) }
            }),
        )
        .route(
            "/v1/models",
            get(move || {
                let dead = Arc::clone(&on_models);
                async move {
                    unless_dead(
                        &dead,
                        serde_json::json!({"object": "list", "data": [
                            {"id": "model-before-restart", "object": "model"}
                        ]}),
                    )
                }
            }),
        )
}

/// The relaunched sidecar's API: it serves `beta` and a catalog that
/// only it can have produced.
fn relaunched_sidecar_api() -> Router {
    Router::new()
        .route(
            "/admin/status",
            get(|| async { axum::Json(serde_json::json!({"profile": "beta"})) }),
        )
        .route(
            "/admin/profiles",
            get(|| async { axum::Json(serde_json::json!({"profiles": ["main", "beta"]})) }),
        )
        .route(
            "/v1/models",
            get(|| async {
                axum::Json(serde_json::json!({"object": "list", "data": [
                    {"id": "model-after-restart", "object": "model"}
                ]}))
            }),
        )
}

/// A workshop server whose gateway snapshot is a supervised sidecar: the
/// validated identity (shutdown authority) is the named fixture process,
/// and the API surface is the mock at `api_url`. Returns the `/ws` URL,
/// the state directory, the shared state, and the fixture for shutdown
/// observation.
async fn spawn_sidecar_server(
    api_url: &str,
    restart_bound: Option<Duration>,
) -> (String, tempfile::TempDir, AppState, ValidatedGateway) {
    let gateway = ValidatedGateway::spawn_in(
        SIDECAR_KEY,
        "session::menu::restart::validated_gateway_fixture_process",
    );
    let identity = gateway.validate(SIDECAR_KEY, 1_778_000_001, "2026-09-15T18:00:01Z");
    let resolved = ResolvedGateway::from_validated(identity).with_base_url(api_url);
    let state_dir = tempfile::TempDir::new().expect("tempdir");
    let config = Config {
        gateway: GatewayConfig {
            base_url: api_url.to_string(),
            api_key: SIDECAR_KEY.to_string(),
        },
        server: ServerConfig {
            state_dir: state_dir.path().to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig::default(),
    };
    let state = match restart_bound {
        Some(bound) => state_with_gateway_and_restart_bound(&config, &resolved, bound),
        None => state_with_gateway(&config, &resolved),
    }
    .expect("state builds in tests");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the session test server");
    let addr = listener.local_addr().expect("session test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("session test server serves");
    });
    (format!("ws://{addr}/ws"), state_dir, state, gateway)
}

/// Waits off the executor for the fixture to report the shutdown
/// request, handing the fixture back.
async fn observe_shutdown(mut gateway: ValidatedGateway) -> (ValidatedGateway, bool) {
    tokio::task::spawn_blocking(move || {
        let hit = gateway.received_shutdown(SHUTDOWN_OBSERVATION);
        (gateway, hit)
    })
    .await
    .expect("the observation thread completes")
}

#[tokio::test]
async fn a_sidecar_restart_climbs_the_ladder_and_refreshes_through_the_replacement() {
    let dying_url = spawn_gateway(dying_sidecar_api()).await;
    let (url, _state_dir, state, gateway) = spawn_sidecar_server(&dying_url, None).await;
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    let mut frames = frames_until(&mut socket, |frame| {
        frame["type"] == "status" && frame["label"] == "Restarting gateway..."
    })
    .await;

    let (_gateway, shutdown_hit) = observe_shutdown(gateway).await;
    assert!(
        shutdown_hit,
        "the ladder posts the authenticated shutdown to the sidecar identity"
    );

    // The supervisor's relaunch: a higher generation at a different
    // port, serving the selected profile.
    let relaunched_url = spawn_gateway(relaunched_sidecar_api()).await;
    assert_ne!(
        relaunched_url, dying_url,
        "the replacement binds a fresh port"
    );
    replace_gateway(&gateway_updater(&state), &relaunched_url, SIDECAR_KEY)
        .expect("the replacement generation publishes");

    frames.extend(
        frames_until(&mut socket, |frame| {
            frame["type"] == "workbench"
                && frame["switching"].is_null()
                && frame["active"] == "beta"
        })
        .await,
    );
    assert_eq!(
        progress_ladder(&frames),
        [
            ("Selecting profile...".to_string(), 1, 3),
            ("Restarting gateway...".to_string(), 2, 3),
            ("Loading models...".to_string(), 3, 3),
        ],
        "the three steps arrive as determinate progress, in order"
    );
    let catalog = frames
        .iter()
        .find(|frame| frame["type"] == "models")
        .expect("the refetched catalog arrives as a models frame");
    assert_eq!(
        catalog["models"],
        serde_json::json!([{"id": "model-after-restart", "object": "model"}]),
        "the catalog refresh hits the replacement port, never the dead one"
    );
    let settled = frames.last().expect("the settled snapshot is last");
    assert_eq!(
        settled["selected"], "model-after-restart",
        "the settled snapshot selects from the replacement's catalog"
    );
    assert_eq!(settled["chat_ready"], true);
    if !frames
        .iter()
        .any(|frame| frame["type"] == "status" && frame["label"] == "Ready")
    {
        frames_until(&mut socket, |frame| {
            frame["type"] == "status" && frame["label"] == "Ready"
        })
        .await;
    }
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_replacement_that_never_appears_fails_within_the_bound() {
    let dying_url = spawn_gateway(dying_sidecar_api()).await;
    let (url, _state_dir, state, gateway) =
        spawn_sidecar_server(&dying_url, Some(Duration::from_millis(600))).await;
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    frames_until(&mut socket, |frame| {
        frame["type"] == "status" && frame["label"] == "Restarting gateway..."
    })
    .await;
    let (_gateway, shutdown_hit) = observe_shutdown(gateway).await;
    assert!(shutdown_hit, "the shutdown went out before the wait began");

    let frames = frames_until(&mut socket, |frame| {
        frame["type"] == "status" && frame["severity"] == "error"
    })
    .await;
    let failure = frames.last().expect("the failure status is last");
    assert_eq!(failure["label"], "Profile switch failed");
    assert_eq!(
        failure["description"], "gateway did not return after restart",
        "the timeout names the missing replacement"
    );
    let restored = frames_until(&mut socket, |frame| {
        frame["type"] == "workbench" && frame["switching"].is_null()
    })
    .await;
    let restored = restored.last().expect("the restored snapshot is last");
    assert_eq!(
        restored["active"], "main",
        "the menu keeps the last known profile after a failed restart"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_lan_gateway_that_needs_a_restart_settles_with_the_notice() {
    let (switch, received) = recording_switch(Some("beta"), true);
    let base_url = spawn_gateway(switch.merge(profile_routes(Some("main")))).await;
    let (url, _state_dir, state) = spawn_session_server(&base_url).await;
    state.catalog().publish(vec![serde_json::json!(
        {"id": "test-model", "object": "model", "owned_by": "promptforge"}
    )]);
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    state
        .menu()
        .set_selected("test-model")
        .expect("the id is in the catalog");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    frames_until(&mut socket, |frame| {
        frame["type"] == "workbench" && frame["switching"] == "beta"
    })
    .await;
    let mut frames = frames_until(&mut socket, |frame| {
        frame["type"] == "status" && frame["label"] == "Profile selected"
    })
    .await;
    let notice = frames.last().expect("the notice is last");
    assert_eq!(
        notice["description"], "profile \"beta\" selected; restart the gateway to load it",
        "a gateway the workshop does not supervise is restarted by hand"
    );
    assert_eq!(notice["severity"], "info", "the notice is not a failure");
    assert_eq!(
        progress_ladder(&frames),
        [("Selecting profile...".to_string(), 1, 3)],
        "no restart step runs against a LAN gateway"
    );
    assert_eq!(
        received.lock().expect("the recorder lock is healthy").len(),
        1,
        "the selection was persisted exactly once"
    );
    // Every frame read so far follows the pending snapshot, so a settled
    // snapshot among them is the finish, not the connect-time replay.
    let is_settled =
        |frame: &serde_json::Value| frame["type"] == "workbench" && frame["switching"].is_null();
    if !frames.iter().any(is_settled) {
        frames.extend(frames_until(&mut socket, is_settled).await);
    }
    let settled = frames
        .iter()
        .find(|frame| is_settled(frame))
        .expect("the settled snapshot was pushed");
    assert_eq!(
        settled["active"], "main",
        "the running profile is unchanged until the operator restarts"
    );
    assert_eq!(
        settled["chat_ready"], true,
        "chat stays usable on the running profile"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_lan_gateway_deferring_no_profile_settles_with_the_unload_notice() {
    let (switch, received) = recording_switch(None, true);
    let base_url = spawn_gateway(switch.merge(profile_routes(Some("main")))).await;
    let (url, _state_dir, state) = spawn_session_server(&base_url).await;
    state.catalog().publish(vec![serde_json::json!(
        {"id": "test-model", "object": "model", "owned_by": "promptforge"}
    )]);
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    state
        .menu()
        .set_selected("test-model")
        .expect("the id is in the catalog");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": null}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    let frames = frames_until(&mut socket, |frame| {
        frame["type"] == "status" && frame["label"] == "Profile selected"
    })
    .await;
    let notice = frames.last().expect("the notice is last");
    assert_eq!(
        notice["description"],
        "no profile selected; restart the gateway to unload the running profile",
        "the null selection reads as its own sentence, not as a profile named \"no profile\""
    );
    assert_eq!(notice["severity"], "info", "the notice is not a failure");
    assert_eq!(
        received
            .lock()
            .expect("the recorder lock is healthy")
            .as_slice(),
        [serde_json::json!({"name": null})],
        "the null selection reached the gateway exactly once"
    );
    socket.close(None).await.expect("close the socket");
}
