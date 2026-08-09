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
use promptforge_core::model::CompletionOptions;
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
key = "test-token"

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
    let state = AppState::new(routing, config.server.key);
    spawn(build_router(state)).await
}

#[tokio::test]
async fn happy_path_through_the_real_client() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let client = GatewayClient::new(&format!("http://{gateway}/v1"), "test-token");
    let options = CompletionOptions {
        model: "test-model".into(),
        temperature: None,
        max_tokens: None,
        thinking: None,
        tool_dialect: promptforge_core::dialects::ToolDialectId::OpenAi,
    };
    let result = client
        .complete(
            &[promptforge_core::client::Message::user("ping")],
            None,
            &options,
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

/// A fake Brave Search backend returning five hits on two hosts (with optional `extra_snippets`).
async fn fake_brave() -> SocketAddr {
    async fn search() -> Json<Value> {
        // Five hits on two hosts so diversity (`max_per_host` default 2) is exercised.
        Json(serde_json::json!({
            "web": {
                "results": [
                    {
                        "title": "A1",
                        "url": "https://a.com/1",
                        "description": "first a",
                        "age": "1 day ago",
                        "extra_snippets": ["snippet a1"]
                    },
                    {
                        "title": "A2",
                        "url": "https://a.com/2",
                        "description": "second a",
                        "extra_snippets": ["snippet a2"]
                    },
                    {
                        "title": "A3",
                        "url": "https://a.com/3",
                        "description": "third a"
                    },
                    {
                        "title": "B1",
                        "url": "https://b.com/1",
                        "description": "first b",
                        "extra_snippets": ["snippet b1"]
                    },
                    {
                        "title": "B2",
                        "url": "https://b.com/2",
                        "description": "second b"
                    }
                ]
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
key = "test-token"

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
    let state = AppState::from_parts(
        routing,
        config.server.key,
        promptforge_gateway::local::LocalRuntime::empty(),
        Some(&web_search),
        None,
        None,
    );
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
    assert_eq!(body.get("query").and_then(Value::as_str), Some("hi"));
    let results = body.get("results").and_then(Value::as_array).unwrap();
    // Default max_per_host=2: keep A1,A2,B1,B2; drop A3.
    assert_eq!(results.len(), 4);
    assert_eq!(results[0].get("title").and_then(Value::as_str), Some("A1"));
    assert_eq!(
        results[0].get("url").and_then(Value::as_str),
        Some("https://a.com/1")
    );
    assert_eq!(
        results[0].get("site_name").and_then(Value::as_str),
        Some("a.com")
    );
    assert_eq!(
        results[0]
            .get("extra_snippets")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(1)
    );

    let mut host_counts = std::collections::HashMap::<String, usize>::new();
    for hit in results {
        let site = hit
            .get("site_name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        *host_counts.entry(site).or_default() += 1;
    }
    for count in host_counts.values() {
        assert!(*count <= 2, "default max_per_host is 2");
    }
}

#[tokio::test]
async fn web_search_empty_query_is_400() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{gateway}/v1/tools/web_search"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "query": "   " }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
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
    assert_eq!(
        data[0].get("tool_dialect").and_then(Value::as_str),
        Some("openai")
    );
    assert_eq!(
        data[0].get("tools_mode").and_then(Value::as_str),
        Some("native")
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
key = "test-token"

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
    let state = AppState::new(routing, config.server.key);
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
async fn concurrency_two_admits_two_in_flight_at_backend() {
    // With concurrency=2, two held requests must both reach the backend.
    let release = Arc::new(Notify::new());
    let in_flight = Arc::new(AtomicUsize::new(0));
    let backend = slow_fake_backend(Arc::clone(&release), Arc::clone(&in_flight)).await;
    let gateway = gateway_with_queue(backend, 2, 10).await;

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

    for _ in 0..50 {
        if in_flight.load(Ordering::SeqCst) == 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(
        in_flight.load(Ordering::SeqCst),
        2,
        "concurrency=2 must admit two requests to the backend at once"
    );

    release.notify_waiters();
    assert_eq!(first.await.unwrap().unwrap().status().as_u16(), 200);
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

/// Live local-inference smoke test. Downloads the pinned llama-server binary and
/// the tiny Qwen3-0.6B GGUF, starts a gateway with `[[local_model]]`, and checks
/// that chat completions return text. Ignored by default so CI stays fast.
#[tokio::test]
#[ignore = "downloads llama-server + Qwen3-0.6B; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn local_model_chat_completion_returns_text() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
        // `cargo test -- --ignored` alone should still be an explicit choice;
        // require the env var so a broad --ignored run does not surprise.
        eprintln!("skipping: set PROMPTFORGE_LIVE_LOCAL=1 to run this test");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[local]
cache_dir = "{cache}"

[[local_model]]
name = "qwen-tiny"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "{source}"
sha256 = "{sha}"
context = 4096
thinking = "never"
gpu_layers = 0
flash_attention = false
n_predict = 64
"#,
        cache = cache.path().display().to_string().replace('\\', "/"),
        source = promptforge_gateway::local::SCENARIO_MODEL_URL,
        sha = promptforge_gateway::local::SCENARIO_MODEL_SHA256,
    );

    let config = Config::from_toml_str(&toml).unwrap();
    let local = tokio::task::spawn_blocking({
        let config = Config::from_toml_str(&toml).unwrap();
        move || promptforge_gateway::local::LocalRuntime::start(&config)
    })
    .await
    .unwrap()
    .expect("start local runtime");

    let description = local.models()[0].description.clone();
    assert!(description.contains("careful analysis"));

    let routing = Arc::new(
        Routing::from_config(&config)
            .unwrap()
            .merge(local.models().iter().cloned())
            .unwrap(),
    );
    let token = promptforge_gateway::config::Secret::from("test-token".to_owned());
    let state = AppState::new(routing, token);
    let gateway = spawn(build_router(state)).await;

    let catalog = reqwest::Client::new()
        .get(format!("http://{gateway}/v1/models"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids: Vec<&str> = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    assert!(ids.contains(&"qwen-tiny"));
    assert_eq!(
        catalog["data"][0]["description"].as_str(),
        Some(description.as_str())
    );

    let client = GatewayClient::new(&format!("http://{gateway}/v1"), "test-token");
    let options = CompletionOptions {
        model: "qwen-tiny".into(),
        temperature: None,
        max_tokens: None,
        thinking: None,
        tool_dialect: promptforge_core::dialects::ToolDialectId::OpenAi,
    };
    let result = client
        .complete(
            &[promptforge_core::client::Message::user(
                "Reply with exactly the word pong and nothing else.",
            )],
            None,
            &options,
        )
        .await
        .unwrap();
    match result.result {
        promptforge_core::client::CompletionResult::Text(text) => {
            assert!(!text.trim().is_empty(), "local model returned empty text");
        }
        other => panic!("expected text reply, got {other:?}"),
    }
    drop(local);
}

/// Switch-profile rebuilds the catalog from a remote-only profile (no llama spawn).
#[tokio::test]
#[expect(
    clippy::too_many_lines,
    reason = "end-to-end admin flow is clearer inline"
)]
async fn switch_profile_updates_models_catalog() {
    use std::fs;

    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();

    let alpha = format!(
        r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "alpha-model"
description = "alpha catalog entry"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let beta = format!(
        r#"
[server]
bind = "127.0.0.1:0"
key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

[[model]]
name = "beta-model"
description = "beta catalog entry"
context = 4096
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    fs::write(profiles.path().join("alpha.toml"), alpha).unwrap();
    fs::write(profiles.path().join("beta.toml"), beta).unwrap();

    let config = promptforge_gateway::profile::load_named(profiles.path(), "alpha").unwrap();
    let routing = Arc::new(Routing::from_config(&config).unwrap());
    let state = AppState::from_parts(
        routing,
        config.server.key,
        promptforge_gateway::local::LocalRuntime::empty(),
        None,
        Some(profiles.path().to_path_buf()),
        Some("alpha".to_owned()),
    );
    let gateway = spawn(build_router(state)).await;
    let http = reqwest::Client::new();

    let catalog = http
        .get(format!("http://{gateway}/v1/models"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids: Vec<&str> = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, vec!["alpha-model"]);

    let listed = http
        .get(format!("http://{gateway}/admin/profiles"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(listed["profiles"], serde_json::json!(["alpha", "beta"]));

    let switched = http
        .post(format!("http://{gateway}/admin/switch-profile"))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "beta" }))
        .send()
        .await
        .unwrap();
    assert_eq!(switched.status().as_u16(), 200);

    let catalog = http
        .get(format!("http://{gateway}/v1/models"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    let ids: Vec<&str> = catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|m| m.get("id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, vec!["beta-model"]);

    let status = http
        .get(format!("http://{gateway}/admin/status"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(status["profile"], "beta");
    assert_eq!(status["models"], serde_json::json!(["beta-model"]));
}
