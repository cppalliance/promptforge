//! Live local-inference smoke tests (ignored by default).

use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use crate::support::{
    SCENARIO_EMBED_MODEL_SHA256, SCENARIO_EMBED_MODEL_URL, SCENARIO_RERANK_MODEL_SHA256,
    SCENARIO_RERANK_MODEL_URL, TestServer, catalog_ids, json_within, send_within,
};

/// Live local-embedding smoke test. Downloads the pinned llama-server binary
/// and the tiny bge-small-en-v1.5 GGUF, starts a gateway with a
/// `kind = "embedding"` local model, and checks that `/v1/embeddings` routes
/// through the child and returns one vector per input. Ignored by default so
/// CI stays fast.
#[tokio::test]
#[ignore = "downloads llama-server + bge-small-en-v1.5; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn local_model_embeddings_return_vectors() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
        eprintln!("skipping: set PROMPTFORGE_LIVE_LOCAL=1 to run this test");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = "{cache}"

[[local_model]]
name = "bge-tiny"
kind = "embedding"
description = "A compact English embedding model for retrieval and similarity"
source = "{source}"
sha256 = "{sha}"
context = 512
gpu_layers = 0
flash_attention = false
"#,
        cache = cache.path().display().to_string().replace('\\', "/"),
        source = SCENARIO_EMBED_MODEL_URL,
        sha = SCENARIO_EMBED_MODEL_SHA256,
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
    assert!(ids.iter().any(|id| id == "bge-tiny"));

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/embeddings", server.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "bge-tiny",
                "input": ["first document", "second document"]
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let body = json_within(response).await;
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("bge-tiny"),
        "response carries the caller's model name: {body}"
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
        body.pointer("/data/1/index").and_then(Value::as_u64),
        Some(1),
        "one entry per input: {body}"
    );
    server.shutdown().await;
}

/// Live local-classifier smoke test. Downloads the pinned llama-server binary
/// and the tiny jina-reranker-v1-tiny-en GGUF, starts a gateway with a
/// `kind = "classifier"` local model, and checks that `/v1/rerank` routes
/// through the child and returns one scored result per document. Ignored by
/// default so CI stays fast.
#[tokio::test]
#[ignore = "downloads llama-server + jina-reranker-v1-tiny-en; set PROMPTFORGE_LIVE_LOCAL=1 to opt in"]
async fn local_model_rerank_returns_scores() {
    if std::env::var_os("PROMPTFORGE_LIVE_LOCAL").is_none() {
        eprintln!("skipping: set PROMPTFORGE_LIVE_LOCAL=1 to run this test");
        return;
    }

    let cache = tempfile::tempdir().unwrap();
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[local]
cache_dir = "{cache}"

[[local_model]]
name = "jina-tiny"
kind = "classifier"
description = "A tiny English reranker for scoring query-document relevance"
source = "{source}"
sha256 = "{sha}"
context = 512
gpu_layers = 0
flash_attention = false
"#,
        cache = cache.path().display().to_string().replace('\\', "/"),
        source = SCENARIO_RERANK_MODEL_URL,
        sha = SCENARIO_RERANK_MODEL_SHA256,
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
    assert!(ids.iter().any(|id| id == "jina-tiny"));

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/rerank", server.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "jina-tiny",
                "query": "what is rust",
                "documents": ["a card game", "a systems language"]
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let body = json_within(response).await;
    assert_eq!(
        body.get("model").and_then(Value::as_str),
        Some("jina-tiny"),
        "response carries the caller's model name: {body}"
    );
    let results = body
        .get("results")
        .and_then(Value::as_array)
        .expect("results array passed through");
    assert_eq!(results.len(), 2, "one scored result per document: {body}");
    assert!(
        results
            .iter()
            .all(|result| result.get("relevance_score").is_some_and(Value::is_number)),
        "every result carries a relevance score: {body}"
    );
    server.shutdown().await;
}
