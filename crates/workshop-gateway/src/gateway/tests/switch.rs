//! Switch-profile tests: a named selection and the no-profile selection
//! each reach the wire as the gateway expects, the JSON answer decodes
//! with an optional profile, a declined switch is buffered rather than
//! an error, and an undecodable success is a malformed answer.

use super::*;

use std::sync::{Arc, Mutex};

/// A mock switch route that records every request body it receives and
/// answers `body`.
fn recording_switch_route(
    answer: &'static str,
) -> (axum::Router, Arc<Mutex<Vec<serde_json::Value>>>) {
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    let app = axum::Router::new().route(
        "/admin/switch-profile",
        axum::routing::post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            let sink = Arc::clone(&sink);
            async move {
                sink.lock()
                    .expect("the recorder lock is healthy")
                    .push(body);
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    answer,
                )
            }
        }),
    );
    (app, received)
}

#[tokio::test]
async fn a_named_selection_posts_the_name_and_decodes_the_outcome() {
    let (app, received) = recording_switch_route(r#"{"profile":"beta","restart_required":true}"#);
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .switch_profile(Some("beta"))
        .await
        .expect("the request completes");
    let SwitchResponse::Selected(outcome) = response else {
        panic!("an accepted selection decodes, got {response:?}");
    };
    assert_eq!(
        outcome,
        SwitchOutcome {
            profile: Some("beta".to_string()),
            restart_required: true,
        }
    );
    assert_eq!(
        received
            .lock()
            .expect("the recorder lock is healthy")
            .as_slice(),
        [serde_json::json!({"name": "beta"})],
        "the selection reaches the gateway as its name"
    );
}

#[tokio::test]
async fn the_no_profile_selection_posts_null_and_decodes_a_null_profile() {
    let (app, received) = recording_switch_route(r#"{"profile":null,"restart_required":false}"#);
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .switch_profile(None)
        .await
        .expect("the request completes");
    let SwitchResponse::Selected(outcome) = response else {
        panic!("an accepted selection decodes, got {response:?}");
    };
    assert_eq!(
        outcome,
        SwitchOutcome {
            profile: None,
            restart_required: false,
        }
    );
    assert_eq!(
        received
            .lock()
            .expect("the recorder lock is healthy")
            .as_slice(),
        [serde_json::json!({"name": null})],
        "no profile reaches the gateway as an explicit null"
    );
}

#[tokio::test]
async fn an_answer_without_a_profile_key_still_decodes() {
    let (app, _received) = recording_switch_route(r#"{"restart_required":true}"#);
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .switch_profile(Some("beta"))
        .await
        .expect("the request completes");
    let SwitchResponse::Selected(outcome) = response else {
        panic!("an accepted selection decodes, got {response:?}");
    };
    assert_eq!(outcome.profile, None, "an absent profile reads as none");
    assert!(outcome.restart_required);
}

#[tokio::test]
async fn a_declined_switch_is_buffered_not_an_error() {
    let app = axum::Router::new().route(
        "/admin/switch-profile",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": {"message": "bad name", "code": "switch_failed"}
                })),
            )
        }),
    );
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .switch_profile(Some("../escape"))
        .await
        .expect("a declined request still completes");
    let SwitchResponse::Buffered(answer) = response else {
        panic!("a declined switch is buffered, got {response:?}");
    };
    assert_eq!(answer.status, reqwest::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn an_undecodable_success_is_a_malformed_answer() {
    let (app, _received) = recording_switch_route("data: {\"stage\":\"loading-profile\"}\n\n");
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let error = client
        .switch_profile(Some("beta"))
        .await
        .expect_err("a success body that is not the outcome JSON is refused");
    assert!(
        matches!(error, GatewayError::Malformed { .. }),
        "expected Malformed, got {error:?}"
    );
}
