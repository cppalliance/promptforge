//! End-to-end tests: a fake OpenAI backend behind the real gateway, driven by
//! the executor's real [`GatewayClient`]. This is the test that keeps the two
//! independent definitions of the wire shape honest.
//!
//! Determinism: the gateway is served on a caller-owned ephemeral listener
//! (no port race), shutdown is driven by a rendezvous [`TestServer`] fixture,
//! and concurrency tests use an arrivals channel plus per-request release
//! handles instead of sleeps.
#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test helpers panic on setup failure, which is the desired behavior"
)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use promptforge_core::client::{GatewayClient, GatewayEndpoint, SecretString};
use promptforge_core::model::CompletionOptions;
use promptforge_gateway::{Config, Gateway, ProfileName, ProfilesContext};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Pinned tiny Qwen3-0.6B GGUF, used only by the ignored live-local test.
const SCENARIO_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
const SCENARIO_MODEL_SHA256: &str =
    "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031";

/// Per-phase timeout so a hung rendezvous fails fast instead of hanging CI.
const PHASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A gateway served on a caller-owned ephemeral listener, shut down on drop.
struct TestServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: JoinHandle<()>,
}

impl TestServer {
    async fn start(gateway: Gateway) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            let _ = gateway
                .serve(listener, async {
                    let _ = rx.await;
                })
                .await;
        });
        TestServer {
            addr,
            shutdown: Some(shutdown),
            handle,
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        self.handle.abort();
    }
}

/// Spawn a plain axum backend on an ephemeral port and return its address.
async fn spawn_backend(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

/// A fake OpenAI backend that echoes the model and returns a canned reply.
async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> Json<Value> {
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Json(canned_reply(&model))
    }
    spawn_backend(Router::new().route("/chat/completions", post(completions))).await
}

fn canned_reply(model: &str) -> Value {
    serde_json::json!({
        "id": "cmpl-test",
        "object": "chat.completion",
        "model": model,
        "choices": [{
            "index": 0,
            "message": { "role": "assistant", "content": "pong" },
            "finish_reason": "stop"
        }]
    })
}

fn gateway_config(backend: SocketAddr) -> Config {
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
    Config::from_toml_str(&toml).unwrap()
}

/// Start the gateway wired to the fake backend.
async fn gateway_for(backend: SocketAddr) -> TestServer {
    let gateway =
        Gateway::from_config(&gateway_config(backend), ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

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
    let result = client
        .complete(
            &[promptforge_core::client::Message::user("ping")],
            None,
            &options,
        )
        .await
        .unwrap();
    match result.result() {
        promptforge_core::client::CompletionResult::Text(reply) => assert_eq!(reply, "pong"),
        other => panic!("expected text reply, got {other:?}"),
    }
}

#[tokio::test]
async fn unknown_model_is_404() {
    let backend = fake_backend().await;
    let gateway = gateway_for(backend).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/chat/completions", gateway.addr))
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
        .post(format!("http://{}/v1/chat/completions", gateway.addr))
        .bearer_auth("wrong-token")
        .json(&serde_json::json!({ "model": "test-model", "messages": [] }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

/// A fake Brave Search backend returning five hits on two hosts.
async fn fake_brave() -> SocketAddr {
    async fn search() -> Json<Value> {
        Json(serde_json::json!({
            "web": {
                "results": [
                    { "title": "A1", "url": "https://a.com/1", "description": "first a", "age": "1 day ago", "extra_snippets": ["snippet a1"] },
                    { "title": "A2", "url": "https://a.com/2", "description": "second a", "extra_snippets": ["snippet a2"] },
                    { "title": "A3", "url": "https://a.com/3", "description": "third a" },
                    { "title": "B1", "url": "https://b.com/1", "description": "first b", "extra_snippets": ["snippet b1"] },
                    { "title": "B2", "url": "https://b.com/2", "description": "second b" }
                ]
            }
        }))
    }
    spawn_backend(Router::new().route("/web/search", axum::routing::get(search))).await
}

/// Start a gateway wired to a fake Brave backend for the web-search tool.
async fn gateway_with_web_search(brave: SocketAddr) -> TestServer {
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
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

#[tokio::test]
async fn web_search_returns_results() {
    let brave = fake_brave().await;
    let gateway = gateway_with_web_search(brave).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
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
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
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
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
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
        .post(format!("http://{}/v1/tools/web_search", gateway.addr))
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
        .get(format!("http://{}/health", gateway.addr))
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
        .get(format!("http://{}/v1/models", gateway.addr))
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
        .get(format!("http://{}/v1/models", gateway.addr))
        .bearer_auth("wrong-token")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 401);
}

/// Release handle handed back by the slow backend when a request arrives.
type ReleaseTx = oneshot::Sender<()>;

/// A fake backend that, on each arrival, hands the test a release handle and
/// blocks until it is fired. No sleeps: arrival and release are rendezvous.
async fn completions_slow(
    State(arrivals): State<UnboundedSender<ReleaseTx>>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let (release, released) = oneshot::channel();
    let _ = arrivals.send(release);
    let _ = released.await;
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    Json(canned_reply(&model))
}

async fn slow_fake_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/chat/completions", post(completions_slow))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

async fn gateway_with_queue(
    backend: SocketAddr,
    concurrency: usize,
    max_depth: usize,
) -> TestServer {
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
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

fn chat_body() -> Value {
    serde_json::json!({
        "model": "test-model",
        "messages": [{ "role": "user", "content": "ping" }]
    })
}

fn spawn_chat(
    client: &reqwest::Client,
    url: &str,
) -> JoinHandle<reqwest::Result<reqwest::Response>> {
    let client = client.clone();
    let url = url.to_string();
    tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("test-token")
            .json(&chat_body())
            .send()
            .await
    })
}

async fn next_arrival(arrivals: &mut UnboundedReceiver<ReleaseTx>) -> ReleaseTx {
    tokio::time::timeout(PHASE_TIMEOUT, arrivals.recv())
        .await
        .expect("timed out waiting for backend arrival")
        .expect("arrivals channel closed")
}

#[tokio::test]
async fn concurrency_one_allows_only_one_in_flight_at_backend() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 1, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Second request cannot reach the backend while the first holds the slot.
    let second = spawn_chat(&client, &url);
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "second request must not reach the backend under concurrency=1"
    );

    release_first.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap().status().as_u16(), 200);

    // After the first releases, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    assert_eq!(second.await.unwrap().unwrap().status().as_u16(), 200);
}

#[tokio::test]
async fn concurrency_two_admits_two_in_flight_at_backend() {
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 2, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let second = spawn_chat(&client, &url);

    let release_a = next_arrival(&mut arrivals).await;
    let release_b = next_arrival(&mut arrivals).await;

    release_a.send(()).unwrap();
    release_b.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap().status().as_u16(), 200);
    assert_eq!(second.await.unwrap().unwrap().status().as_u16(), 200);
}

#[tokio::test]
async fn queue_full_returns_503_when_waiting_slots_exhausted() {
    // concurrency=1, max_depth=1: one in-flight + one waiting; the third is 503.
    let (backend, mut arrivals) = slow_fake_backend().await;
    let gateway = gateway_with_queue(backend, 1, 1).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = spawn_chat(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Exactly one of these acquires the single waiting slot; the other is 503.
    let mut second = spawn_chat(&client, &url);
    let mut third = spawn_chat(&client, &url);
    let (rejected, survivor) = tokio::select! {
        r = &mut second => (r, third),
        r = &mut third => (r, second),
    };

    let rejected = rejected.unwrap().unwrap();
    assert_eq!(rejected.status().as_u16(), 503);
    let body: Value = rejected.json().await.unwrap();
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_full")
    );

    release_first.send(()).unwrap();
    assert_eq!(first.await.unwrap().unwrap().status().as_u16(), 200);

    let release_survivor = next_arrival(&mut arrivals).await;
    release_survivor.send(()).unwrap();
    assert_eq!(survivor.await.unwrap().unwrap().status().as_u16(), 200);
}

/// Switch-profile rebuilds the catalog from a remote-only profile (no llama spawn).
#[tokio::test]
async fn switch_profile_updates_models_catalog() {
    use std::fs;

    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();

    let profile_toml = |model: &str, context: u32| {
        format!(
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
name = "{model}"
description = "{model} catalog entry"
context = {context}
upstream = "backend-model"
endpoints = ["fake"]
"#
        )
    };
    fs::write(
        profiles.path().join("alpha.toml"),
        profile_toml("alpha-model", 8192),
    )
    .unwrap();
    fs::write(
        profiles.path().join("beta.toml"),
        profile_toml("beta-model", 4096),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let gateway = Gateway::from_config(&config, context).unwrap();
    let server = TestServer::start(gateway).await;
    let http = reqwest::Client::new();

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["alpha-model"]);

    let listed = http
        .get(format!("http://{}/admin/profiles", server.addr))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(listed["profiles"], serde_json::json!(["alpha", "beta"]));

    let switched = http
        .post(format!("http://{}/admin/switch-profile", server.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "beta" }))
        .send()
        .await
        .unwrap();
    assert_eq!(switched.status().as_u16(), 200);

    let ids = catalog_ids(&http, server.addr).await;
    assert_eq!(ids, vec!["beta-model"]);

    let status = http
        .get(format!("http://{}/admin/status", server.addr))
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

/// A traversal profile name is rejected before touching the filesystem.
#[tokio::test]
async fn switch_profile_rejects_traversal_name() {
    let backend = fake_backend().await;
    let profiles = tempfile::tempdir().unwrap();
    std::fs::write(
        profiles.path().join("alpha.toml"),
        format!(
            "[server]\nbind = \"127.0.0.1:0\"\nkey = \"test-token\"\n\n[[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\nbase_url = \"http://{backend}\"\napi_key = \"\"\n\n[[model]]\nname = \"alpha-model\"\ndescription = \"alpha\"\ncontext = 8192\nupstream = \"backend-model\"\nendpoints = [\"fake\"]\n"
        ),
    )
    .unwrap();

    let alpha = ProfileName::parse("alpha").unwrap();
    let config = Config::load_profile(profiles.path(), &alpha).unwrap();
    let context = ProfilesContext::new(Some(profiles.path().to_path_buf()), Some(alpha));
    let server = TestServer::start(Gateway::from_config(&config, context).unwrap()).await;

    let response = reqwest::Client::new()
        .post(format!("http://{}/admin/switch-profile", server.addr))
        .bearer_auth("test-token")
        .json(&serde_json::json!({ "name": "../escape" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status().as_u16(), 400);
}

async fn catalog_ids(http: &reqwest::Client, addr: SocketAddr) -> Vec<String> {
    let catalog = http
        .get(format!("http://{addr}/v1/models"))
        .bearer_auth("test-token")
        .send()
        .await
        .unwrap()
        .json::<Value>()
        .await
        .unwrap();
    catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}

/// Live local-inference smoke test. Downloads the pinned llama-server binary and
/// the tiny Qwen3-0.6B GGUF, starts a gateway with `[[local_model]]`, and checks
/// that chat completions return text. Ignored by default so CI stays fast.
#[tokio::test]
#[ignore = "downloads llama-server + Qwen3-0.6B; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn local_model_chat_completion_returns_text() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
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
        source = SCENARIO_MODEL_URL,
        sha = SCENARIO_MODEL_SHA256,
    );

    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = tokio::task::spawn_blocking(move || {
        Gateway::from_config(&config, ProfilesContext::default())
    })
    .await
    .unwrap()
    .expect("assemble local gateway");
    let server = TestServer::start(gateway).await;

    let ids = catalog_ids(&reqwest::Client::new(), server.addr).await;
    assert!(ids.iter().any(|id| id == "qwen-tiny"));

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{}/v1", server.addr)).expect("valid test endpoint"),
        SecretString::new("test-token").expect("non-empty test key"),
    );
    let options = CompletionOptions::new(
        "qwen-tiny",
        promptforge_core::dialects::ToolDialectId::OpenAi,
    );
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
    match result.result() {
        promptforge_core::client::CompletionResult::Text(text) => {
            assert!(!text.trim().is_empty(), "local model returned empty text");
        }
        other => panic!("expected text reply, got {other:?}"),
    }
}
