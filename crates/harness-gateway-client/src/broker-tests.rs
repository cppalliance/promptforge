//! The Gateway broker against the transport suite's mock gateways: a
//! streamed round's deltas reach the callback in wire order, a round with
//! no callback still answers with the whole reply, and `models` lists the
//! catalog the gateway serves under the broker's key. Its `Debug` never
//! prints the bearer key.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use harness::{InferenceBroker, OnDelta};
use promptforge::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
    StreamDelta, ThinkingMode,
};
use serde_json::Value;

use super::GatewayBroker;
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

/// A delta callback that keeps every delta it is handed, beside what it
/// kept.
fn recording() -> (OnDelta, Arc<Mutex<Vec<StreamDelta>>>) {
    let kept: Arc<Mutex<Vec<StreamDelta>>> = Arc::default();
    let sink = Arc::clone(&kept);
    (
        Arc::new(move |delta| sink.lock().unwrap().push(delta)),
        kept,
    )
}

fn reply_of(result: &CompletionResult) -> &str {
    match result {
        CompletionResult::Text(text) => text,
        other => panic!("the round replies with text: {other:?}"),
    }
}

#[tokio::test]
async fn a_streamed_round_sends_its_deltas_to_the_callback_in_wire_order() {
    let broker = broker_for(sse_app(three_fragments())).await;
    let (on_delta, kept) = recording();

    let completion = broker
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            Some(on_delta),
        )
        .await
        .expect("the mock round completes");
    assert_eq!(reply_of(completion.result()), "one two three");
    assert_eq!(
        *kept.lock().unwrap(),
        vec![
            StreamDelta::Text("one ".to_owned()),
            StreamDelta::Text("two ".to_owned()),
            StreamDelta::Text("three".to_owned()),
        ],
        "each fragment reaches the callback as it arrives, in the stream's order"
    );
}

#[tokio::test]
async fn a_round_without_a_callback_still_answers_with_the_whole_reply() {
    let broker = broker_for(sse_app(three_fragments())).await;

    let completion = broker
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            None,
        )
        .await
        .expect("the mock round completes");
    assert_eq!(
        reply_of(completion.result()),
        "one two three",
        "a nested infer's fragments have no consumer, and the completed reply still travels in the answer"
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
