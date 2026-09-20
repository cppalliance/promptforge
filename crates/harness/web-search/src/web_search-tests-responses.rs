//! Gateway response handling: a refused connection or a stalled gateway is
//! a transport error with its source, a wrong-shaped or empty-url success
//! body is a backend error, an oversized success body is rejected rather
//! than truncated, and an error body is bounded, sanitized, and keeps a
//! mid-read failure as the error source.

use super::*;

use std::time::Duration;

use crate::web_search::{MAX_ERROR_BODY, MAX_RESPONSE_BODY};

#[tokio::test]
async fn transport_failure_is_transport_kind() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let tool =
        WebSearch::new(&format!("http://{addr}"), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a refused connection must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn stalling_gateway_times_out_as_transport() {
    async fn web_search() -> Json<Value> {
        std::future::pending().await
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::with_timeout(&mock.url(), "tok", Duration::from_millis(200))
        .expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a stalled gateway must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert!(
        std::error::Error::source(&err).is_some(),
        "the timeout must be preserved as the error's transport source"
    );
}

#[tokio::test]
async fn malformed_success_json_is_backend_error_with_source() {
    async fn web_search() -> Json<Value> {
        // Missing the required `results` array: valid JSON, wrong shape.
        Json(serde_json::json!({ "unexpected": true }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a wrong-shaped success body must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        std::error::Error::source(&err).is_some(),
        "a malformed response must preserve its parse source"
    );
}

#[tokio::test]
async fn success_body_with_empty_url_is_rejected() {
    async fn web_search() -> Json<Value> {
        Json(serde_json::json!({ "results": [{ "url": "" }] }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("an empty result url must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
}

#[tokio::test]
async fn oversized_success_body_is_rejected() {
    async fn web_search() -> String {
        "x".repeat(MAX_RESPONSE_BODY + 4096)
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("an oversized success body must be rejected, not truncated");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        err.to_string().contains("exceeded"),
        "the error must name the cap overflow: {err}"
    );
}

#[tokio::test]
async fn oversized_error_body_is_bounded_and_sanitized() {
    async fn web_search() -> (axum::http::StatusCode, String) {
        // Oversized and control-laden so both bounding and sanitization run.
        let mut body = "line-one\nline-two\ttab".to_owned();
        body.push_str(&"e".repeat(MAX_ERROR_BODY * 4));
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body)
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a 500 response must surface as an error");
    let message = err.to_string();
    assert!(
        message.contains("backend returned 500"),
        "error must name the status: {message}"
    );
    assert!(
        !message.contains('\n') && !message.contains('\t'),
        "control characters must be escaped, got: {message}"
    );
    assert!(
        message.len() < MAX_ERROR_BODY + 128,
        "the error-path body must be bounded, got {} bytes",
        message.len()
    );
}

/// A raw TCP mock that promises a large body via `Content-Length`, sends a
/// few bytes, then drops the connection so the error-body read fails partway.
#[tokio::test]
async fn error_body_read_failure_is_preserved_as_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = spawn_tagged(mock_tag(), async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100000\r\n\r\n";
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(b"partial").await;
            let _ = socket.flush().await;
        }
    });
    let tool =
        WebSearch::new(&format!("http://{addr}"), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a truncated 500 body must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        std::error::Error::source(&err).is_some(),
        "the body-read failure must be preserved as the error's source, got: {err}"
    );
    handle.abort();
}
