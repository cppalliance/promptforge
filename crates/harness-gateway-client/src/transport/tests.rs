//! The gateway client against an axum mock gateway: environment loading,
//! the bearer on the wire, the streamed round, and the bounds.

use promptforge::model::CompletionOptions;
use serde_json::Value;

use super::*;

mod env;
mod limits;
mod streaming;

/// Serves `app` on a loopback port and returns its `/v1` root.
pub(crate) async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}

/// Serves `app` on a loopback port and returns a keyed client pointed at
/// its `/v1` root.
async fn client_for(app: axum::Router) -> GatewayChat {
    GatewayChat::new(
        GatewayEndpoint::new(&serve(app).await).expect("valid test endpoint"),
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

/// A mock gateway that answers every completion with the given SSE body.
pub(crate) fn sse_app(body: String) -> axum::Router {
    use axum::Router;
    use axum::routing::post;

    Router::new().route(
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
    )
}

/// A client pointed at a mock gateway that answers every completion with
/// the given SSE body.
async fn sse_client(body: String) -> GatewayChat {
    client_for(sse_app(body)).await
}

/// One streamed chunk with a content fragment.
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
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}

/// A keyed client pointed at the `/v1` base `base`.
fn keyed_client(base: &str) -> GatewayChat {
    GatewayChat::new(
        GatewayEndpoint::new(base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
}

fn lookup_from<'a>(
    pairs: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> Result<Option<String>, GatewayConfigError> + 'a {
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
