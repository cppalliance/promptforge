//! What the handler does with a run that has to wait, or that outlasts the call
//! which started it.
//!
//! Nothing here waits on the real clock. The deadline and the admission wait are
//! spent on Tokio's paused clock, and the one prompt that has to be slower than
//! its deadline is made slow by a gateway that answers only when the test says
//! so, rather than by a duration anybody has to guess.

use std::sync::Arc;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::routing::post;
use rmcp::model::{CallToolRequestParams, ErrorCode};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::sync::Notify;

use super::{
    Gateway, PromptForgeServer, call, server, server_with, speaking_server_with, structured_of,
    text_of,
};

/// A gateway that answers nothing until `release` is signalled, which is how a
/// prompt is made slower than its deadline without waiting on a clock to make it
/// so.
async fn spawn_gated_gateway(release: Arc<Notify>) -> Gateway {
    let completions = move |Json(_body): Json<Value>| {
        let release = Arc::clone(&release);
        async move {
            release.notified().await;
            Json(json!({
                "choices": [{ "message": { "role": "assistant", "content": "eventually" } }]
            }))
        }
    };

    let router = Router::new().route("/v1/chat/completions", post(completions));
    Gateway::serve(router).await
}

/// A server whose one prose prompt is pointed at a gateway the test releases.
///
/// The returned [`Gateway`] is the test's to keep alive: dropping it stops the
/// serving task, so the caller holds it until the run under test no longer needs
/// the backend.
async fn gated_server(server_lines: &str) -> (TempDir, PromptForgeServer, Arc<Notify>, Gateway) {
    let release = Arc::new(Notify::new());
    let gateway = spawn_gated_gateway(Arc::clone(&release)).await;
    let (dir, server) = speaking_server_with(gateway.addr(), server_lines);
    (dir, server, release, gateway)
}

/// A gateway that reports on `started` when a run reaches it and then blocks for
/// ever, so a run can be observed in flight and never answered.
async fn spawn_reporting_gateway(started: Arc<Notify>) -> Gateway {
    let completions = move |Json(_body): Json<Value>| {
        let started = Arc::clone(&started);
        async move {
            started.notify_one();
            // Never answers: the run stays in flight until it is cancelled.
            std::future::pending::<()>().await;
            Json(json!({ "choices": [] }))
        }
    };

    let router = Router::new().route("/v1/chat/completions", post(completions));
    Gateway::serve(router).await
}

#[tokio::test(start_paused = true)]
async fn an_abandoned_call_cancels_the_in_flight_run_and_frees_its_slot() {
    // The run reaches the gateway and blocks there, so it is provably in flight
    // when its call is dropped well before the reply deadline.
    let started = Arc::new(Notify::new());
    let gateway = spawn_reporting_gateway(Arc::clone(&started)).await;
    let (_dir, server) = speaking_server_with(
        gateway.addr(),
        "max_concurrent_runs = 1\nadmission_timeout = \"50s\"\nreply_deadline = \"300s\"",
    );

    let abandoned = tokio::time::timeout(
        Duration::from_secs(2),
        server.dispatch(call("run_prompt", json!({ "prompt": "speak" }))),
    )
    .await;
    assert!(
        abandoned.is_err(),
        "the call is dropped long before its 300s reply deadline"
    );

    // The run had reached the gateway, so what was dropped was an in-flight run,
    // not one that failed before it started.
    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .expect("the run reached the gateway before its call was abandoned");

    // The old bug detached the run here, leaving it to hold the only slot for
    // the life of the process. Now dropping the call cancelled it, so it ended
    // and returned its slot: a fresh admission is granted rather than refused.
    let readmitted = server.registry.admit().await;
    assert!(
        readmitted.is_some(),
        "a cancelled run frees its slot; a detached leak would refuse this within the admission wait"
    );
}

#[tokio::test]
async fn check_run_collects_a_run_that_finished_inside_its_deadline() {
    let (_dir, server) = server();
    let ran = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": "hello" }),
        ))
        .await
        .expect("running a named prompt is not a protocol error");
    let run_id = structured_of(&ran)["run_id"]
        .as_str()
        .expect("a run carries an identifier")
        .to_owned();

    let collected = server
        .dispatch(call("check_run", json!({ "run_id": run_id })))
        .await
        .expect("collecting is not a protocol error");
    assert_eq!(collected.is_error, Some(false));
    let structured = structured_of(&collected);
    assert_eq!(structured["status"], json!("completed"));
    assert_eq!(structured["value"], json!("hello"));
    assert_eq!(text_of(&collected), "hello");
}

#[tokio::test(start_paused = true)]
async fn a_run_that_outlives_its_deadline_is_reported_running_and_keeps_going() {
    let (_dir, server, release, _gateway) = gated_server("reply_deadline = \"50ms\"").await;

    let result = server
        .dispatch(call("run_prompt", json!({ "prompt": "speak" })))
        .await
        .expect("a deadline is not a protocol error");

    assert_eq!(result.is_error, Some(false), "running is not a failure");
    let structured = structured_of(&result);
    assert_eq!(structured["status"], json!("running"));
    assert!(structured["value"].is_null());
    assert!(
        text_of(&result).contains("check_run"),
        "the caller is told how to collect it"
    );
    let run_id = structured["run_id"]
        .as_str()
        .expect("a running run is named by its id")
        .to_owned();

    // The deadline detached the run rather than cancelling it: the record is
    // still there, and still going, after the call was answered.
    let polled = server
        .dispatch(call("check_run", json!({ "run_id": run_id })))
        .await
        .expect("collecting is not a protocol error");
    assert_eq!(polled.is_error, Some(false));
    assert_eq!(structured_of(&polled)["status"], json!("running"));

    release.notify_one();
}

#[tokio::test]
async fn an_unknown_run_id_is_a_result_naming_the_retention_window() {
    let (_dir, server) = server();
    let result = server
        .dispatch(call("check_run", json!({ "run_id": "0".repeat(32) })))
        .await
        .expect("polling too late is an answer, not a protocol error");

    assert_eq!(result.is_error, Some(true));
    assert!(
        result.structured_content.is_none(),
        "there is no run to report"
    );
    let text = text_of(&result);
    assert!(text.contains("1h"), "the window is named: {text}");
}

#[tokio::test]
async fn a_missing_run_id_is_a_protocol_error() {
    let (_dir, server) = server();
    let Err(missing) = server
        .dispatch(CallToolRequestParams::new("check_run"))
        .await
    else {
        panic!("an absent run_id is the client's bug, not the model's")
    };
    assert_eq!(missing.code, ErrorCode::INVALID_PARAMS);
}

#[tokio::test(start_paused = true)]
async fn a_call_that_cannot_get_a_slot_is_refused_with_the_wait_it_spent() {
    let (_dir, server) = server_with("max_concurrent_runs = 1\nadmission_timeout = \"5s\"");
    let slot = server
        .registry
        .admit()
        .await
        .expect("the only slot starts free");

    let result = server
        .dispatch(call(
            "run_prompt",
            json!({ "prompt": "echo", "args": "hello" }),
        ))
        .await
        .expect("a refusal is an answer, not a protocol error");
    assert_eq!(result.is_error, Some(true));
    assert!(
        result.structured_content.is_none(),
        "nothing ran, so there is no run to report"
    );
    let text = text_of(&result);
    assert!(text.contains("5s"), "the wait is named: {text}");

    drop(slot);
}
