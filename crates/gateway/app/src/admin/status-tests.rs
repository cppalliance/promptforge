//! The status surface: endpoint readiness entries and the queue readout.
//! The status surfaces over the command queue: the `GET
//! /admin/status` queue and endpoint readouts, and the queue-cancel
//! routes firing the active command's token and dropping pending
//! entries, exercised against a running fixture gateway with a
//! parked command.

use axum::body::Body;
use axum::http::header::AUTHORIZATION;
use axum::http::{Request, StatusCode};
use gateway_config::{Config, ProfileName};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use crate::commands::Command;
use crate::error::GatewayError;
use crate::models::{EndpointStatus, endpoint_status};
use crate::test_support::{app_state, parking_executor, serve_state, wait_until};
use crate::{AppState, build_router};

/// A state whose catalog declares one local chat model the routing
/// table never holds: `app_state` routes only the remote catalog, so
/// `slow-model` stays configured-but-unloaded for the test's run.
/// Strict bearer auth (`trust_loopback = false`): the cancel-route
/// test pins that a missing key is refused from the loopback listener.
fn state() -> AppState {
    let config = Config::from_toml_str(
        "config-version = 0\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         trust_loopback = false\n\
         [[local_model]]\nname = \"slow-model\"\ndescription = \"d\"\n\
         source = \"/models/slow.gguf\"\ncontext = 4096\n\
         [[profile]]\nname = \"main\"\nmodels = [\"slow-model\"]\n",
    )
    .expect("config parses");
    app_state(config, None)
}

async fn get_status(state: AppState) -> serde_json::Value {
    let response = build_router(state, None)
        .oneshot(
            Request::builder()
                .uri("/admin/status")
                .header(AUTHORIZATION, "Bearer test-token")
                .body(Body::empty())
                .expect("static request parts are valid"),
        )
        .await
        .expect("the router is infallible");
    assert_eq!(response.status(), StatusCode::OK);
    serde_json::from_slice(
        &axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("body reads"),
    )
    .expect("the status body is JSON")
}

#[test]
fn the_endpoint_readiness_mapping() {
    let ready = endpoint_status("/v1/chat/completions", "Chat completions", true, true, true);
    assert_eq!(
        ready,
        EndpointStatus {
            path: "/v1/chat/completions",
            name: "Chat completions",
            ready: true,
            provisioning: false,
        },
        "a served endpoint is never provisioning, even mid-command"
    );
    assert!(
        endpoint_status("/v1/embeddings", "Embeddings", true, false, true).provisioning,
        "configured, unserved, and a command running: provisioning"
    );
    assert!(
        !endpoint_status("/v1/embeddings", "Embeddings", true, false, false).provisioning,
        "configured and unserved with an idle queue reads as not ready, not provisioning"
    );
    assert!(
        !endpoint_status("/v1/rerank", "Rerank", false, false, true).provisioning,
        "an unconfigured endpoint is never provisioning"
    );
}

#[tokio::test]
async fn the_status_response_carries_the_queue_and_endpoint_shape() {
    let body = get_status(state()).await;
    assert_eq!(body["queue"]["active"], serde_json::Value::Null);
    assert_eq!(body["queue"]["pending"], serde_json::json!([]));
    assert_eq!(
        body["progress"],
        serde_json::json!({ "busy": false, "text": "" }),
        "an idle gateway carries the idle Progress snapshot at the top level: {body}"
    );
    assert_eq!(
        body["loading_models"],
        serde_json::json!([]),
        "with no switch running, nothing is loading: {body}"
    );
    assert!(
        body["vram_gb"].is_number(),
        "the declared VRAM total is always present: {body}"
    );
    let endpoints = body["endpoints"].as_array().expect("endpoints is an array");
    let chat = endpoints
        .iter()
        .find(|entry| entry["path"] == "/v1/chat/completions")
        .expect("the chat completions endpoint is listed");
    assert_eq!(chat["name"], "Chat completions");
    assert_eq!(
        chat["ready"], false,
        "the configured local model is not loaded"
    );
    assert_eq!(
        chat["provisioning"], false,
        "no command is running, so nothing provisions"
    );
    for path in ["/v1/embeddings", "/v1/rerank", "/v1/audio/speech"] {
        let entry = endpoints
            .iter()
            .find(|entry| entry["path"] == path)
            .unwrap_or_else(|| panic!("the {path} endpoint is listed"));
        assert_eq!(entry["ready"], false, "{path} has no configured model");
        assert_eq!(entry["provisioning"], false);
    }
    #[cfg(feature = "stt")]
    assert!(
        endpoints
            .iter()
            .any(|entry| entry["path"] == "/v1/audio/transcriptions"),
        "stt builds list the transcriptions endpoint"
    );
}

#[tokio::test]
async fn the_status_response_reports_the_active_and_pending_commands() {
    let state = state();
    let queue = state.commands.clone();
    let worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    let active = queue.enqueue(Command::load_profile(
        ProfileName::parse("main").expect("profile name"),
        CancellationToken::new(),
    ));
    let pending = queue.enqueue(Command::ProvisionModel {
        name: "extra".to_owned(),
        source: "/models/extra.gguf".to_owned(),
        token: CancellationToken::new(),
    });
    wait_until("the boot command to go active", || {
        queue.active_command().is_some()
    })
    .await;

    let body = get_status(state.clone()).await;
    assert_eq!(
        body["queue"]["active"]["name"], "load-profile: main",
        "the active command is named: {body}"
    );
    assert!(
        body["queue"]["active"].get("fraction").is_none(),
        "the active command carries no fraction: {body}"
    );
    assert!(
        body["queue"]["active"]["started_at"].is_u64(),
        "the active command carries its start time as epoch seconds: {body}"
    );
    assert_eq!(
        body["progress"],
        serde_json::json!({ "busy": true, "text": "load-profile: main" }),
        "the running command's activity is the top-level progress object: {body}"
    );
    let pending_entries = body["queue"]["pending"]
        .as_array()
        .expect("pending is an array");
    assert_eq!(pending_entries.len(), 1);
    assert_eq!(pending_entries[0]["name"], "provision-model: extra");
    assert!(pending_entries[0]["queued_at"].is_u64());
    let chat = body["endpoints"]
        .as_array()
        .expect("endpoints is an array")
        .iter()
        .find(|entry| entry["path"] == "/v1/chat/completions")
        .expect("the chat completions endpoint is listed")
        .clone();
    assert_eq!(
        chat["provisioning"], true,
        "a configured, unloaded chat model under a running command is provisioning"
    );

    queue.cancel_active();
    drop((active, pending));
    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");
}

#[tokio::test]
async fn the_cancel_routes_fire_the_token_and_drop_pending_entries() {
    let state = state();
    let queue = state.commands.clone();
    let worker = state
        .commands
        .spawn_worker_with(&state, parking_executor())
        .expect("worker spawns");
    let addr = serve_state(state).await;
    let client = reqwest::Client::new();

    let active = queue.enqueue(Command::load_profile(
        ProfileName::parse("main").expect("profile name"),
        CancellationToken::new(),
    ));
    let pending = queue.enqueue(Command::ProvisionModel {
        name: "extra".to_owned(),
        source: "/models/extra.gguf".to_owned(),
        token: CancellationToken::new(),
    });
    wait_until("the boot command to go active", || {
        queue.active_command().is_some()
    })
    .await;

    // The routes refuse an unauthenticated caller before touching the
    // queue.
    let response = client
        .post(format!("http://{addr}/admin/queue/cancel"))
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::UNAUTHORIZED);

    // Cancelling a pending entry settles its waiter as cancelled and
    // leaves the active command running.
    let response = client
        .post(format!("http://{addr}/admin/queue/cancel-pending"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "index": 0 }))
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(body["cancelled"], true, "the pending entry was removed");
    let outcome = pending.outcome.await.expect("the pending waiter settles");
    assert!(
        matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
        "the cancelled pending command settles as cancelled: {outcome:?}"
    );
    assert!(
        queue.active_command().is_some(),
        "the active command still runs"
    );

    // Cancelling the active command fires its token; the parked body
    // observes it and settles as cancelled.
    let response = client
        .post(format!("http://{addr}/admin/queue/cancel"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the request sends");
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(body["cancelled"], true, "a command was active to cancel");
    let outcome = active.outcome.await.expect("the active waiter settles");
    assert!(
        matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
        "the parked command observed its token: {outcome:?}"
    );

    // With the queue idle, both routes report nothing to cancel.
    wait_until("the queue to go idle", || queue.active_command().is_none()).await;
    let response = client
        .post(format!("http://{addr}/admin/queue/cancel"))
        .bearer_auth("test-token")
        .send()
        .await
        .expect("the request sends");
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(body["cancelled"], false, "no active command to cancel");
    let response = client
        .post(format!("http://{addr}/admin/queue/cancel-pending"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "index": 0 }))
        .send()
        .await
        .expect("the request sends");
    let body: serde_json::Value = response.json().await.expect("the reply is JSON");
    assert_eq!(body["cancelled"], false, "no pending entry at the index");

    queue.shutdown();
    worker.await.expect("the worker exits on shutdown");
}
