//! Model-menu behavior of the `/ws` workshop socket: `select_model` and
//! `switch_profile` orchestration, the switch progress ladder, and the
//! single-flight refusal.

use axum::Router;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite;

use crate::common::spawn_gateway;

use super::{frames_until, mock_models, spawn_session_server};

/// Streams the full stage ladder then the terminal ready.
async fn mock_switch_succeeds() -> Response {
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        concat!(
            "data: {\"stage\":\"loading-profile\"}\n\n",
            "data: {\"stage\":\"stopping-models\"}\n\n",
            "data: {\"stage\":\"starting-models\"}\n\n",
            "data: {\"status\":\"ready\",\"profile\":\"beta\"}\n\n",
        ),
    )
        .into_response()
}

/// Streams one stage then the terminal error.
async fn mock_switch_fails() -> Response {
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        concat!(
            "data: {\"stage\":\"loading-profile\"}\n\n",
            "data: {\"status\":\"error\",\"message\":\"start-local failed\"}\n\n",
        ),
    )
        .into_response()
}

/// The profile endpoints the switch task's refetch hits.
fn profile_routes(active: &'static str) -> Router {
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
            get(move || async move {
                (
                    [(header::CONTENT_TYPE, "application/json")],
                    format!(r#"{{"profile":"{active}"}}"#),
                )
            }),
        )
        .route("/v1/models", get(mock_models))
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
async fn a_switch_profile_event_streams_progress_and_settles_the_menu() {
    let base_url = spawn_gateway(
        Router::new()
            .route("/admin/switch-profile", post(mock_switch_succeeds))
            .merge(profile_routes("beta")),
    )
    .await;
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

    let progress: Vec<(String, u64, u64)> = frames
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
        .collect();
    assert_eq!(
        progress,
        [
            ("Loading profile...".to_string(), 1, 3),
            ("Stopping models...".to_string(), 2, 3),
            ("Starting models...".to_string(), 3, 3),
        ],
        "each stage arrives as determinate progress, in execution order"
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
async fn a_failed_switch_restores_the_menu_and_reports_the_failure() {
    let base_url = spawn_gateway(
        Router::new()
            .route("/admin/switch-profile", post(mock_switch_fails))
            // The gateway still serves the previous profile after the
            // failure, so its status endpoint keeps naming it.
            .merge(profile_routes("main")),
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
        "the selection survives the failed switch"
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
        failure["description"], "start-local failed",
        "the gateway's own message is reported"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_switch_while_one_runs_is_refused_with_an_error_frame() {
    let (url, _state_dir, state) = spawn_session_server("http://127.0.0.1:1").await;
    state
        .menu()
        .begin_switch("running")
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
    socket.close(None).await.expect("close the socket");
}
