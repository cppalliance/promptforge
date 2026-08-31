//! Rerank route: remote passthrough and the kind guard.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, Method, header::AUTHORIZATION};
use axum::routing::post;
use axum::{Json, Router};
use promptforge_gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use crate::support::{
    RecordedRequest, Recorder, TestServer, json_within, send_within, spawn_backend,
};

/// A canned Jina/vLLM-shaped rerank reply echoing the (rewritten) model.
fn canned_rerank(model: &str) -> Value {
    serde_json::json!({
        "model": model,
        "results": [
            { "index": 1, "relevance_score": 0.9, "document": { "text": "a systems language" } },
            { "index": 0, "relevance_score": 0.1 }
        ],
        "usage": { "total_tokens": 12 }
    })
}

/// A fake backend that records each rerank request, then replies.
async fn recording_rerank_backend() -> (SocketAddr, Recorder) {
    async fn rerank(
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
        Json(canned_rerank(&model))
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/rerank", post(rerank))
        .with_state(Arc::clone(&recorder));
    (spawn_backend(router).await, recorder)
}

/// Start a gateway serving one remote classifier model.
async fn rerank_gateway(backend: SocketAddr) -> TestServer {
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
name = "rerank-model"
kind = "classifier"
description = "a reranker model for integration"
context = 8192
upstream = "backend-rerank"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

fn rerank_body() -> Value {
    serde_json::json!({
        "model": "rerank-model",
        "query": "what is rust",
        "documents": ["a card game", "a systems language"],
        "top_n": 2
    })
}

/// IT-005/006 for the rerank route: the backend records the request, so we
/// assert exactly what the gateway forwarded - method, path, the rewritten
/// upstream model, the intact query, document set and top_n - and that the
/// client's bearer is not leaked upstream. The response restores the caller's
/// model name and passes `results` and `usage` through.
#[tokio::test]
async fn remote_passthrough_rewrites_model_and_relays_response() {
    let (backend, recorder) = recording_rerank_backend().await;
    let gateway = rerank_gateway(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/rerank", gateway.addr))
            .bearer_auth("test-token")
            .json(&rerank_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let body = json_within(response).await;
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("rerank-model"),
        "response carries the caller's model name, not the backend's"
    );
    assert_eq!(
        body.pointer("/results/0/relevance_score")
            .and_then(Value::as_f64),
        Some(0.9),
        "ranked scores passed through: {body}"
    );
    assert_eq!(
        body.pointer("/usage/total_tokens").and_then(Value::as_u64),
        Some(12),
        "usage passed through: {body}"
    );

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/rerank");
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-rerank"),
        "public model name rewritten to the upstream alias"
    );
    assert_eq!(
        request.body.get("query").and_then(Value::as_str),
        Some("what is rust"),
        "query forwarded intact"
    );
    assert_eq!(
        request.body.pointer("/documents/1").and_then(Value::as_str),
        Some("a systems language"),
        "document set forwarded intact"
    );
    assert_eq!(request.body.get("top_n").and_then(Value::as_u64), Some(2));
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// A model configured for a non-classifier kind is rejected on the rerank
/// route with 400 and `kind_mismatch` before any queue admission or upstream
/// call.
#[tokio::test]
async fn non_classifier_kinds_are_rejected_on_the_rerank_route() {
    let (backend, recorder) = recording_rerank_backend().await;
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

    for model in ["chat-model", "embed-model"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/rerank", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "query": "what is rust",
                    "documents": ["a systems language"]
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
