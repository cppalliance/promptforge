//! Shared integration-test scaffolding: the [`TestServer`] fixture, fake
//! OpenAI/Brave backends (including a request-recording backend), gateway
//! builders, and rendezvous helpers used across the area modules.

use std::io::Read as _;
use std::net::SocketAddr;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

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
const TEST_START_READY_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_READY";
const TEST_START_RELEASE_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_RELEASE";

/// A real Gateway child whose teardown is bounded even when a race test
/// panics before its ordinary shutdown path.
pub(crate) struct GatewayProcess {
    child: Child,
}

impl GatewayProcess {
    /// Starts the production binary against an isolated profile and config.
    pub(crate) fn spawn(config: &Path, home: &Path) -> Self {
        Self::spawn_command(config, home)
            .spawn()
            .map(|child| Self { child })
            .expect("the Gateway race fixture spawns")
    }

    /// Starts the production binary paused immediately before lease
    /// acquisition until `release` exists.
    #[cfg(feature = "test-fixtures")]
    pub(crate) fn spawn_gated(config: &Path, home: &Path, ready: &Path, release: &Path) -> Self {
        let child = Self::spawn_command(config, home)
            .env(TEST_START_READY_ENV, ready)
            .env(TEST_START_RELEASE_ENV, release)
            .spawn()
            .expect("the gated Gateway race fixture spawns");
        Self { child }
    }

    /// Starts the default binary with rendezvous-looking environment that
    /// must be inert when the test fixture feature is absent.
    #[cfg(not(feature = "test-fixtures"))]
    pub(crate) fn spawn_with_inert_rendezvous(
        config: &Path,
        home: &Path,
        ready: &Path,
        release: &Path,
    ) -> Self {
        let child = Self::spawn_command(config, home)
            .env(TEST_START_READY_ENV, ready)
            .env(TEST_START_RELEASE_ENV, release)
            .spawn()
            .expect("the default Gateway fixture spawns");
        Self { child }
    }

    fn spawn_command(config: &Path, home: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_promptforge-gateway"));
        command
            .arg("--config")
            .arg(config)
            .arg("--profile")
            .arg("main")
            .arg("--print-url")
            .env("USERPROFILE", home)
            .env("HOME", home)
            .env_remove("RUST_LOG")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub(crate) fn id(&self) -> u32 {
        self.child.id()
    }

    pub(crate) fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child
            .try_wait()
            .expect("observe the Gateway race fixture")
    }

    pub(crate) fn wait_for_exit(&mut self, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.try_wait() {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "the Gateway race fixture did not exit within {timeout:?}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[cfg(feature = "test-fixtures")]
    pub(crate) fn stdout(&mut self) -> String {
        let mut output = String::new();
        self.child
            .stdout
            .take()
            .expect("the Gateway fixture has piped stdout")
            .read_to_string(&mut output)
            .expect("read the Gateway fixture stdout");
        output
    }

    pub(crate) fn stderr(&mut self) -> String {
        let mut output = String::new();
        self.child
            .stderr
            .take()
            .expect("the Gateway fixture has piped stderr")
            .read_to_string(&mut output)
            .expect("read the Gateway fixture stderr");
        output
    }

    pub(crate) fn stop(&mut self, timeout: Duration) {
        if self.try_wait().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.wait_for_exit(timeout);
    }
}

impl Drop for GatewayProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.child.try_wait().ok().flatten().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// A gateway served on a caller-owned ephemeral listener.
///
/// Prefer [`TestServer::shutdown`] for an explicit, awaited teardown that
/// propagates a serve failure (IT-004). [`Drop`] remains a best-effort fallback
/// for panicking tests that unwind before reaching an explicit shutdown.
pub(crate) struct TestServer {
    pub(crate) addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<Result<(), gateway::ServeError>>>,
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

    /// Awaits the serve task without sending the shutdown signal, for
    /// tests that stop the server through its own HTTP surface
    /// (`POST /shutdown`): only the route's signal can end the task.
    pub(crate) async fn join(mut self) {
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(PHASE_TIMEOUT, handle)
                .await
                .expect("gateway serve task did not stop within the phase timeout")
                .expect("gateway serve task panicked")
                .expect("gateway serve returned an error");
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

/// A fake OpenAI backend that echoes the model and returns a canned reply,
/// speaking SSE when the request asks to stream and JSON otherwise.
pub(crate) async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> axum::response::Response {
        use axum::response::IntoResponse;
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        if body.get("stream").and_then(Value::as_bool) == Some(true) {
            return (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                canned_sse_reply(&model),
            )
                .into_response();
        }
        Json(canned_reply(&model)).into_response()
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

/// The streamed form of [`canned_reply`]: two content chunks, the finish
/// chunk, and the `[DONE]` sentinel.
pub(crate) fn canned_sse_reply(model: &str) -> String {
    let chunk = |delta: Value, finish: Value| {
        serde_json::json!({
            "id": "cmpl-test",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }]
        })
    };
    let events = [
        chunk(serde_json::json!({ "content": "po" }), Value::Null),
        chunk(serde_json::json!({ "content": "ng" }), Value::Null),
        chunk(serde_json::json!({}), Value::String("stop".to_owned())),
    ];
    let mut body = String::new();
    for event in &events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
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
config-version = 2

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
#[cfg(feature = "web-search")]
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
#[cfg(feature = "web-search")]
pub(crate) async fn gateway_with_web_search(brave: SocketAddr) -> TestServer {
    let toml = format!(
        r#"
config-version = 2

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
config-version = 2

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
