//! Model-menu behavior of the `/ws` workshop socket: `select_model` and
//! `switch_profile` orchestration, the selection ladder without a
//! restart, the no-profile selection, and the single-flight refusal. The
//! sidecar restart ladder lives in the `restart` child.

mod restart;

use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite;

use crate::common::spawn_gateway;

use super::{frames_until, mock_models, spawn_session_server};

/// Every body a recording switch route received, in arrival order.
type ReceivedBodies = Arc<Mutex<Vec<serde_json::Value>>>;

/// A `/admin/switch-profile` route recording each request body and
/// answering `{"profile": profile, "restart_required": restart}`.
fn recording_switch(profile: Option<&'static str>, restart: bool) -> (Router, ReceivedBodies) {
    let received = ReceivedBodies::default();
    let sink = Arc::clone(&received);
    let router = Router::new().route(
        "/admin/switch-profile",
        post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = Arc::clone(&sink);
            async move {
                sink.lock()
                    .expect("the recorder lock is healthy")
                    .push(body);
                axum::Json(serde_json::json!({
                    "profile": profile,
                    "restart_required": restart,
                }))
            }
        }),
    );
    (router, received)
}

/// Declines the selection with the gateway's error envelope.
async fn mock_switch_declined() -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        axum::Json(serde_json::json!({
            "error": {"message": "profile \"beta\" is not defined", "code": "switch_failed"}
        })),
    )
        .into_response()
}

/// The profile endpoints the switch task's refetch hits, reporting
/// `active` (`null` for none) as the served profile.
fn profile_routes(active: Option<&'static str>) -> Router {
    Router::new()
        .route(
            "/admin/profiles",
            get(|| async {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    r#"{"profiles":["main","beta"]}"#,
                )
            }),
        )
        .route(
            "/admin/status",
            get(move || async move { axum::Json(serde_json::json!({"profile": active})) }),
        )
        .route("/v1/models", get(mock_models))
}

/// The `(label, current, total)` of every progress status frame in
/// `frames`, in order.
fn progress_ladder(frames: &[serde_json::Value]) -> Vec<(String, u64, u64)> {
    frames
        .iter()
        .filter(|frame| frame["type"] == "status" && !frame["progress"].is_null())
        .map(|frame| {
            (
                frame["label"]
                    .as_str()
                    .expect("a progress frame carries a label")
                    .to_string(),
                frame["progress"]["current"]
                    .as_u64()
                    .expect("current is an integer"),
                frame["progress"]["total"]
                    .as_u64()
                    .expect("total is an integer"),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_select_model_event_round_trips_and_refusals_answer_errors() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    state
        .catalog()
        .publish(vec![serde_json::json!({"id": "test-model"})]);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let select = serde_json::json!({"type": "select_model", "model": "test-model"}).to_string();
    socket
        .send(tungstenite::Message::Text(select.into()))
        .await
        .expect("the select frame is sent");
    let frames = frames_until(&mut socket, |frame| frame["type"] == "workbench").await;
    let published = frames.last().expect("the accepted frame is last");
    assert_eq!(
        published["selected"], "test-model",
        "the selection round-trips as a workbench push"
    );

    let unknown =
        serde_json::json!({"type": "select_model", "model": "bogus", "id": 3}).to_string();
    socket
        .send(tungstenite::Message::Text(unknown.into()))
        .await
        .expect("the select frame is sent");
    let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
    let refusal = frames.last().expect("the refusal is last");
    assert_eq!(refusal["id"], 3, "the refusal echoes the event id");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the rule")
            .contains("unknown model"),
        "the refusal names the unknown-model rule: {refusal}"
    );

    let missing = serde_json::json!({"type": "select_model"}).to_string();
    socket
        .send(tungstenite::Message::Text(missing.into()))
        .await
        .expect("the select frame is sent");
    let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
    let refusal = frames.last().expect("the refusal is last");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the field")
            .contains("model"),
        "a field-less select is refused, not fatal: {refusal}"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_selection_served_without_a_restart_completes_after_one_step() {
    let (switch, received) = recording_switch(Some("beta"), false);
    let base_url = spawn_gateway(switch.merge(profile_routes(Some("beta")))).await;
    let (url, _state_dir, state) = spawn_session_server(&base_url).await;
    // Readiness needs a non-empty catalog and reachability; seed both
    // so the settled snapshot recomputes chat_ready to true. The seed
    // matches the refetched catalog so the reconcile stays a no-op.
    state.catalog().publish(vec![serde_json::json!(
        {"id": "test-model", "object": "model", "owned_by": "promptforge"}
    )]);
    state.menu().set_gateway_reachable(true);
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": "beta"}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    let frames = frames_until(&mut socket, |frame| {
        frame["type"] == "workbench" && frame["switching"].is_null() && frame["active"] == "beta"
    })
    .await;

    let pending = frames
        .iter()
        .find(|frame| frame["type"] == "workbench" && frame["switching"] == "beta")
        .expect("the pending snapshot was pushed before the settle");
    assert_eq!(
        pending["chat_ready"], false,
        "a switch in flight blocks chat: {pending}"
    );
    assert_eq!(
        progress_ladder(&frames),
        [("Selecting profile...".to_string(), 1, 3)],
        "a selection the gateway already serves climbs no restart steps"
    );
    assert_eq!(
        received
            .lock()
            .expect("the recorder lock is healthy")
            .as_slice(),
        [serde_json::json!({"name": "beta"})],
        "the selection reaches the gateway as its name"
    );
    assert!(
        frames.iter().any(|frame| frame["type"] == "models"),
        "the refetched catalog arrives as a models frame: {frames:?}"
    );

    let settled = frames.last().expect("the settled snapshot is last");
    assert_eq!(
        settled["selected"], "test-model",
        "the settled snapshot selects the profile's model"
    );
    assert_eq!(
        settled["chat_ready"], true,
        "readiness recomputes once the switch settles"
    );
    assert_eq!(
        settled["profiles"],
        serde_json::json!(["main", "beta"]),
        "the refetched profile list rides into the snapshot"
    );

    // The idle push follows the settle; it may already have arrived
    // interleaved with the frames above.
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
async fn a_null_name_selects_no_profile_and_converges_on_a_null_active_profile() {
    let (switch, received) = recording_switch(None, false);
    let base_url = spawn_gateway(switch.merge(profile_routes(None))).await;
    let (url, _state_dir, state) = spawn_session_server(&base_url).await;
    state.catalog().publish(vec![serde_json::json!(
        {"id": "test-model", "object": "model", "owned_by": "promptforge"}
    )]);
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": null}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    // The pending snapshot names no target for a switch to no profile,
    // so readiness is the visible sign of the switch in flight.
    frames_until(&mut socket, |frame| {
        frame["type"] == "workbench" && frame["chat_ready"] == false
    })
    .await;
    let frames = frames_until(&mut socket, |frame| {
        frame["type"] == "workbench" && frame["active"].is_null() && frame["chat_ready"] == true
    })
    .await;

    assert_eq!(
        received
            .lock()
            .expect("the recorder lock is healthy")
            .as_slice(),
        [serde_json::json!({"name": null})],
        "no profile reaches the gateway as an explicit null"
    );
    let settled = frames.last().expect("the settled snapshot is last");
    assert_eq!(
        settled["selected"], "test-model",
        "chat stays usable on the remote catalog with no profile"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_declined_selection_restores_the_menu_and_reports_the_gateway_message() {
    let base_url = spawn_gateway(
        Router::new()
            .route("/admin/switch-profile", post(mock_switch_declined))
            // The gateway still serves the previous profile after the
            // refusal, so its status endpoint keeps naming it.
            .merge(profile_routes(Some("main"))),
    )
    .await;
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
        frame["type"] == "workbench" && frame["switching"].is_null()
    })
    .await;

    let restored = frames.last().expect("the restored snapshot is last");
    assert_eq!(
        restored["active"], "main",
        "the previous profile still serves: {restored}"
    );
    assert_eq!(
        restored["selected"], "test-model",
        "the selection survives the refused switch"
    );
    assert_eq!(
        restored["chat_ready"], true,
        "readiness returns to its truthful pre-switch state"
    );

    // The failure status and the restored snapshot ride different
    // buses, so their wire order is not pinned; read on if needed.
    let is_failure =
        |frame: &serde_json::Value| frame["type"] == "status" && frame["severity"] == "error";
    if !frames.iter().any(is_failure) {
        frames.extend(frames_until(&mut socket, is_failure).await);
    }
    let failure = frames
        .iter()
        .find(|frame| is_failure(frame))
        .expect("the failure status was pushed");
    assert_eq!(failure["label"], "Profile switch failed");
    assert_eq!(
        failure["description"], "profile \"beta\" is not defined",
        "the gateway's own message is reported"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_switch_while_one_runs_is_refused_with_an_error_frame() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    state
        .menu()
        .begin_switch(Some("running"))
        .expect("no switch is in flight");
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    let switch = serde_json::json!({"type": "switch_profile", "name": "beta", "id": 9}).to_string();
    socket
        .send(tungstenite::Message::Text(switch.into()))
        .await
        .expect("the switch frame is sent");
    let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
    let refusal = frames.last().expect("the refusal is last");
    assert_eq!(refusal["id"], 9, "the refusal echoes the event id");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the rule")
            .contains("already in progress"),
        "the refusal names the single-flight rule: {refusal}"
    );

    let nameless = serde_json::json!({"type": "switch_profile", "id": 10}).to_string();
    socket
        .send(tungstenite::Message::Text(nameless.into()))
        .await
        .expect("the switch frame is sent");
    let frames = frames_until(&mut socket, |frame| frame["type"] == "error").await;
    let refusal = frames.last().expect("the refusal is last");
    assert_eq!(refusal["id"], 10);
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the field")
            .contains("null for no profile"),
        "an absent name is refused and the refusal names the null form: {refusal}"
    );
    socket.close(None).await.expect("close the socket");
}
