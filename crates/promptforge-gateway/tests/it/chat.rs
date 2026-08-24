//! Chat-completions, models-catalog, and health routes through the real client.

use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::model::CompletionOptions;
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use crate::support::{
    PHASE_TIMEOUT, TestServer, canned_reply, chat_body, fake_backend, gateway_for, json_within,
    recording_backend, send_within,
};

#[tokio::test]
async fn happy_path_through_the_real_client() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{}/v1", gateway.addr)).expect("valid test endpoint"),
        SecretString::new("test-token").expect("non-empty test key"),
    );
    let options = CompletionOptions::new(
        "test-model",
        promptforge_core::dialects::ToolDialectId::OpenAi,
    );
    let result = tokio::time::timeout(
        PHASE_TIMEOUT,
        client.complete(
            &[promptforge_core::client::Message::user("ping")],
            None,
            &options,
        ),
    )
    .await
    .expect("client completion exceeded the phase timeout")
    .unwrap();
    match result.result() {
        promptforge_core::client::CompletionResult::Text(reply) => assert_eq!(reply, "pong"),
        other => panic!("expected text reply, got {other:?}"),
    }
    gateway.shutdown().await;
}

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

#[tokio::test]
async fn models_catalog_returns_configured_entries() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    assert_eq!(body.get("object").and_then(Value::as_str), Some("list"));
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("test-model")
    );
    assert_eq!(data[0].get("object").and_then(Value::as_str), Some("model"));
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[0].get("description").and_then(Value::as_str),
        Some("a test model for integration")
    );
    assert_eq!(data[0].get("context").and_then(Value::as_u64), Some(8192));
    assert_eq!(
        data[0].get("thinking").and_then(Value::as_str),
        Some("never")
    );
    assert_eq!(
        data[0].get("tool_dialect").and_then(Value::as_str),
        Some("openai")
    );
    assert_eq!(
        data[0].get("tools_mode").and_then(Value::as_str),
        Some("native")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_carries_model_kinds() {
    let backend = fake_backend().await;
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);

    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 2);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("chat-model")
    );
    assert_eq!(data[0].get("kind").and_then(Value::as_str), Some("chat"));
    assert_eq!(
        data[1].get("id").and_then(Value::as_str),
        Some("embed-model")
    );
    assert_eq!(
        data[1].get("kind").and_then(Value::as_str),
        Some("embedding")
    );
    gateway.shutdown().await;
}

#[tokio::test]
async fn models_catalog_wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("wrong-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
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
