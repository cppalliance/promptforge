//! Embeddings route: remote passthrough, dominion queue admission, and the
//! kind guard.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::support::{
    RecordedRequest, Recorder, ReleaseTx, TestServer, join_within, json_within, next_arrival,
    send_within, spawn_backend,
};

/// A canned OpenAI-shaped embeddings reply echoing the (rewritten) model.
fn canned_embeddings(model: &str) -> Value {
    serde_json::json!({
        "object": "list",
        "model": model,
        "data": [{ "object": "embedding", "index": 0, "embedding": [0.1, 0.2, 0.3] }],
        "usage": { "prompt_tokens": 3, "total_tokens": 3 }
    })
}

/// A fake backend that records each embeddings request, then replies.
async fn recording_embeddings_backend() -> (SocketAddr, Recorder) {
    async fn embeddings(
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
        recorder.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            authorization,
            body: body.clone(),
        });
        let model = body
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Json(canned_embeddings(&model))
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/embeddings", post(embeddings))
        .with_state(Arc::clone(&recorder));
    (spawn_backend(router).await, recorder)
}

/// A fake embeddings backend that hands the test a release handle on each
/// arrival and blocks until it is fired. No sleeps: arrival and release are
/// rendezvous.
async fn slow_embeddings_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    async fn embeddings(
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
        Json(canned_embeddings(&model))
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/embeddings", post(embeddings))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// Start a gateway serving one remote embedding model. With
/// `max_concurrency`, the endpoint is bound to a dominion pool capped at that
/// many in-flight requests; without it the endpoint is an unlimited
/// pass-through.
async fn embeddings_gateway(backend: SocketAddr, max_concurrency: Option<usize>) -> TestServer {
    let (dominion_block, binding) = match max_concurrency {
        Some(limit) => (
            format!(
                r#"
[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = {limit}
"#
            ),
            "\ndominion = \"pool\"",
        ),
        None => (String::new(), ""),
    };
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
{dominion_block}
[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""{binding}

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model for integration"
context = 8192
upstream = "backend-embed"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

fn embedding_body() -> Value {
    serde_json::json!({
        "model": "embed-model",
        "input": ["first document", "second document"],
        "encoding_format": "float"
    })
}

fn spawn_embeddings(
    client: &reqwest::Client,
    url: &str,
) -> tokio::task::JoinHandle<reqwest::Result<reqwest::Response>> {
    let client = client.clone();
    let url = url.to_string();
    tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("test-token")
            .json(&embedding_body())
            .send()
            .await
    })
}

/// IT-005/006 for the embeddings route: the backend records the request, so we
/// assert exactly what the gateway forwarded - method, path, the rewritten
/// upstream model, the intact input batch and encoding format - and that the
/// client's bearer is not leaked upstream. The response restores the caller's
/// model name and passes `data` and `usage` through.
#[tokio::test]
async fn remote_passthrough_rewrites_model_and_relays_response() {
    let (backend, recorder) = recording_embeddings_backend().await;
    let gateway = embeddings_gateway(backend, None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/embeddings", gateway.addr))
            .bearer_auth("test-token")
            .json(&embedding_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let body = json_within(response).await;
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("embed-model"),
        "response carries the caller's model name, not the backend's"
    );
    assert_eq!(
        body.pointer("/data/0/index").and_then(Value::as_u64),
        Some(0)
    );
    assert!(
        body.pointer("/data/0/embedding")
            .is_some_and(Value::is_array),
        "embedding vector passed through: {body}"
    );
    assert_eq!(
        body.pointer("/usage/total_tokens").and_then(Value::as_u64),
        Some(3),
        "usage passed through: {body}"
    );

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/embeddings");
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-embed"),
        "public model name rewritten to the upstream alias"
    );
    assert_eq!(
        request.body.pointer("/input/1").and_then(Value::as_str),
        Some("second document"),
        "input batch forwarded intact"
    );
    assert_eq!(
        request.body.get("encoding_format").and_then(Value::as_str),
        Some("float")
    );
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// The embeddings handler admits through the model's dominion queue exactly
/// like chat: under `max_concurrency = 1` a second request cannot reach the
/// backend until the first releases the slot.
#[tokio::test]
async fn embeddings_requests_hold_the_dominion_slot_across_the_upstream_call() {
    let (backend, mut arrivals) = slow_embeddings_backend().await;
    let gateway = embeddings_gateway(backend, Some(1)).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/embeddings", gateway.addr);

    let first = spawn_embeddings(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Second request cannot reach the backend while the first holds the slot.
    let second = spawn_embeddings(&client, &url);
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "second request must not reach the backend under concurrency=1"
    );

    release_first.send(()).unwrap();
    assert_eq!(join_within(first).await.unwrap().status().as_u16(), 200);

    // After the first releases, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    assert_eq!(join_within(second).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

/// A model configured for a non-embedding kind is rejected on the embeddings
/// route with 400 and `kind_mismatch` before any queue admission or upstream
/// call.
#[tokio::test]
async fn non_embedding_kinds_are_rejected_on_the_embeddings_route() {
    let (backend, recorder) = recording_embeddings_backend().await;
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
name = "chat-model"
description = "a chat model"
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
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    for model in ["chat-model", "reranker"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/embeddings", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "input": "embed me"
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
    }
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a kind-mismatched request must never reach the backend"
    );
    gateway.shutdown().await;
}
