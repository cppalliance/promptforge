//! End-to-end checks that the model client and Gateway agree on their wire contract.

#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "test setup failures should stop the test immediately"
)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::response::IntoResponse as _;
use axum::routing::post;
use axum::{Json, Router};
use gateway::{Config, Gateway, ProfilesContext};
use promptforge_model_client::client::{
    CompletionResult, GatewayClient, GatewayEndpoint, Message, SecretString,
};
use promptforge_model_client::model::CompletionOptions;
use serde_json::Value;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

const PHASE_TIMEOUT: Duration = Duration::from_secs(10);
const SCENARIO_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen3-0.6B-GGUF/resolve/main/Qwen3-0.6B-Q8_0.gguf?download=true";
const SCENARIO_MODEL_SHA256: &str =
    "9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031";

struct TestServer {
    addr: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    handle: Option<JoinHandle<Result<(), gateway::ServeError>>>,
}

impl TestServer {
    async fn start(gateway: Gateway) -> TestServer {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (shutdown, stopped) = oneshot::channel();
        let handle = tokio::spawn(async move {
            gateway
                .serve(listener, async {
                    let _ = stopped.await;
                })
                .await
        });
        TestServer {
            addr,
            shutdown: Some(shutdown),
            handle: Some(handle),
        }
    }

    async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(handle) = self.handle.take() {
            tokio::time::timeout(PHASE_TIMEOUT, handle)
                .await
                .expect("Gateway shutdown timed out")
                .expect("Gateway task panicked")
                .expect("Gateway serve failed");
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

fn sse_reply(model: &str) -> String {
    let chunk = |delta: Value, finish_reason: Value| {
        serde_json::json!({
            "id": "chatcmpl-test",
            "object": "chat.completion.chunk",
            "model": model,
            "choices": [{
                "index": 0,
                "delta": delta,
                "finish_reason": finish_reason
            }]
        })
    };
    let events = [
        chunk(serde_json::json!({ "content": "po" }), Value::Null),
        chunk(serde_json::json!({ "content": "ng" }), Value::Null),
        chunk(serde_json::json!({}), Value::String("stop".to_owned())),
    ];
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

async fn fake_backend() -> SocketAddr {
    async fn completions(Json(body): Json<Value>) -> axum::response::Response {
        let model = body.get("model").and_then(Value::as_str).unwrap_or("");
        (
            [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
            sse_reply(model),
        )
            .into_response()
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            Router::new().route("/chat/completions", post(completions)),
        )
        .await;
    });
    addr
}

async fn complete(server: &TestServer, model: &str, prompt: &str) -> String {
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{}/v1", server.addr)).expect("valid test endpoint"),
        SecretString::new("test-token").expect("non-empty test key"),
    );
    let options = CompletionOptions::new(model);
    let completion = tokio::time::timeout(
        PHASE_TIMEOUT,
        client.complete(&[Message::user(prompt)], None, &options, |_delta| {}),
    )
    .await
    .expect("client completion timed out")
    .expect("client completion failed");
    match completion.result() {
        CompletionResult::Text(reply) => reply.to_owned(),
        other => panic!("expected text reply, got {other:?}"),
    }
}

#[tokio::test]
async fn real_model_client_completes_through_gateway() {
    let backend = fake_backend().await;
    let config = Config::from_toml_str(&format!(
        "config-version = 2\n\
         [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
         [[endpoint]]\nid = \"fake\"\nprotocol = \"openai\"\n\
         base_url = \"http://{backend}\"\napi_key = \"\"\n\
         [[model]]\nname = \"test-model\"\ndescription = \"test\"\n\
         context = 8192\nupstream = \"backend-model\"\nendpoints = [\"fake\"]\n"
    ))
    .expect("Gateway config parses");
    let gateway =
        Gateway::from_config(&config, ProfilesContext::default()).expect("Gateway assembles");
    let server = TestServer::start(gateway).await;

    assert_eq!(complete(&server, "test-model", "ping").await, "pong");
    server.shutdown().await;
}

#[tokio::test]
#[ignore = "downloads llama-server and Qwen3-0.6B; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn real_model_client_completes_through_local_gateway() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
        eprintln!("skipping: set PROMPTFORGE_LIVE_LOCAL=1 to run this test");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let config = Config::from_toml_str(&format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

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
    ))
    .expect("local Gateway config parses");
    let gateway = tokio::task::spawn_blocking(move || {
        Gateway::from_config(&config, ProfilesContext::default())
    })
    .await
    .expect("local Gateway assembly task joins")
    .expect("local Gateway assembles");
    let server = TestServer::start(gateway).await;

    let reply = complete(
        &server,
        "qwen-tiny",
        "Reply with exactly the word pong and nothing else.",
    )
    .await;
    let normalized: String = reply
        .chars()
        .filter(|character| character.is_alphanumeric())
        .collect::<String>()
        .to_lowercase();
    assert_eq!(normalized, "pong", "expected pong, got {reply:?}");
    server.shutdown().await;
}
