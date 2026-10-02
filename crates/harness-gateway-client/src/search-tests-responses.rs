//! Gateway response handling: a refused connection or a stalled gateway is
//! a transport error with its source, a success body with the wrong shape
//! or invalid UTF-8 is a backend error with its source, an oversized
//! success body is rejected rather than truncated, a success body that
//! fails mid-read is a transport error with its source, and an error body
//! is bounded, sanitized, and keeps a mid-read failure as the error source.

use super::*;

use std::time::Duration;

use crate::search::{GatewaySearchErrorKind, MAX_ERROR_BODY, MAX_RESPONSE_BODY};

#[tokio::test]
async fn transport_failure_is_transport_kind() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let err = search_at(&format!("http://{addr}"))
        .search(&query("hi"))
        .await
        .expect_err("a refused connection must surface as an error");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Transport);
    assert_eq!(err.to_string(), "request failed");
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn stalling_gateway_times_out_as_transport() {
    async fn web_search() -> Json<Value> {
        std::future::pending().await
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let search = GatewaySearch::with_timeout(
        GatewayEndpoint::new(&mock.url()).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
        Duration::from_millis(200),
    );

    let err = search
        .search(&query("hi"))
        .await
        .expect_err("a stalled gateway must surface as an error");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Transport);
    assert!(
        std::error::Error::source(&err)
            .and_then(|source| source.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout),
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

    let err = search_at(&mock.url())
        .search(&query("hi"))
        .await
        .expect_err("a wrong-shaped success body must be rejected");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Backend);
    assert_eq!(err.to_string(), "malformed search response");
    assert!(
        std::error::Error::source(&err).is_some(),
        "a malformed response must preserve its parse source"
    );
}

#[tokio::test]
async fn non_utf8_success_body_is_backend_error_with_source() {
    async fn web_search() -> Vec<u8> {
        vec![b'{', 0xff, 0xfe, b'}']
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;

    let err = search_at(&mock.url())
        .search(&query("hi"))
        .await
        .expect_err("a success body that is not UTF-8 must be rejected");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Backend);
    assert_eq!(err.to_string(), "response body was not valid UTF-8");
    assert!(
        std::error::Error::source(&err)
            .and_then(|source| source.downcast_ref::<std::string::FromUtf8Error>())
            .is_some(),
        "the UTF-8 decode failure must be preserved as the error's source"
    );
}

#[tokio::test]
async fn oversized_success_body_is_rejected() {
    async fn web_search() -> String {
        "x".repeat(MAX_RESPONSE_BODY + 4096)
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;

    let err = search_at(&mock.url())
        .search(&query("hi"))
        .await
        .expect_err("an oversized success body must be rejected, not truncated");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Backend);
    assert_eq!(
        err.to_string(),
        format!("response body exceeded {MAX_RESPONSE_BODY} bytes"),
        "the error must name the cap overflow"
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

    let err = search_at(&mock.url())
        .search(&query("hi"))
        .await
        .expect_err("a 500 response must surface as an error");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Backend);
    let message = err.to_string();
    assert!(
        message.starts_with("backend returned 500: line-one\\nline-two\\ttab"),
        "error must name the status and escape the body: {message}"
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

/// A raw TCP mock that answers with `status`, promises a large body via
/// `Content-Length`, sends a few bytes, then drops the connection so the
/// body read fails partway.
async fn spawn_truncated_reply(status: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header = format!("HTTP/1.1 {status}\r\nContent-Length: 100000\r\n\r\n");
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(b"partial").await;
            let _ = socket.flush().await;
        }
    });
    (addr, handle)
}

#[tokio::test]
async fn success_body_read_failure_is_transport_error_with_source() {
    let (addr, handle) = spawn_truncated_reply("200 OK").await;

    let err = search_at(&format!("http://{addr}"))
        .search(&query("hi"))
        .await
        .expect_err("a truncated 200 body must surface as an error");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Transport);
    assert_eq!(err.to_string(), "reading response failed");
    assert!(
        std::error::Error::source(&err)
            .and_then(|source| source.downcast_ref::<reqwest::Error>())
            .is_some(),
        "the body-read failure must be preserved as the error's source, got: {err}"
    );
    handle.abort();
}

#[tokio::test]
async fn error_body_read_failure_is_preserved_as_source() {
    let (addr, handle) = spawn_truncated_reply("500 Internal Server Error").await;

    let err = search_at(&format!("http://{addr}"))
        .search(&query("hi"))
        .await
        .expect_err("a truncated 500 body must surface as an error");
    assert_eq!(err.kind(), GatewaySearchErrorKind::Backend);
    assert_eq!(
        err.to_string(),
        "backend returned 500, and its error body could not be read"
    );
    assert!(
        std::error::Error::source(&err).is_some(),
        "the body-read failure must be preserved as the error's source, got: {err}"
    );
    handle.abort();
}
