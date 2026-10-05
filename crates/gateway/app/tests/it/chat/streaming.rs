//! Streaming chat completions: typed SSE chunks, the permit held until the
//! stream ends, client disconnect, and an upstream error before the stream.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use futures_util::StreamExt as _;
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use super::text_within;
use crate::support::{
    PHASE_TIMEOUT, ReleaseTx, canned_reply, gateway_for, gateway_with_queue, join_within,
    json_within, next_arrival, send_within, spawn_backend, spawn_chat,
};

/// One SSE `data:` line containing an OpenAI streaming chunk.
fn sse_line(model: &str, content: &str) -> String {
    format!(
        "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n"
    )
}

/// The mock upstream emits three chunks; the client receives three SSE data
/// lines (each a validated chunk that names the caller's model) plus the
/// terminal `data: [DONE]`.
#[tokio::test]
async fn stream_true_relays_typed_chunks_and_done() {
    let body = ["Hel", "lo", "!"]
        .into_iter()
        .map(|part| sse_line("backend-model", part))
        .collect::<String>()
        + "data: [DONE]\n\n";
    let backend = spawn_backend(Router::new().route(
        "/chat/completions",
        post(move || {
            let body = body.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    ))
    .await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream")
    );

    let body = text_within(response).await;
    let data_lines: Vec<&str> = body
        .lines()
        .filter(|line| line.starts_with("data:"))
        .collect();
    assert_eq!(data_lines.len(), 4, "three chunks plus [DONE]: {body}");
    assert_eq!(data_lines[3], "data: [DONE]");
    let mut text = String::new();
    for line in &data_lines[..3] {
        let chunk: Value = serde_json::from_str(line.strip_prefix("data: ").unwrap()).unwrap();
        // The relay re-serializes each chunk with the caller's model name.
        assert_eq!(
            chunk.get("model").and_then(Value::as_str),
            Some("test-model")
        );
        text.push_str(
            chunk
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .unwrap(),
        );
    }
    assert_eq!(text, "Hello!");
    gateway.shutdown().await;
}

/// A backend that gates each request on a release handle: streaming requests
/// get one chunk, then block mid-stream until released; non-streaming
/// requests block, then return the canned reply.
async fn completions_gated(
    State(arrivals): State<UnboundedSender<ReleaseTx>>,
    Json(body): Json<Value>,
) -> Response {
    let (release, released) = oneshot::channel();
    let _ = arrivals.send(release);
    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if body.get("stream").and_then(Value::as_bool) == Some(true) {
        let first = futures_util::stream::once({
            let model = model.clone();
            async move { Ok::<_, std::convert::Infallible>(sse_line(&model, "po")) }
        });
        let rest = futures_util::stream::once(async move {
            let _ = released.await;
            Ok(format!("{}data: [DONE]\n\n", sse_line(&model, "ng")))
        });
        let mut response = Response::new(axum::body::Body::from_stream(first.chain(rest)));
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("text/event-stream"),
        );
        response
    } else {
        let _ = released.await;
        Json(canned_reply(&model)).into_response()
    }
}

async fn gated_sse_backend() -> (std::net::SocketAddr, UnboundedReceiver<ReleaseTx>) {
    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/chat/completions", post(completions_gated))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// Under concurrency=1, a streaming request holds the dominion queue permit
/// for the stream's whole lifetime: a second request is not admitted until
/// the first stream has ended.
#[tokio::test]
async fn stream_permit_is_held_until_the_stream_ends() {
    let (backend, mut arrivals) = gated_sse_backend().await;
    let gateway = gateway_with_queue(backend, 1, 10).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/chat/completions", gateway.addr);

    let first = {
        let client = client.clone();
        let url = url.clone();
        tokio::spawn(async move {
            client
                .post(url)
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": "test-model",
                    "messages": [{ "role": "user", "content": "ping" }],
                    "stream": true
                }))
                .send()
                .await
        })
    };
    let release_first = next_arrival(&mut arrivals).await;

    // The second request cannot be admitted while the first stream holds the
    // only concurrency slot.
    let second = spawn_chat(&client, &url);
    assert!(
        matches!(arrivals.try_recv(), Err(mpsc::error::TryRecvError::Empty)),
        "second request must not reach the backend while the stream is open"
    );

    let first_response = join_within(first).await.unwrap();
    assert_eq!(first_response.status().as_u16(), 200);
    release_first.send(()).unwrap();
    // Drain the body so the relay finishes and releases the permit.
    let body = text_within(first_response).await;
    assert!(body.contains("data: [DONE]"), "stream completed: {body}");

    // After the stream ends, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    assert_eq!(join_within(second).await.unwrap().status().as_u16(), 200);
    gateway.shutdown().await;
}

/// A client disconnect mid-stream cancels the upstream stream: dropping the
/// response body drops the relay, which drops the gateway's upstream
/// connection, which the backend observes as its own response body being
/// dropped. Drop is the entire mechanism - there is no explicit cancel path.
#[tokio::test]
async fn client_disconnect_aborts_the_upstream_stream() {
    /// Signals once the backend's response body is dropped mid-stream.
    struct NotifyOnDrop(UnboundedSender<()>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    let (dropped, mut observed) = mpsc::unbounded_channel::<()>();
    let backend = spawn_backend(Router::new().route(
        "/chat/completions",
        post(move || {
            let dropped = dropped.clone();
            async move {
                let first = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(sse_line("backend-model", "po"))
                });
                let rest = futures_util::stream::once(async move {
                    let _notify = NotifyOnDrop(dropped);
                    futures_util::future::pending::<()>().await;
                    unreachable!("the stream never yields a second chunk")
                });
                let mut response = Response::new(axum::body::Body::from_stream(first.chain(rest)));
                response.headers_mut().insert(
                    axum::http::header::CONTENT_TYPE,
                    axum::http::HeaderValue::from_static("text/event-stream"),
                );
                response
            }
        }),
    ))
    .await;
    let gateway = gateway_for(backend).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    // Read the first chunk so the stream is genuinely mid-flight, then hang up.
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "first chunk arrived");
    drop(response);

    tokio::time::timeout(PHASE_TIMEOUT, observed.recv())
        .await
        .expect("backend did not observe the disconnect within the phase timeout")
        .expect("disconnect notification channel closed");
    gateway.shutdown().await;
}

/// An upstream 500 before the stream starts is consumed as a normal JSON
/// error, never an SSE stream that dies mid-flight.
#[tokio::test]
async fn stream_true_upstream_500_is_a_json_error_not_sse() {
    async fn completions() -> (StatusCode, Json<Value>) {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": "backend exploded" })),
        )
    }
    let backend = spawn_backend(Router::new().route("/chat/completions", post(completions))).await;
    let gateway = gateway_for(backend).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/chat/completions", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "test-model",
                "messages": [{ "role": "user", "content": "ping" }],
                "stream": true
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 502);
    assert_ne!(
        response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "a pre-stream failure is never an SSE response"
    );
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_error")
    );
    gateway.shutdown().await;
}
