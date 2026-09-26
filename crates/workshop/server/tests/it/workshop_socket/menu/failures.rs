//! Profile selections that fail before the gateway answers with the
//! outcome document: a selection lost in transit, and a success answer
//! this build cannot read. The gateway may still serve after either, so
//! the menu refetches its state before it settles.

use axum::Router;
use axum::routing::post;

use crate::common::spawn_gateway;

use super::super::spawn_session_server;
use super::{failed_switch_frames, profile_routes};

/// Runs a switch from `main` to `beta` against the gateway at
/// `base_url` and returns the failure status's description and the
/// settled workbench snapshot.
async fn failed_switch(base_url: &str) -> (String, serde_json::Value) {
    let (url, _state_dir, state) = spawn_session_server(base_url).await;
    state.menu().set_gateway_reachable(true);
    state.menu().set_profiles(
        vec!["main".to_string(), "beta".to_string()],
        Some("main".to_string()),
    );
    let (failure, restored) = failed_switch_frames(&url).await;
    let description = failure["description"]
        .as_str()
        .expect("the failure has a description")
        .to_owned();
    (description, restored)
}

#[tokio::test]
async fn a_selection_lost_in_transit_fails_with_the_transport_error() {
    // Nothing listens on the discard port, so the selection never lands
    // and the refetch finds no profile to restore.
    let (description, restored) = failed_switch("http://127.0.0.1:1").await;
    assert_eq!(description, "gateway transport error");
    assert_eq!(
        restored["profiles"],
        serde_json::json!([]),
        "the menu shows what the refetch found, not a stale list: {restored}"
    );
}

#[tokio::test]
async fn an_unrecognized_success_answer_fails_the_switch() {
    let base_url = spawn_gateway(
        Router::new()
            .route(
                "/admin/switch-profile",
                post(|| async { "switched, probably" }),
            )
            .merge(profile_routes(Some("main"))),
    )
    .await;
    let (description, restored) = failed_switch(&base_url).await;
    assert_eq!(
        description,
        "malformed gateway answer: the switch-profile answer is not the outcome document"
    );
    assert_eq!(
        restored["active"], "main",
        "the menu keeps the profile the gateway still serves: {restored}"
    );
}
