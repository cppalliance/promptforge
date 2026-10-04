//! Shared integration-test scaffolding: the [`TestServer`] fixture, fake
//! OpenAI/Brave backends (including a request-recording backend), gateway
//! builders, and rendezvous helpers used across the area modules.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

mod backends;
mod process;

#[cfg(feature = "web-search")]
pub(crate) use backends::fake_brave;
pub(crate) use backends::{
    RecordedRequest, Recorder, ReleaseTx, canned_reply, fake_backend, recording_backend,
    slow_fake_backend, spawn_backend,
};
pub(crate) use process::GatewayProcess;

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

/// Polls the discovery file until a spawned gateway publishes its port,
/// the readiness signal for a real-binary fixture.
pub(crate) fn wait_for_connection(
    run_dir: &Path,
    timeout: Duration,
) -> gateway_api_discovery::GatewayDiscoveryFile {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(connection) = gateway_api_discovery::GatewayDiscoveryFile::read(run_dir)
            .expect("read the gateway discovery file")
        {
            return connection;
        }
        assert!(
            Instant::now() < deadline,
            "no Gateway published a connection within {timeout:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(crate) fn gateway_config(backend: SocketAddr) -> Config {
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

/// Starts the gateway wired to the fake backend.
pub(crate) async fn gateway_for(backend: SocketAddr) -> TestServer {
    let gateway =
        Gateway::from_config(&gateway_config(backend), ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

/// Starts a gateway wired to a fake Brave backend for the web-search tool.
#[cfg(feature = "web-search")]
pub(crate) async fn gateway_with_web_search(brave: SocketAddr) -> TestServer {
    let toml = format!(
        r#"
config-version = 0

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

pub(crate) async fn gateway_with_queue(
    backend: SocketAddr,
    concurrency: usize,
    max_depth: usize,
) -> TestServer {
    let toml = format!(
        r#"
config-version = 0

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
