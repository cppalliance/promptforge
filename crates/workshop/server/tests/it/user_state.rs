//! The user-state bucket end to end: `/user/state` is mounted through the
//! registry, a fresh state directory answers all null and gains no file,
//! and a value put into one server is served by a second server booted
//! over the same state directory - the relaunch the bucket exists for.

use axum::Router;
use axum::http::StatusCode;

use crate::common::{TestServer, spawn_gateway};

/// The state file the user bucket persists to.
const USER_STATE_FILE: &str = "ui-state.json";

/// `GET /user/state` on `server`, parsed.
// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// their helpers; failing the test by panicking with the invariant named
// is exactly what this is for.
#[expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]
async fn get_state(client: &reqwest::Client, server: &TestServer) -> serde_json::Value {
    let response = client
        .get(server.http_url("/user/state"))
        .send()
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.expect("the body is JSON")
}

#[tokio::test]
async fn a_fresh_state_dir_answers_all_null_and_gains_no_file() {
    let gateway = spawn_gateway(Router::new()).await;
    let server = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();

    let state = get_state(&client, &server).await;

    assert_eq!(
        state,
        serde_json::json!({
            "editor_settings": null,
            "zoom": null,
            "recent_files": null,
            "commands_history": null,
        }),
        "every allow-listed key is present and null on a fresh state directory"
    );
    assert!(
        !server.state_dir().join(USER_STATE_FILE).exists(),
        "boot and a read create no state file"
    );
}

#[tokio::test]
async fn a_put_survives_a_relaunch_over_the_same_state_dir() {
    let gateway = spawn_gateway(Router::new()).await;
    let first = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();

    let response = client
        .put(first.http_url("/user/state/zoom"))
        .header("content-type", "application/json")
        .body("1.5")
        .send()
        .await
        .expect("the route answers");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .json::<serde_json::Value>()
            .await
            .expect("the body is JSON"),
        serde_json::json!({ "saved": true })
    );
    assert_eq!(
        get_state(&client, &first).await["zoom"],
        serde_json::json!(1.5)
    );

    let state_dir = first.shutdown_keeping_state_dir();
    let second = TestServer::spawn_in(&gateway, state_dir);

    let state = get_state(&client, &second).await;
    assert_eq!(
        state["zoom"],
        serde_json::json!(1.5),
        "the second launch reads what the first one stored"
    );
    assert_eq!(
        state["editor_settings"],
        serde_json::Value::Null,
        "a key never put stays null across the relaunch"
    );
}

#[tokio::test]
async fn a_refused_put_answers_the_envelope_through_the_full_router() {
    let gateway = spawn_gateway(Router::new()).await;
    let server = TestServer::spawn(&gateway);
    let client = reqwest::Client::new();

    let response = client
        .put(server.http_url("/user/state/layout"))
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("the route answers");

    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let json: serde_json::Value = response.json().await.expect("the body is JSON");
    assert_eq!(
        json["error"]["code"], "user_state_key",
        "the crate's own envelope reaches the wire through the shell's router"
    );
}
