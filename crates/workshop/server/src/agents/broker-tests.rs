//! Workshop broker tests: with no gateway registered, or with one whose
//! key cannot build, every round and model list fails as `Unavailable`
//! with no URL or key in the message, after a replacement binding is
//! published the next round and the next model list reach the
//! replacement, and a model list waits while the menu's catalog holds no
//! chat-capable model.

use std::num::NonZeroU32;
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use harness::InferenceBroker;
use harness_gateway_client::{CompletionError, CompletionErrorKind};
use promptforge::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
};
use serde_json::json;
use workshop_gateway::{GatewayBinding, GatewayHandles, GatewayHealth};
use workshop_menu::{CatalogBus, MenuBus};
use workshop_registry::{Registration, Registry};

use super::WorkshopBroker;
use crate::app::test_helpers::spawn_gateway;

/// A registry holding gateway handles bound to `base_url` under `key`,
/// with the binding and the guard that keeps the registration alive.
fn registry_with_gateway(base_url: &str, key: &str) -> (Registry, GatewayBinding, Registration) {
    let registry = Registry::new();
    let binding = GatewayBinding::new(base_url, key).expect("the binding builds");
    let guard = workshop_gateway::register(
        &registry,
        GatewayHandles::new(binding.clone(), GatewayHealth::new()),
    );
    (registry, binding, guard)
}

/// A mock Gateway keyed with `key` that lists one model named `model` and
/// answers every round with `from <model>`; a request under another key
/// is refused.
fn keyed_gateway(model: &'static str, key: &'static str) -> Router {
    let keyed = move |headers: &HeaderMap| {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|bearer| bearer == format!("Bearer {key}"))
    };
    Router::new()
        .route(
            "/v1/models",
            get(move |headers: HeaderMap| async move {
                if !keyed(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                axum::Json(json!({ "data": [{
                    "id": model, "description": model, "context": 131_072, "thinking": "never",
                }] }))
                .into_response()
            }),
        )
        .route(
            "/v1/chat/completions",
            post(move |headers: HeaderMap| async move {
                if !keyed(&headers) {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                reply_stream(model)
            }),
        )
}

/// One round's SSE reply: `from <model>` in one content chunk, then a
/// stop.
fn reply_stream(model: &str) -> Response {
    let mut body = String::new();
    for chunk in [
        json!({ "model": model, "choices": [{
            "index": 0, "delta": { "content": format!("from {model}") }, "finish_reason": null,
        }] }),
        json!({ "model": model, "choices": [{
            "index": 0, "delta": {}, "finish_reason": "stop",
        }] }),
    ] {
        body.push_str("data: ");
        body.push_str(&chunk.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], body).into_response()
}

/// A binding for one round; the broker sends the round under its options,
/// so the binding's own fields are inert here.
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

/// Runs one round on `broker` and returns its reply text.
async fn round(broker: &WorkshopBroker) -> Result<String, CompletionError> {
    let completion = broker
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            None,
        )
        .await?;
    match completion.result() {
        CompletionResult::Text(text) => Ok(text.clone()),
        other => panic!("the round replies with text: {other:?}"),
    }
}

/// Lists the models on `broker` and returns their names.
async fn listed(broker: &WorkshopBroker) -> Result<Vec<String>, CompletionError> {
    let catalog = broker.models().await?;
    Ok(catalog
        .models()
        .iter()
        .map(|model| model.id().name().to_owned())
        .collect())
}

/// Asserts `error` is `Unavailable` with exactly the kind's fixed phrase,
/// so no URL or key rides in its message.
fn assert_unavailable(error: &CompletionError) {
    let kind = CompletionErrorKind::Unavailable;
    assert_eq!(error.kind(), kind);
    assert_eq!(
        error.to_string(),
        kind.phrase(),
        "the message is the fixed phrase alone"
    );
}

#[tokio::test]
async fn without_a_gateway_registration_rounds_and_model_lists_fail_as_unavailable() {
    let broker = WorkshopBroker::new(Registry::new());
    let error = round(&broker).await.expect_err("no gateway, no round");
    assert_unavailable(&error);
    let error = listed(&broker)
        .await
        .expect_err("no gateway, no model list");
    assert_unavailable(&error);
}

#[tokio::test]
async fn a_gateway_whose_key_cannot_build_fails_as_unavailable_naming_no_url() {
    let url = "http://127.0.0.1:9";
    let (registry, _binding, _guard) = registry_with_gateway(url, "");
    let broker = WorkshopBroker::new(registry);
    let error = round(&broker)
        .await
        .expect_err("an empty key builds no broker");
    assert_unavailable(&error);
    assert!(!error.to_string().contains(url), "{error}");
    let error = listed(&broker)
        .await
        .expect_err("an empty key lists no model");
    assert_unavailable(&error);
    assert!(!error.to_string().contains(url), "{error}");
}

#[tokio::test]
async fn after_a_replacement_the_next_round_and_model_list_reach_the_replacement() {
    let first = spawn_gateway(keyed_gateway("first-model", "first-key")).await;
    let second = spawn_gateway(keyed_gateway("second-model", "second-key")).await;
    let (registry, binding, _guard) = registry_with_gateway(&first, "first-key");
    let broker = WorkshopBroker::new(registry);
    assert_eq!(
        listed(&broker).await.expect("the first gateway lists"),
        ["first-model"]
    );
    assert_eq!(
        round(&broker).await.expect("the first gateway answers"),
        "from first-model"
    );

    binding
        .replace(&second, "second-key")
        .expect("the replacement publishes");
    assert_eq!(
        listed(&broker).await.expect("the replacement lists"),
        ["second-model"],
        "the next model list reaches the replacement under its key"
    );
    assert_eq!(
        round(&broker).await.expect("the replacement answers"),
        "from second-model",
        "the next round reaches the replacement under its key"
    );
}

#[tokio::test]
async fn a_model_list_waits_until_the_catalog_holds_a_chat_capable_model() {
    let gateway = spawn_gateway(keyed_gateway("first-model", "first-key")).await;
    let (registry, _binding, _guard) = registry_with_gateway(&gateway, "first-key");
    let catalog = CatalogBus::new();
    let menu = MenuBus::new(catalog.clone(), None);
    let _menu_guards = workshop_menu::register(&registry, &catalog, &menu);
    let broker = WorkshopBroker::new(registry);

    let mut listing = Box::pin(listed(&broker));
    catalog.publish(vec![
        json!({ "id": "whisper-base-en", "kind": "transcription" }),
    ]);
    assert!(
        tokio::time::timeout(Duration::from_millis(200), &mut listing)
            .await
            .is_err(),
        "a catalog holding no chat-capable model keeps the list waiting"
    );

    catalog.publish(vec![json!({ "id": "first-model", "kind": "chat" })]);
    let models = tokio::time::timeout(Duration::from_secs(10), listing)
        .await
        .expect("the list answers once a chat-capable model is published")
        .expect("the gateway lists");
    assert_eq!(models, ["first-model"], "the list is the gateway's own");
}
