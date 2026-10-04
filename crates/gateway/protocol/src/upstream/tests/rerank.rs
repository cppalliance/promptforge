//! Tests for the rerank passthrough and the declining rerank default.

use super::*;

fn rerank_request(model: &str) -> RerankRequest {
    RerankRequest {
        model: model.to_owned(),
        query: "what is rust".to_owned(),
        documents: vec!["a systems language".to_owned()],
        top_n: None,
        rest: Map::new(),
    }
}

#[tokio::test]
async fn rerank_rewrites_caller_model_and_posts_to_rerank() {
    // UP-008: same contract as chat - the caller's model name is restored
    // on the response while the upstream model is what the backend sees.
    let (base, handle) = serve_once(
        "200 OK",
        r#"{"model":"backend-rerank","results":[{"index":0,"relevance_score":0.9}],"usage":{"total_tokens":5}}"#,
    );
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let response = upstream
        .send_rerank(rerank_request("caller-model"), "backend-rerank")
        .await
        .expect("send ok");
    assert_eq!(response.model, "caller-model");
    assert_eq!(response.results.len(), 1);
    assert!(response.rest.contains_key("usage"));
    let sent = handle.join().expect("join");
    assert!(sent.contains("POST /rerank"), "{sent}");
    assert!(sent.contains("backend-rerank"), "forwarded body: {sent}");
    assert!(
        !sent.contains("caller-model"),
        "caller model leaked: {sent}"
    );
}

#[tokio::test]
async fn rerank_non_success_status_is_upstream_status() {
    let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .send_rerank(rerank_request("m"), "u")
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
        "expected UpstreamStatus 500, got {err:?}"
    );
    let _ = handle.join();
}

#[tokio::test]
async fn default_send_rerank_is_model_unavailable() {
    // Upstreams without a rerank implementation (a local chat server, for
    // example) decline the workload with ModelUnavailable naming the
    // caller's model.
    struct ChatOnly;

    #[async_trait]
    impl Upstream for ChatOnly {
        async fn send(
            &self,
            _req: ChatRequest,
            _upstream_model: &str,
        ) -> Result<ChatResponse, ProtocolError> {
            unreachable!("not under test")
        }
    }

    let err = ChatOnly
        .send_rerank(rerank_request("local-classifier"), "ignored-alias")
        .await
        .expect_err("default must decline");
    match err {
        ProtocolError::ModelUnavailable(model) => assert_eq!(model, "local-classifier"),
        other => panic!("expected ModelUnavailable, got {other:?}"),
    }
}
