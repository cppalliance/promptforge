//! Chat-completions, models-catalog, and health route behavior.

use axum::routing::post;
use axum::{Json, Router};
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use crate::support::{
    PHASE_TIMEOUT, TestServer, canned_reply, chat_body, fake_backend, gateway_for, json_within,
    recording_backend, send_within, spawn_backend,
};

mod catalog;
mod gemma;
mod streaming;

/// IT-005/006: the fake backend records the request, so we can assert exactly
/// what the gateway forwarded: method, path, the rewritten upstream model, the
/// intact messages, and that the client's bearer is not leaked upstream.
#[tokio::test]
async fn forwards_method_path_model_and_messages_to_backend() {
    let (backend, recorder) = recording_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/chat/completions");
    // The public model name is rewritten to the endpoint's upstream alias.
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-model")
    );
    let messages = request
        .body
        .get("messages")
        .and_then(Value::as_array)
        .expect("messages forwarded");
    assert_eq!(messages.len(), 1);
    assert_eq!(
        messages[0].get("role").and_then(Value::as_str),
        Some("user")
    );
    assert_eq!(
        messages[0].get("content").and_then(Value::as_str),
        Some("ping")
    );
    // The gateway must not pass the caller's own bearer through to the upstream.
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// A 200 upstream response that fails shape validation is a protocol error
/// (UP-004): the gateway must not fabricate an upstream status that never
/// happened.
#[tokio::test]
async fn invalid_shape_200_is_upstream_protocol_not_upstream_error() {
    async fn completions() -> Json<Value> {
        Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": "backend-model",
            "choices": [{ "index": 0 }]
        }))
    }
    let backend = spawn_backend(Router::new().route("/chat/completions", post(completions))).await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&chat_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 502);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_protocol")
    );
    gateway.shutdown().await;
}

/// A model configured for a non-chat kind is rejected on the chat route with
/// 400 and `kind_mismatch` before any queue admission or upstream call.
#[tokio::test]
async fn non_chat_kinds_are_rejected_on_the_chat_route() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "reranker"
kind = "classifier"
description = "a classifier model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
voices = ["alloy"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    for model in ["embed-model", "reranker", "tts-model"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/chat/completions", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "messages": [{ "role": "user", "content": "hi" }]
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400, "model {model}");
        let body = json_within(response).await;
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("kind_mismatch"),
            "model {model}"
        );
        assert_eq!(
            body.pointer("/error/type").and_then(Value::as_str),
            Some("invalid_request_error"),
            "model {model}"
        );
    }
    gateway.shutdown().await;
}

#[tokio::test]
async fn unknown_model_is_404_with_model_not_found_code() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "nope",
                "messages": [{ "role": "user", "content": "hi" }]
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 404);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("model_not_found")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("invalid_request_error")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn wrong_token_is_401_with_unauthorized_code() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("wrong-token")
            .json(&serde_json::json!({ "model": "test-model", "messages": [] })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("unauthorized")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn health_needs_no_auth() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response =
        send_within(reqwest::Client::new().get(format!("http://{}/health", gateway.addr))).await;
    assert_eq!(response.status().as_u16(), 200);
    gateway.shutdown().await;
}

#[test]
fn canned_reply_shapes_a_chat_completion() {
    let reply = canned_reply("m");
    assert_eq!(
        reply.get("object").and_then(Value::as_str),
        Some("chat.completion")
    );
    assert_eq!(reply.get("model").and_then(Value::as_str), Some("m"));
}

/// Reads a response body to completion, bounded by the phase timeout.
async fn text_within(response: reqwest::Response) -> String {
    tokio::time::timeout(PHASE_TIMEOUT, response.text())
        .await
        .expect("HTTP body read exceeded the phase timeout")
        .expect("HTTP body read failed")
}
