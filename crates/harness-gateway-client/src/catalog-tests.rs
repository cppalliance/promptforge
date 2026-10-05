//! Tests for the model catalog fetch against an axum mock gateway: the body
//! bounds, the preserved failure sources, the entry filter, and failure
//! messages that carry no Gateway-supplied text.

use super::*;
use crate::CompletionErrorKind;

async fn spawn_models(app: axum::Router) -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

#[tokio::test]
async fn fetch_model_catalog_bounds_and_reports_non_success_body() {
    use axum::Router;
    use axum::routing::get;

    async fn models() -> (axum::http::StatusCode, String) {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "e".repeat(MAX_CATALOG_ERROR_BODY * 4),
        )
    }
    let app = Router::new().route("/models", get(models));
    let addr = spawn_models(app).await;

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a 500 response must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::ServerError);
    assert_eq!(
        err.to_string(),
        "the model backend reported a fault of its own (status 500)"
    );
    let detail = err.detail().expect("the bounded body is the detail");
    assert!(
        detail.len() < MAX_CATALOG_ERROR_BODY + 128,
        "the error-path body must be bounded, got {} bytes",
        detail.len()
    );
}

#[tokio::test]
async fn fetch_model_catalog_bounds_an_oversized_success_body() {
    use axum::Router;
    use axum::routing::get;

    // A 200 response whose body exceeds the success cap must be refused
    // BEFORE decoding, not buffered unbounded. The body is deliberately not
    // valid JSON: the bound must trip first, regardless of contents.
    async fn models() -> (axum::http::StatusCode, String) {
        let oversized = usize::try_from(MAX_CATALOG_BODY).unwrap() + 1;
        (axum::http::StatusCode::OK, "e".repeat(oversized))
    }
    let app = Router::new().route("/models", get(models));
    let addr = spawn_models(app).await;

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("an oversized success body must be refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        err.to_string().contains("exceeds"),
        "the bound must report the size limit, got {err:?}"
    );
    assert_eq!(err.detail(), None, "our own wording is not provider text");
}

#[tokio::test]
async fn fetch_model_catalog_preserves_the_json_decode_source() {
    use axum::Router;
    use axum::routing::get;

    // MODEL-009: a 200 body that is not a valid model list is classified as
    // MalformedResponse, and the underlying `serde_json::Error` survives as
    // the error-chain `#[source]` rather than being flattened into the text.
    async fn models() -> (axum::http::StatusCode, String) {
        (axum::http::StatusCode::OK, "{ this is not json".to_owned())
    }
    let app = Router::new().route("/models", get(models));
    let addr = spawn_models(app).await;

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("an undecodable body must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    let source =
        std::error::Error::source(&err).expect("the decode error must be a preserved source");
    assert!(
        source.downcast_ref::<serde_json::Error>().is_some(),
        "the preserved source must be the JSON decode error, got {source}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_skips_entries_without_a_context_window() {
    use axum::Router;
    use axum::routing::get;

    // A gateway with speech-to-text lists its transcription models beside
    // the inference models, and those entries omit `context` and
    // `thinking` (they answer no completion request). The fetch must keep
    // the inference descriptors instead of rejecting the whole list,
    // otherwise the Harness binds each run on such a gateway to a fallback
    // descriptor and the context precheck refuses real conversations.
    async fn models() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "object": "list",
            "data": [
                { "id": "chat-model", "object": "model", "kind": "chat",
                  "description": "a chat model", "context": 1_000_000, "thinking": "never" },
                { "id": "whisper-base-en", "object": "model", "kind": "transcription" },
                { "id": "whisper-small-en", "object": "model", "kind": "transcription" }
            ]
        }))
    }
    let app = Router::new().route("/models", get(models));
    let addr = spawn_models(app).await;

    let catalog = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect("transcription entries must not fail the inference catalog");
    assert_eq!(
        catalog.models().len(),
        1,
        "only the inference model is a descriptor"
    );
    let chat = catalog
        .get(&ModelId::gateway("chat-model").expect("valid id"))
        .expect("the inference model survives the filter");
    assert_eq!(
        chat.context(),
        NonZeroU32::new(1_000_000).expect("non-zero")
    );
    assert_eq!(chat.thinking(), ThinkingMode::Never);
}

#[tokio::test]
async fn fetch_model_catalog_still_rejects_a_zero_context_window() {
    use axum::Router;
    use axum::routing::get;

    // Skipping applies only to entries with no context field at all; an
    // inference entry that declares a zero window is still malformed.
    async fn models() -> axum::Json<serde_json::Value> {
        axum::Json(serde_json::json!({
            "object": "list",
            "data": [
                { "id": "broken", "object": "model", "kind": "chat",
                  "description": "d", "context": 0, "thinking": "never" }
            ]
        }))
    }
    let app = Router::new().route("/models", get(models));
    let addr = spawn_models(app).await;

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a zero context window is malformed");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(err.to_string().contains("zero-token"), "got {err:?}");
    assert!(!err.to_string().contains("broken"), "got {err:?}");
    assert!(
        err.detail().is_some_and(|detail| detail.contains("broken")),
        "the model name belongs in the detail, got {err:?}"
    );
}

/// Serves `data` as the gateway model list and returns the fetch failure.
async fn fetch_rejection(data: serde_json::Value) -> CompletionError {
    let list = serde_json::json!({ "object": "list", "data": data });
    let app = axum::Router::new().route(
        "/models",
        axum::routing::get(move || async move { axum::Json(list) }),
    );
    let addr = spawn_models(app).await;
    fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("the model list is malformed")
}

#[tokio::test]
async fn fetch_model_catalog_moves_the_name_of_a_model_without_a_thinking_mode_to_detail() {
    let err = fetch_rejection(serde_json::json!([
        { "id": "thoughtless", "object": "model", "kind": "chat",
          "description": "d", "context": 4096 }
    ]))
    .await;
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(!err.to_string().contains("thoughtless"), "got {err:?}");
    assert!(
        err.detail()
            .is_some_and(|detail| detail.contains("thoughtless")),
        "the model name belongs in the detail, got {err:?}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_moves_a_duplicate_id_to_detail() {
    let entry = serde_json::json!(
        { "id": "twin", "object": "model", "kind": "chat",
          "description": "d", "context": 4096, "thinking": "never" }
    );
    let err = fetch_rejection(serde_json::json!([entry.clone(), entry])).await;
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(!err.to_string().contains("twin"), "got {err:?}");
    assert!(
        err.detail()
            .is_some_and(|detail| detail.contains("gateway/twin")),
        "the duplicate identity belongs in the detail, got {err:?}"
    );
}

#[tokio::test]
async fn fetch_model_catalog_bounds_a_gateway_supplied_detail() {
    let long = "n".repeat(MAX_CATALOG_ERROR_BODY * 4);
    let entry = |context: u32, thinking: Option<&str>| {
        serde_json::json!({ "id": long, "object": "model", "kind": "chat",
            "description": "d", "context": context, "thinking": thinking })
    };
    let rejections = [
        serde_json::json!([entry(0, Some("never"))]),
        serde_json::json!([entry(4096, None)]),
        serde_json::json!([entry(4096, Some("never")), entry(4096, Some("never"))]),
    ];
    for data in rejections {
        let err = fetch_rejection(data).await;
        assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
        let detail = err.detail().expect("the model name is the detail");
        assert!(
            detail.chars().count() <= MAX_CATALOG_ERROR_BODY,
            "a Gateway-supplied detail must be bounded, got {} chars",
            detail.chars().count()
        );
    }
}

#[tokio::test]
async fn fetch_model_catalog_keeps_an_invalid_id_out_of_the_message() {
    let err = fetch_rejection(serde_json::json!([
        { "id": "smuggled\u{7}id", "object": "model", "kind": "chat",
          "description": "d", "context": 4096, "thinking": "never" }
    ]))
    .await;
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    let message = err.to_string();
    assert!(!message.contains("smuggled"), "got {err:?}");
    assert!(!message.contains('\u{7}'), "got {err:?}");
}

#[tokio::test]
async fn fetch_model_catalog_preserves_a_body_read_failure_source() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // MODEL-010: a non-success response whose body cannot be fully read
    // (the server promises a large body then drops the connection) must
    // surface as a typed transport failure that keeps the `reqwest::Error`
    // as its `#[source]`, not display text.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let header = "HTTP/1.1 500 Internal Server Error\r\n\
                 Content-Length: 1000000\r\n\r\n";
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock.write_all(b"abc").await;
            // Socket drops here: the promised body never completes.
        }
    });

    let err = fetch_model_catalog(&format!("http://{addr}"), "tok")
        .await
        .expect_err("a truncated error body must surface as an error");
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    assert_eq!(
        err.to_string(),
        "the connection to the model backend failed"
    );
    let source =
        std::error::Error::source(&err).expect("the read failure must be a preserved source");
    assert!(
        source.downcast_ref::<reqwest::Error>().is_some(),
        "the preserved source must be the reqwest read error, got {source}"
    );
}
