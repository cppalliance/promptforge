//! The bounds and refusals: the disabled sentinel, the byte caps on both
//! paths, the request timeout, and the malformed or cut-off stream.

use std::num::NonZeroU64;
use std::time::Duration;

use promptforge_api_runtime::model::Message;

use super::*;
use crate::CompletionErrorKind;

#[tokio::test]
async fn complete_on_a_disabled_client_is_a_disabled_error() {
    // F14: a disabled client never touches the network.
    let client = GatewayClient::disabled();
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a disabled client cannot complete");
    assert_eq!(err.kind(), CompletionErrorKind::Disabled);
}

#[tokio::test]
async fn backend_error_display_is_body_free_and_body_is_opt_in_and_escaped() {
    use axum::Router;
    use axum::routing::post;

    // A non-success body holding control characters and a would-be secret.
    async fn handler() -> (axum::http::StatusCode, String) {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            "forged\nlog: super-secret".to_owned(),
        )
    }
    let app = Router::new().route("/v1/chat/completions", post(handler));
    let client = client_for(app).await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a 502 must surface as a backend error");

    // F5: the public Display names only the status, never the raw body.
    let shown = err.to_string();
    assert!(shown.contains("502"), "status must appear, got {shown}");
    assert!(
        !shown.contains("super-secret") && !shown.contains('\n'),
        "the raw body must not appear in Display, got {shown}"
    );
    // The bounded, control-escaped body is available only via the opt-in.
    let body = err
        .backend_body()
        .expect("backend body is available opt-in");
    assert!(
        body.contains("\\n"),
        "control chars must be escaped, got {body}"
    );
    assert!(
        !body.contains('\n'),
        "no raw newline in the diagnostic body"
    );
}

#[tokio::test]
async fn complete_refuses_a_success_stream_over_the_size_cap() {
    // F14 (body-size, success path): a 200 stream larger than the cap is
    // refused as the bytes arrive, before any further parsing.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a long reply\"}}]}\n\n",
    )
    .await;
    let client = keyed_client(&base).with_request_limits(
        DEFAULT_REQUEST_TIMEOUT,
        NonZeroU64::new(8).expect("non-zero cap"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an oversize stream must be refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
}

#[tokio::test]
async fn complete_refuses_a_backend_error_body_over_the_size_cap() {
    // F14 (body-size, error path): a non-success body larger than the cap is
    // also refused before it is buffered.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "this backend error body is definitely longer than eight bytes",
    )
    .await;
    let client = keyed_client(&base).with_request_limits(
        DEFAULT_REQUEST_TIMEOUT,
        NonZeroU64::new(8).expect("non-zero cap"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an oversize error body must be refused");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
}

#[tokio::test]
async fn a_request_past_the_timeout_is_a_timeout_transport_failure() {
    use axum::Router;
    use axum::routing::post;

    // The run's wall-clock cap bounds the whole request; a gateway that
    // never answers within it fails as Transport, and the timeout survives
    // the type erasure so `is_timeout` holds.
    async fn stall() -> (axum::http::StatusCode, String) {
        std::future::pending().await
    }
    let app = Router::new().route("/v1/chat/completions", post(stall));
    let client = client_for(app).await.with_request_limits(
        Duration::from_millis(50),
        NonZeroU64::new(1024).expect("non-zero cap"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a stalled gateway must time out");
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    assert!(
        err.is_timeout(),
        "the timeout must be recognizable: {err:?}"
    );
    assert!(err.is_retryable());
}

#[tokio::test]
async fn a_body_read_timeout_keeps_its_marker_under_backend_body_read() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // The catalog fetch boxes a failed error-body read as `BackendBodyRead`
    // through the same marking as a send failure, so a timeout during that
    // read still reports `is_timeout`, as the marker's contract promises.
    // The server answers a 500 with a large promised body, then stalls.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged(mock_tag(), async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let header = "HTTP/1.1 500 Internal Server Error\r\n\
                 Content-Length: 1000000\r\n\r\nabc";
            let _ = sock.write_all(header.as_bytes()).await;
            // The stall never ends on its own: the client's read timeout
            // is what ends the test, and the runtime's teardown drops
            // the socket.
            std::future::pending::<()>().await;
        }
    });
    let response = reqwest::Client::new()
        .get(format!("http://{addr}/models"))
        .timeout(Duration::from_millis(50))
        .send()
        .await
        .expect("the headers arrive before the stall");
    let read = response
        .bytes()
        .await
        .expect_err("the body read stalls past the timeout");
    assert!(read.is_timeout(), "reqwest reports the read as a timeout");

    let err = CompletionError::from(Error::BackendBodyRead {
        status: 500,
        source: transport_source(read),
    });
    assert_eq!(err.kind(), CompletionErrorKind::Transport);
    assert_eq!(err.status(), Some(500));
    assert!(
        err.is_timeout(),
        "the marker must survive under BackendBodyRead: {err:?}"
    );
}

#[tokio::test]
async fn complete_refuses_a_malformed_stream_chunk() {
    // F14: a 200 whose stream holds an undecodable chunk is
    // MalformedResponse, and the decode failure is preserved as the
    // error-chain source.
    let base = spawn_raw_gateway(axum::http::StatusCode::OK, "data: { not json\n\n").await;
    let client = keyed_client(&base);
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("undecodable chunk must fail");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    let source =
        std::error::Error::source(&err).expect("the decode error must be a preserved source");
    assert!(
        source.downcast_ref::<serde_json::Error>().is_some(),
        "the preserved source must be the JSON decode error, got {source}"
    );
}

#[tokio::test]
async fn complete_refuses_malformed_tool_call_fragments_at_the_boundary() {
    // F14: a well-formed HTTP 200 whose streamed tool-call fragment has
    // non-string arguments is rejected at the client boundary, not passed on.
    let client = sse_client(sse_body(&[serde_json::json!({
        "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "id": "c1", "type": "function",
            "function": { "name": "t", "arguments": 123 }
        }] } }]
    })]))
    .await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("malformed tool arguments must be rejected");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
}

#[tokio::test]
async fn stream_without_done_sentinel_is_malformed() {
    // A stream cut off before [DONE] may be missing its tail; it must never
    // pass for a complete turn.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"half\"}}]}\n\n",
    )
    .await;
    let client = keyed_client(&base);
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a truncated stream must fail");
    assert_eq!(err.kind(), CompletionErrorKind::MalformedResponse);
    assert!(
        err.to_string().contains("[DONE]"),
        "the error names the missing sentinel: {err}"
    );
}
