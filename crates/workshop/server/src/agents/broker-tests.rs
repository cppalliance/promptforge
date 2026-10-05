//! Workshop broker tests: with no gateway registered, or with one whose
//! key cannot build, every round and model list fails as `Unavailable`
//! with no URL or key in the message, after a replacement binding is
//! published the next round and the next model list reach the
//! replacement, a model list waits while the menu's catalog holds no
//! chat-capable model, a streaming round hands its pieces to its callback
//! and answers whole, a run's broker streams only its section's own
//! rounds into the conversation, and the window comparison flags a pick
//! whose context window is smaller than the round's binding's.

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::Router;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use harness::InferenceBroker;
use harness_gateway_client::{CompletionError, CompletionErrorKind, StreamDelta};
use promptforge::effect::Round;
use promptforge::event::ReplyOrigin;
use promptforge::ids::RoundId;
use promptforge::model::{
    CompletionOptions, CompletionResult, Message, ModelBinding, ModelId, ModelInvocation,
};
use serde_json::json;
use workshop_agents::{Conversations, DeltaKind};
use workshop_gateway::{GatewayBinding, GatewayHandles, GatewayHealth};
use workshop_menu::{CatalogBus, MenuBus};
use workshop_registry::{Registration, Registry};

use super::{RunBroker, WorkshopBroker, narrower_pick_window};
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
            Round {
                id: RoundId::new(0),
                origin: ReplyOrigin::Infer,
            },
        )
        .await?;
    match completion.result() {
        CompletionResult::Text(text) => Ok(text.clone()),
        other => panic!("the round replies with text: {other:?}"),
    }
}

/// Runs one streaming round on `broker` and returns its reply text beside
/// every piece its callback was handed.
async fn streamed_round(
    broker: &WorkshopBroker,
) -> (Result<String, CompletionError>, Vec<StreamDelta>) {
    let kept: Arc<Mutex<Vec<StreamDelta>>> = Arc::default();
    let sink = Arc::clone(&kept);
    let completion = broker
        .chat_streaming(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            Arc::new(move |piece| {
                sink.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(piece);
            }),
        )
        .await;
    let reply = completion.map(|completion| match completion.result() {
        CompletionResult::Text(text) => text.clone(),
        other => panic!("the round replies with text: {other:?}"),
    });
    let pieces = kept.lock().unwrap_or_else(PoisonError::into_inner).clone();
    (reply, pieces)
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
    let (reply, pieces) = streamed_round(&broker).await;
    assert_unavailable(&reply.expect_err("no gateway, no streaming round"));
    assert!(pieces.is_empty(), "a refused round streams nothing");
    let error = listed(&broker)
        .await
        .expect_err("no gateway, no model list");
    assert_unavailable(&error);
}

#[tokio::test]
async fn a_streaming_round_hands_its_pieces_to_the_callback_and_answers_whole() {
    let gateway = spawn_gateway(keyed_gateway("first-model", "first-key")).await;
    let (registry, _binding, _guard) = registry_with_gateway(&gateway, "first-key");
    let broker = WorkshopBroker::new(registry);

    let (reply, pieces) = streamed_round(&broker).await;
    assert_eq!(
        reply.expect("the gateway answers"),
        "from first-model",
        "the completed reply travels in the answer"
    );
    assert_eq!(
        pieces,
        [StreamDelta::Text("from first-model".to_owned())],
        "the reply's one content chunk reaches the callback as it arrives"
    );
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

/// Runs one round of `origin` numbered `id` on a conversation's run
/// broker and returns its reply text.
async fn run_round(broker: &RunBroker, id: u64, origin: ReplyOrigin) -> String {
    let completion = broker
        .chat(
            binding(),
            vec![Message::user("hi")],
            Vec::new(),
            CompletionOptions::new("m"),
            Round {
                id: RoundId::new(id),
                origin,
            },
        )
        .await
        .expect("the gateway answers");
    match completion.result() {
        CompletionResult::Text(text) => text.clone(),
        other => panic!("the round replies with text: {other:?}"),
    }
}

#[tokio::test]
async fn a_sections_round_streams_into_its_conversation_under_the_round_id_and_an_infer_round_does_not()
 {
    let gateway = spawn_gateway(keyed_gateway("first-model", "first-key")).await;
    let (registry, _binding, _guard) = registry_with_gateway(&gateway, "first-key");
    let conversation = Conversations::new().open("chat");
    let mut deltas = conversation.subscribe_deltas();
    let broker = RunBroker::new(WorkshopBroker::new(registry), conversation);

    assert_eq!(
        run_round(&broker, 4, ReplyOrigin::Infer).await,
        "from first-model"
    );
    assert!(
        deltas.try_recv().is_err(),
        "a nested infer round streams nothing"
    );

    assert_eq!(
        run_round(&broker, 3, ReplyOrigin::Chat).await,
        "from first-model"
    );
    let delta = deltas.try_recv().expect("the section's round streamed");
    assert_eq!(delta.kind, DeltaKind::Text);
    assert_eq!(delta.content, "from first-model");
    assert_eq!(delta.reply, 3, "the piece carries its round's id");
}

#[test]
fn the_window_comparison_flags_a_pick_smaller_than_the_binding_and_not_an_equal_or_larger_one() {
    let models = [
        json!({ "id": "smaller", "kind": "chat", "context": 4095 }),
        json!({ "id": "equal", "kind": "chat", "context": 4096 }),
        json!({ "id": "larger", "kind": "chat", "context": 131_072 }),
        json!({ "id": "unsized", "kind": "chat" }),
    ];
    let bound = NonZeroU32::new(4096).expect("4096 is non-zero");
    assert_eq!(
        narrower_pick_window(&models, "smaller", bound),
        Some(4095),
        "a pick one token short of the binding's window warns"
    );
    assert_eq!(narrower_pick_window(&models, "equal", bound), None);
    assert_eq!(narrower_pick_window(&models, "larger", bound), None);
    assert_eq!(
        narrower_pick_window(&models, "unsized", bound),
        None,
        "a pick the catalog lists without a window has nothing to compare"
    );
    assert_eq!(
        narrower_pick_window(&models, "unlisted", bound),
        None,
        "a pick the catalog does not list has nothing to compare"
    );
}
