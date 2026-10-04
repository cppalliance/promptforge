//! Tests for the embeddings passthrough and the declining embeddings default.

use super::*;

fn embedding_request(model: &str) -> EmbeddingRequest {
    EmbeddingRequest {
        model: model.to_owned(),
        input: crate::wire::EmbeddingInput::One("embed me".to_owned()),
        encoding_format: None,
        rest: Map::new(),
    }
}

#[tokio::test]
async fn embeddings_rewrites_caller_model_and_posts_to_embeddings() {
    // UP-008: same contract as chat - the caller's model name is restored
    // on the response while the upstream model is what the backend sees.
    let (base, handle) = serve_once(
        "200 OK",
        r#"{"object":"list","model":"backend-embed","data":[{"object":"embedding","index":0,"embedding":[0.1,0.2]}],"usage":{"prompt_tokens":2,"total_tokens":2}}"#,
    );
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let response = upstream
        .send_embeddings(embedding_request("caller-model"), "backend-embed")
        .await
        .expect("send ok");
    assert_eq!(response.model, "caller-model");
    assert_eq!(response.data.len(), 1);
    assert!(response.rest.contains_key("usage"));
    let sent = handle.join().expect("join");
    assert!(sent.contains("POST /embeddings"), "{sent}");
    assert!(sent.contains("backend-embed"), "forwarded body: {sent}");
    assert!(
        !sent.contains("caller-model"),
        "caller model leaked: {sent}"
    );
}

#[tokio::test]
async fn embeddings_non_success_status_is_upstream_status() {
    let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .send_embeddings(embedding_request("m"), "u")
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
        "expected UpstreamStatus 500, got {err:?}"
    );
    let _ = handle.join();
}

#[tokio::test]
async fn default_send_embeddings_is_model_unavailable() {
    // Upstreams without an embeddings implementation (a local chat server)
    // decline the workload with ModelUnavailable naming the caller's model.
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
        .send_embeddings(embedding_request("local-chat"), "ignored-alias")
        .await
        .expect_err("default must decline");
    match err {
        ProtocolError::ModelUnavailable(model) => assert_eq!(model, "local-chat"),
        other => panic!("expected ModelUnavailable, got {other:?}"),
    }
}
