//! The gateway client against an axum mock gateway: environment loading,
//! the bearer on the wire, the streamed round, and the bounds.

use harness_runner::spawn::spawn_tagged;
pub(crate) use harness_runner::test_support::mock_tag;
use promptforge_api_runtime::model::{ClientError as Error, CompletionOptions};
use serde_json::Value;

use super::*;

mod env;
mod limits;
mod streaming;

/// Serves `app` on a loopback port and returns a keyed client pointed at
/// its `/v1` root.
pub(crate) async fn client_for(app: axum::Router) -> GatewayClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app).await.unwrap();
    });
    GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
}

/// Renders `events` as SSE `data:` lines closed by the `[DONE]` sentinel.
pub(crate) fn sse_body(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A client pointed at a mock gateway that answers every completion with
/// the given SSE body.
pub(crate) async fn sse_client(body: String) -> GatewayClient {
    use axum::Router;
    use axum::routing::post;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let body = body.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    );
    client_for(app).await
}

/// One streamed chunk carrying a content fragment.
pub(crate) fn content_chunk(text: &str) -> Value {
    serde_json::json!({
        "model": "qwen3-30b",
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }]
    })
}

/// The minimal stop-finished stream: one content chunk and a finish chunk.
fn ok_stream() -> String {
    sse_body(&[
        content_chunk("ok"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ])
}

fn openai_options() -> CompletionOptions {
    CompletionOptions::new("m")
}

/// Spawns a gateway that answers `/v1/chat/completions` with a fixed status
/// and raw body, returning its `/v1` base.
async fn spawn_raw_gateway(status: axum::http::StatusCode, body: &'static str) -> String {
    use axum::Router;
    use axum::routing::post;
    use tokio::net::TcpListener;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move { (status, body) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged(mock_tag(), async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}

/// A keyed client pointed at the `/v1` base `base`.
fn keyed_client(base: &str) -> GatewayClient {
    GatewayClient::new(
        GatewayEndpoint::new(base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
}

fn lookup_from<'a>(
    pairs: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> Result<Option<String>, Error> + 'a {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    move |name| {
        Ok(pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone()))
    }
}
