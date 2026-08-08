//! End-to-end tests: a fake OpenAI backend behind the real gateway, driven by
//! the executor's real [`GatewayClient`]. This is the test that keeps the two
//! independent definitions of the wire shape honest.
#![expect(
    clippy::unwrap_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use axum::Json;
use axum::Router;
use axum::routing::post;
use promptforge_core::client::GatewayClient;
use promptforge_gateway::config::Config;
use promptforge_gateway::routing::Routing;
use promptforge_gateway::{AppState, build_router};
use serde_json::Value;
use tokio::sync::Notify;

/// Spawn a server on an ephemeral port and return its address.
async fn spawn(router: Router) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    addr
}

/// A fake OpenAI backend that echoes the model it was asked for and returns a
/// canned assistant message.
async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> Json<Value> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" },
                "finish_reason": "stop"
            }]
        }))
    }
    let router = Router::new().route("/chat/completions", post(completions));
    spawn(router).await
}

/// Start the gateway wired to the fake backend and return its address.
async fn gateway_for(backend: SocketAddr) -> SocketAddr {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
token = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
thinking = "never"
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let routing = Arc::new(Routing::from_config(&config).unwrap());
    let state = AppState::new(routing, config.server.token);
    spawn(build_router(state)).await
}

#[tokio::test]
async fn happy_path_through_the_real_client() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let client = GatewayClient::new(&format!("http://{gateway}/v1"), "test-token", "test-model");
    let result = client
        .complete(
            &[promptforge_core::client::Message::user("ping")],
            None,
            None,
        )
        .await
        .unwrap();
    match result.result {
        promptforge_core::client::CompletionResult::Text(reply) => assert_eq!(reply, "pong"),
        other => panic!("expected text reply, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_model_is_404() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "model": "nope", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/chat/completions"))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({ "model": "test-model", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

/// A fake Brave Search backend returning one canned result.
async fn fake_brave() -> SocketAddr {
    async fn search() -> Json<Value> {
        Json(serde_json::json!({
            "web": {
                "results": [{
                    "title": "T",
                    "url": "https://e.com",
                    "description": "D",
                    "age": "2026-01-01"
                }]
            }
        }))
    }
    let router = Router::new().route("/web/search", axum::routing::get(search));
    spawn(router).await
}

/// Start a gateway wired to a fake Brave backend for the web-search tool.
async fn gateway_with_web_search(brave: SocketAddr) -> SocketAddr {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
token = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{brave}"
api_key = ""

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
thinking = "never"
upstream = "backend-model"
endpoints = ["fake"]

[tools.web_search]
provider = "brave"
api_key = "brave-key"
base_url = "http://{brave}"
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let routing = Arc::new(Routing::from_config(&config).unwrap());
    let web_search = config.tools.and_then(|tools| tools.web_search).unwrap();
    let state = AppState::new(routing, config.server.token).with_web_search(&web_search);
    spawn(build_router(state)).await
}

#[tokio::test]
async fn web_search_returns_results() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/tools/web_search"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    let body: Value = response.json().await.unwrap();
    let results = body.get("results").and_then(Value::as_array).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].get("title").and_then(Value::as_str), Some("T"));
    assert_eq!(
        results[0].get("url").and_then(Value::as_str),
        Some("https://e.com")
    );
}

#[tokio::test]
async fn web_search_wrong_token_is_401() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/tools/web_search"))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

#[tokio::test]
async fn web_search_not_configured_is_404() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/tools/web_search"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "hi" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 404);
}

#[tokio::test]
async fn health_needs_no_auth() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/health"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);
}

#[tokio::test]
async fn models_catalog_returns_configured_entries() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/v1/models"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 200);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body.get("object").and_then(Value::as_str), Some("list"));
    let data = body.get("data").and_then(Value::as_array).unwrap();
    assert_eq!(data.len(), 1);
    assert_eq!(
        data[0].get("id").and_then(Value::as_str),
        Some("test-model")
    );
    assert_eq!(data[0].get("object").and_then(Value::as_str), Some("model"));
    assert_eq!(
        data[0].get("description").and_then(Value::as_str),
        Some("a test model for integration")
    );
    assert_eq!(data[0].get("context").and_then(Value::as_u64), Some(8192));
    assert_eq!(
        data[0].get("thinking").and_then(Value::as_str),
        Some("never")
    );
}

#[tokio::test]
async fn models_catalog_wrong_token_is_401() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .get(format!("http://{gateway}/v1/models"))
        .bearer_auth("wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

/// A fake backend that holds each request until `release` is notified.
///
/// `in_flight` counts how many requests are currently inside the handler
/// (after arrival, before release).
async fn slow_fake_backend(release: Arc<Notify>, in_flight: Arc<AtomicUsize>) -> SocketAddr {
    async fn completions(
        axum::extract::State((release, in_flight)): axum::extract::State<(
            Arc<Notify>,
            Arc<AtomicUsize>,
        )>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        in_flight.fetch_add(1, Ordering::SeqCst);
        release.notified().await;
        in_flight.fetch_sub(1, Ordering::SeqCst);
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Json(serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion",
            "model": model,
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "pong" },
                "finish_reason": "stop"
            }]
        }))
    }
    let router = Router::new()
        .route("/chat/completions", post(completions))
        .with_state((release, in_flight));
    spawn(router).await
}

/// Gateway with an explicit concurrency / queue configuration.
async fn gateway_with_queue(
    backend: SocketAddr,
    concurrency: usize,
    max_depth: usize,
) -> SocketAddr {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
token = "test-token"

[queue]
max_depth = {max_depth}
fair_scheduling = true

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""
concurrency = {concurrency}

[[model]]
name = "test-model"
description = "a test model for integration"
context = 8192
thinking = "never"
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let routing = Arc::new(Routing::from_config(&config).unwrap());
    let state = AppState::new(routing, config.server.token);
    spawn(build_router(state)).await
}

fn chat_body() -> Value {
    serde_json::json!({
        "model": "test-model",
        "messages": [{ "role": "user", "content": "ping" }]
    })
}

#[tokio::test]
async fn concurrency_one_allows_only_one_in_flight_at_backend() {
    // max_depth is waiting slots only; with concurrency=1, a second request
    // waits while the first is in-flight, and only one reaches the backend.
    let release = Arc::new(Notify::new());
    let in_flight = Arc::new(AtomicUsize::new(0));
    let backend = slow_fake_backend(Arc::clone(&release), Arc::clone(&in_flight)).await;
    let gateway = gateway_with_queue(backend, 1, 10).await;

    let client = reqwest::Client::new();
    let url = format!("http://{gateway}/v1/chat/completions");

    let first = tokio::spawn({
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&chat_body())
                .send()
                .await
        }
    });
    let second = tokio::spawn({
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&chat_body())
                .send()
                .await
        }
    });

    // Wait until the first request is inside the slow backend.
    for _ in 0..50 {
        if in_flight.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        1,
        "only one request should reach the backend under concurrency=1"
    );
    // Second request should be queued at the gateway, not in the backend.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        1,
        "second request must not enter the backend while the first holds the slot"
    );

    // Release the held backend request; the waiter should then enter.
    release.notify_one();
    for _ in 0..50 {
        // After the first leaves, the second should become the sole in-flight.
        if in_flight.load(Ordering::SeqCst) == 1 {
            // Distinguish "first still held" from "second entered" by waiting
            // for first to finish, then checking again.
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let first_status = first.await.unwrap().unwrap().status().as_u16();
    assert_eq!(first_status, 200);
    for _ in 0..50 {
        if in_flight.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        1,
        "second request should enter after the first is released"
    );

    release.notify_one();
    assert_eq!(second.await.unwrap().unwrap().status().as_u16(), 200);
}

#[tokio::test]
async fn queue_full_returns_503_when_waiting_slots_exhausted() {
    // concurrency=1, max_depth=1: one in-flight + one waiting; a third gets 503.
    // max_depth counts waiting requests only, not in-flight.
    let release = Arc::new(Notify::new());
    let in_flight = Arc::new(AtomicUsize::new(0));
    let backend = slow_fake_backend(Arc::clone(&release), Arc::clone(&in_flight)).await;
    let gateway = gateway_with_queue(backend, 1, 1).await;

    let client = reqwest::Client::new();
    let url = format!("http://{gateway}/v1/chat/completions");

    let first_handle = tokio::spawn({
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&chat_body())
                .send()
                .await
        }
    });

    // Wait until the first request is inside the slow backend.
    for _ in 0..50 {
        if in_flight.load(Ordering::SeqCst) == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(in_flight.load(Ordering::SeqCst), 1);

    let second = tokio::spawn({
        let client = client.clone();
        let url = url.clone();
        async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&chat_body())
                .send()
                .await
        }
    });
    // Let the second request enter the waiting queue.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let third = client
        .post(&url)
        .bearer_auth("test-token")
        .json(&chat_body())
        .send()
        .await
        .unwrap();
    assert_eq!(third.status().as_u16(), 503);
    let body: Value = third.json().await.unwrap();
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_full")
    );

    // Drain: wake the in-flight request, then the waiter once it reaches the backend.
    release.notify_one();
    assert_eq!(first_handle.await.unwrap().unwrap().status().as_u16(), 200);
    release.notify_one();
    assert_eq!(second.await.unwrap().unwrap().status().as_u16(), 200);
}
