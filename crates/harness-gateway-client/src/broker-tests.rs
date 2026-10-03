//! The Gateway broker against the transport suite's mock gateways:
//! `chat_streaming` hands each piece to its callback in wire order, the
//! `InferenceBroker` round answers with the whole reply, and `models`
//! lists the catalog the gateway serves under the broker's key. Its
//! `Debug` never prints the bearer key.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use harness::InferenceBroker;
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::ids::RoundId;
use promptforge::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
    ThinkingMode,
};
use serde_json::Value;

use super::GatewayBroker;
use crate::StreamDelta;
use crate::config::{GatewayEndpoint, SecretString};
use crate::transport::tests::{content_chunk, serve, sse_app, sse_body};

/// The bearer key every mock gateway here is keyed with.
const KEY: &str = "tok";

/// Serves `app` on a loopback port and returns a broker keyed for its
/// `/v1` root.
async fn broker_for(app: axum::Router) -> GatewayBroker {
    GatewayBroker::new(
        GatewayEndpoint::new(&serve(app).await).expect("valid test endpoint"),
        SecretString::new(KEY).expect("non-empty test key"),
    )
}

/// A binding for the round; the broker runs the round under the effect's
/// frozen options, so the binding's own fields are inert here.
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

/// A reply that reasons once and then answers in three fragments, closed
/// by a stop finish.
fn three_fragments() -> String {
    sse_body(&[
        serde_json::json!({
            "choices": [{ "index": 0, "delta": { "reasoning_content": "hmm" }, "finish_reason": null }]
        }),
        content_chunk("one "),
        content_chunk("two "),
        content_chunk("three"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ])
}

/// The callback `chat_streaming` hands each piece to.
type OnPiece = Arc<dyn Fn(StreamDelta) + Send + Sync>;

/// A piece callback that keeps every piece it is handed, beside what it
/// kept.
fn recording() -> (OnPiece, Arc<Mutex<Vec<StreamDelta>>>) {
    let kept: Arc<Mutex<Vec<StreamDelta>>> = Arc::default();
    let sink = Arc::clone(&kept);
    (
        Arc::new(move |piece| sink.lock().unwrap().push(piece)),
        kept,
    )
}

/// The run's first round, dispatched from `origin`.
fn round(origin: ReplyOrigin) -> Round {
    Round {
        id: RoundId::new(0),
        origin,
    }
}

fn reply_of(result: &CompletionResult) -> &str {
    match result {
        CompletionResult::Text(text) => text,
        other => panic!("the round replies with text: {other:?}"),
    }
}

#[tokio::test]
async fn chat_streaming_hands_each_piece_to_its_callback_in_wire_order() {
    let broker = broker_for(sse_app(three_fragments())).await;
    let (on_piece, kept) = recording();

    let completion = broker
        .chat_streaming(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            on_piece,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(reply_of(completion.result()), "one two three");
    assert_eq!(
        *kept.lock().unwrap(),
        vec![
            StreamDelta::Reasoning("hmm".to_owned()),
            StreamDelta::Text("one ".to_owned()),
            StreamDelta::Text("two ".to_owned()),
            StreamDelta::Text("three".to_owned()),
        ],
        "each piece, reasoning and text alike, reaches the callback as it arrives, in the stream's order"
    );
}

#[tokio::test]
async fn the_inference_broker_round_answers_with_the_whole_reply() {
    let broker = broker_for(sse_app(three_fragments())).await;

    let completion = broker
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            round(ReplyOrigin::Chat),
        )
        .await
        .expect("the mock round completes");
    assert_eq!(
        reply_of(completion.result()),
        "one two three",
        "a headless Host's round streams nowhere, and the completed reply travels in the answer"
    );
}

#[test]
fn debug_never_leaks_the_bearer_key() {
    let api_root = "http://127.0.0.1:8081/v1";
    let broker = GatewayBroker::new(
        GatewayEndpoint::new(api_root).expect("valid test endpoint"),
        SecretString::new("super-secret-token").expect("non-empty test key"),
    );
    let rendered = format!("{broker:?}");
    assert!(
        !rendered.contains("super-secret-token"),
        "the bearer key must never appear in Debug output, got: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "the key field must be redacted, got: {rendered}"
    );
    assert!(
        rendered.contains(api_root),
        "the API root is not a secret and should still appear, got: {rendered}"
    );
}

#[tokio::test]
async fn models_lists_the_catalog_the_gateway_serves_under_the_brokers_key() {
    use axum::Router;
    use axum::http::header::AUTHORIZATION;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;

    async fn models(headers: HeaderMap) -> Result<axum::Json<Value>, StatusCode> {
        let bearer = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok());
        if bearer != Some("Bearer tok") {
            return Err(StatusCode::UNAUTHORIZED);
        }
        Ok(axum::Json(serde_json::json!({
            "data": [{
                "id": "mock-model",
                "description": "the mock model",
                "context": 131_072,
                "thinking": "switchable",
            }]
        })))
    }
    let broker = broker_for(Router::new().route("/v1/models", get(models))).await;

    let catalog = broker
        .models()
        .await
        .expect("the keyed mock serves its catalog");
    assert_eq!(catalog.models().len(), 1, "the one served model");
    let descriptor = catalog
        .get(&ModelId::gateway("mock-model").expect("a literal model name is valid"))
        .expect("the served model is listed under its gateway id");
    assert_eq!(descriptor.description(), "the mock model");
    assert_eq!(descriptor.context().get(), 131_072);
    assert_eq!(descriptor.thinking(), ThinkingMode::Switchable);
}
