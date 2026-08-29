//! Shared integration-test scaffolding: the [`TestServer`] fixture, fake
//! OpenAI/Brave backends (including a request-recording backend), gateway
//! builders, and rendezvous helpers used across the area modules.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

/// Pinned tiny Qwen3-0.6B GGUF, used only by the ignored live-local test.
#[cfg(feature = "local")]
pub(crate) const SCENARIO_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
#[cfg(feature = "local")]
pub(crate) const SCENARIO_MODEL_SHA256: &str =
    "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031";

/// Pinned tiny bge-small-en-v1.5 GGUF, used only by the ignored live-local
/// embeddings test.
#[cfg(feature = "local")]
pub(crate) const SCENARIO_EMBED_MODEL_URL: &str = "https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5-q8_0.gguf";
#[cfg(feature = "local")]
pub(crate) const SCENARIO_EMBED_MODEL_SHA256: &str =
    "ec38e8da142596baa913124ae50550de284b6916bf59577ef2f0cb9660c2f514";

/// Pinned tiny jina-reranker-v1-tiny-en GGUF, used only by the ignored
/// live-local rerank test.
#[cfg(feature = "local")]
pub(crate) const SCENARIO_RERANK_MODEL_URL: &str = "https://huggingface.co/gpustack/jina-reranker-v1-tiny-en-GGUF/resolve/main/jina-reranker-v1-tiny-en-Q8_0.gguf";
#[cfg(feature = "local")]
pub(crate) const SCENARIO_RERANK_MODEL_SHA256: &str =
    "0defc1f8a1f4dd22183124a2a25a97765603e5a9e42258046c9b2c8a26d1f553";

/// Per-phase timeout so a hung rendezvous fails fast instead of hanging CI.
pub(crate) const PHASE_TIMEOUT: Duration = Duration::from_secs(10);

/// A gateway served on a caller-owned ephemeral listener.
///
/// Prefer [`TestServer::shutdown`] for an explicit, awaited teardown that
/// propagates a serve failure (IT-004). [`Drop`] remains a best-effort fallback
/// for panicking tests that unwind before reaching an explicit shutdown.
pub(crate) struct TestServer {
    pub(crate) addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<Result<(), promptforge_gateway::ServeError>>>,
}

impl TestServer {
    pub(crate) async fn start(gateway: Gateway) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, rx) = oneshot::channel();
        let handle = tokio::spawn(async move {
            gateway
                .serve(listener, async {
                    let _ = rx.await;
                })
                .await
        });
        TestServer {
            addr,
            shutdown: Some(shutdown),
            handle: Some(handle),
        }
    }

    /// Signals graceful shutdown, awaits the serve task within [`PHASE_TIMEOUT`],
    /// and propagates a serve failure instead of discarding it (IT-004).
    pub(crate) async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(PHASE_TIMEOUT, handle)
                .await
                .expect("gateway serve task did not stop within the phase timeout")
                .expect("gateway serve task panicked")
                .expect("gateway serve returned an error");
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Sends a request bounded by [`PHASE_TIMEOUT`] so a hung send fails fast (IT-003).
pub(crate) async fn send_within(builder: reqwest::RequestBuilder) -> reqwest::Response {
    tokio::time::timeout(PHASE_TIMEOUT, builder.send())
        .await
        .expect("HTTP send exceeded the phase timeout")
        .expect("HTTP send failed")
}

/// Reads a JSON body bounded by [`PHASE_TIMEOUT`] (IT-003).
pub(crate) async fn json_within(response: reqwest::Response) -> Value {
    tokio::time::timeout(PHASE_TIMEOUT, response.json::<Value>())
        .await
        .expect("HTTP body read exceeded the phase timeout")
        .expect("HTTP body was not valid JSON")
}

/// Reads a full text body bounded by [`PHASE_TIMEOUT`] (IT-003), for SSE
/// responses whose stream ends when the work behind them completes.
pub(crate) async fn text_within(response: reqwest::Response) -> String {
    tokio::time::timeout(PHASE_TIMEOUT, response.text())
        .await
        .expect("SSE body exceeded the phase timeout")
        .expect("SSE body read failed")
}

/// Parses an SSE body into its `data:` JSON payloads.
pub(crate) fn parse_sse(body: &str) -> Vec<Value> {
    body.split("\n\n")
        .filter(|chunk| !chunk.trim().is_empty())
        .map(|chunk| {
            let data = chunk.trim().strip_prefix("data: ").expect("data prefix");
            serde_json::from_str(data).expect("json event")
        })
        .collect()
}

/// Joins a spawned task bounded by [`PHASE_TIMEOUT`] (IT-003).
pub(crate) async fn join_within<T>(handle: JoinHandle<T>) -> T {
    tokio::time::timeout(PHASE_TIMEOUT, handle)
        .await
        .expect("task join exceeded the phase timeout")
        .expect("joined task panicked")
}

/// Spawn a plain axum backend on an ephemeral port and return its address.
pub(crate) async fn spawn_backend(router: Router) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    addr
}

/// A fake OpenAI backend that echoes the model and returns a canned reply.
pub(crate) async fn fake_backend() -> SocketAddr {
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

pub(crate) fn canned_reply(model: &str) -> Value {
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

/// One request as observed by the recording backend (IT-005/006).
#[derive(Clone, Debug)]
pub(crate) struct RecordedRequest {
    pub(crate) method: String,
    pub(crate) path: String,
    pub(crate) authorization: Option<String>,
    pub(crate) body: Value,
}

/// Shared, thread-safe log of requests the backend received.
pub(crate) type Recorder = Arc<Mutex<Vec<RecordedRequest>>>;

/// A fake OpenAI backend that validates and records each request it receives,
/// then returns the canned reply. The recorder lets a test assert exactly what
/// the gateway forwarded (method, path, bearer, rewritten model, messages).
pub(crate) async fn recording_backend() -> (SocketAddr, Recorder) {
    async fn completions(
        State(recorder): State<Recorder>,
        method: Method,
        uri: axum::http::Uri,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let authorization = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        recorder.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            authorization,
            body: body.clone(),
        });
        Json(canned_reply(&model))
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/chat/completions", post(completions))
        .with_state(Arc::clone(&recorder));
    (spawn_backend(router).await, recorder)
}

pub(crate) fn gateway_config(backend: SocketAddr) -> Config {
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
pub(crate) async fn gateway_for(backend: SocketAddr) -> TestServer {
    let gateway =
        Gateway::from_config(&gateway_config(backend), ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

/// A fake Brave Search backend returning five hits on two hosts.
pub(crate) async fn fake_brave() -> SocketAddr {
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
pub(crate) async fn gateway_with_web_search(brave: SocketAddr) -> TestServer {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

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

/// Release handle handed back by the slow backend when a request arrives.
pub(crate) type ReleaseTx = oneshot::Sender<()>;

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

pub(crate) async fn slow_fake_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/chat/completions", post(completions_slow))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

pub(crate) async fn gateway_with_queue(
    backend: SocketAddr,
    concurrency: usize,
    max_depth: usize,
) -> TestServer {
    let toml = format!(
        r#"
[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = {concurrency}
max_queue = {max_depth}
fair_scheduling = true

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""
dominion = "pool"

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

pub(crate) fn chat_body() -> Value {
    serde_json::json!({
        "model": "test-model",
        "messages": [{ "role": "user", "content": "ping" }]
    })
}

pub(crate) fn spawn_chat(
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

pub(crate) async fn next_arrival(arrivals: &mut UnboundedReceiver<ReleaseTx>) -> ReleaseTx {
    tokio::time::timeout(PHASE_TIMEOUT, arrivals.recv())
        .await
        .expect("timed out waiting for backend arrival")
        .expect("arrivals channel closed")
}

pub(crate) async fn catalog_ids(http: &reqwest::Client, addr: SocketAddr) -> Vec<String> {
    let response = send_within(
        http.get(format!("http://{addr}/v1/models"))
            .bearer_auth("test-token"),
    )
    .await;
    let catalog = json_within(response).await;
    catalog["data"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|model| model.get("id").and_then(Value::as_str).map(str::to_owned))
        .collect()
}
