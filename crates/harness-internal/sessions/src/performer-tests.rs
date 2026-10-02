//! The chat performer against the axum mock gateway: a streamed round's
//! deltas reach the sink in wire order, a round without a live consumer
//! sends none, and a sink nobody drains does not fail the round.

use std::num::NonZeroU32;

use harness_gateway_client::{GatewayClient, GatewayEndpoint, SecretString};
use harness_runner::performers::ChatPerformer;
use harness_runner::spawn::spawn_tagged;
use harness_runner::test_support::mock_tag;
use promptforge::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
    StreamDelta,
};
use serde_json::Value;
use tokio::sync::mpsc;

use super::GatewayChatPerformer;

/// Serves `app` on a loopback port and returns a keyed client pointed at
/// its `/v1` root.
async fn client_for(app: axum::Router) -> GatewayClient {
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
fn sse_body(events: &[Value]) -> String {
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
async fn sse_client(body: String) -> GatewayClient {
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

/// One streamed chunk with a content fragment.
fn content_chunk(text: &str) -> Value {
    serde_json::json!({
        "model": "qwen3-30b",
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }]
    })
}

/// A binding for the round; the performer runs the round under the
/// effect's frozen options, so the binding's own fields are inert here.
fn binding() -> ModelBinding {
    ModelBinding::new(
        "writer",
        "the round's model",
        ModelId::gateway("m").expect("a literal model name is valid"),
        ModelInvocation {
            temperature: None,
            max_tokens: None,
            thinking: None,
        },
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

/// A three-fragment reply closed by a stop finish.
fn three_fragments() -> String {
    sse_body(&[
        content_chunk("one "),
        content_chunk("two "),
        content_chunk("three"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ])
}

/// Every delta the sink's receiver holds, in arrival order.
fn drain(rx: &mut mpsc::UnboundedReceiver<StreamDelta>) -> Vec<StreamDelta> {
    let mut deltas = Vec::new();
    while let Ok(delta) = rx.try_recv() {
        deltas.push(delta);
    }
    deltas
}

fn reply_of(result: &CompletionResult) -> &str {
    match result {
        CompletionResult::Text(text) => text,
        other => panic!("the round replies with text: {other:?}"),
    }
}

#[tokio::test]
async fn a_streamed_round_sends_its_deltas_to_the_sink_in_wire_order() {
    let client = sse_client(three_fragments()).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            true,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(reply_of(completion.result()), "one two three");
    assert_eq!(
        drain(&mut rx),
        vec![
            StreamDelta::Text("one ".to_owned()),
            StreamDelta::Text("two ".to_owned()),
            StreamDelta::Text("three".to_owned()),
        ],
        "each fragment reaches the sink as it arrives, in the stream's order"
    );
}

#[tokio::test]
async fn a_round_without_a_live_consumer_sends_no_deltas() {
    let client = sse_client(three_fragments()).await;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            false,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(
        reply_of(completion.result()),
        "one two three",
        "the completed reply still travels in the answer"
    );
    assert!(
        drain(&mut rx).is_empty(),
        "a nested infer's fragments have no consumer and drop at the performer"
    );
}

#[tokio::test]
async fn a_sink_nobody_drains_does_not_fail_the_round() {
    let client = sse_client(three_fragments()).await;
    let (tx, rx) = mpsc::unbounded_channel::<StreamDelta>();
    drop(rx);
    let performer = GatewayChatPerformer::new(client, tx);

    let completion = performer
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            true,
        )
        .await
        .expect("a closed sink drops the deltas and the round still completes");
    assert_eq!(reply_of(completion.result()), "one two three");
}
