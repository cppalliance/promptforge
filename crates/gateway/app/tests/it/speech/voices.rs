//! Catalog voices: the speech kind and voices in `GET /v1/models`, and the
//! `GET /v1/audio/voices` union across speech models.

use std::net::SocketAddr;

use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;

use super::{bytes_within, recording_speech_backend, speech_gateway};
use crate::support::{TestServer, json_within, send_within};

/// `GET /v1/models` surfaces a speech model's kind and catalog voices.
#[tokio::test]
async fn models_catalog_shows_the_speech_kind_and_voices() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/models", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let body = json_within(response).await;
    let data = body.get("data").and_then(Value::as_array).unwrap();
    let model = data
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some("tts-model"))
        .expect("the speech model is listed");
    assert_eq!(model.get("kind").and_then(Value::as_str), Some("speech"));
    assert_eq!(
        model.get("voices").and_then(Value::as_array),
        Some(&vec![Value::from("alloy"), Value::from("nova")]),
        "the catalog voices are listed: {model}"
    );
    gateway.shutdown().await;
}

/// Starts a gateway whose catalog is the given `[[model]]` TOML fragments,
/// all resolving to one fake backend. The voices route never calls an
/// upstream; the backend exists only to satisfy config validation.
async fn catalog_gateway(backend: SocketAddr, models: &str) -> TestServer {
    let toml = format!(
        r#"
config-version = 0

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
trust_loopback = false

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

{models}
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

/// `GET /v1/audio/voices` with the test bearer, returning the raw body.
async fn voices_body(gateway: &TestServer) -> (u16, String) {
    let response = send_within(
        reqwest::Client::new()
            .get(format!("http://{}/v1/audio/voices", gateway.addr))
            .bearer_auth("test-token"),
    )
    .await;
    let status = response.status().as_u16();
    let body = bytes_within(response).await;
    (
        status,
        String::from_utf8(body).expect("the voices body is UTF-8"),
    )
}

/// `GET /v1/audio/voices` answers the union of the speech models' catalog
/// voices as id-first `{"id", "name"}` entries under `{"voices": [...]}`.
/// OpenAI has no voice-list route, but the OpenAI-compatible ecosystem
/// converged on this shape and clients read the `id` key, so the entry
/// shape is a compatibility surface pinned here on the raw body.
#[tokio::test]
async fn voices_route_returns_the_id_first_union_shape() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = catalog_gateway(
        backend,
        r#"
[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["alloy"]
"#,
    )
    .await;

    let (status, body) = voices_body(&gateway).await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"voices":[{"id":"alloy","name":"alloy"}]}"#);
    gateway.shutdown().await;
}

/// The union is deduplicated and sorted across every speech model in the
/// active profile.
#[tokio::test]
async fn voices_route_deduplicates_and_sorts_across_speech_models() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = catalog_gateway(
        backend,
        r#"
[[model]]
name = "tts-one"
kind = "speech"
description = "a speech model"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["nova", "alloy"]

[[model]]
name = "tts-two"
kind = "speech"
description = "another speech model"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["shimmer", "nova"]
"#,
    )
    .await;

    let (status, body) = voices_body(&gateway).await;
    assert_eq!(status, 200);
    assert_eq!(
        body,
        r#"{"voices":[{"id":"alloy","name":"alloy"},{"id":"nova","name":"nova"},{"id":"shimmer","name":"shimmer"}]}"#
    );
    gateway.shutdown().await;
}

/// With no speech model in the active profile the union is empty.
#[tokio::test]
async fn voices_route_is_empty_without_speech_models() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = catalog_gateway(
        backend,
        r#"
[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#,
    )
    .await;

    let (status, body) = voices_body(&gateway).await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"voices":[]}"#);
    gateway.shutdown().await;
}

/// Non-speech models contribute nothing to the union.
#[tokio::test]
async fn voices_route_ignores_non_speech_models() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = catalog_gateway(
        backend,
        r#"
[[model]]
name = "chat-model"
description = "a chat model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "embed-model"
kind = "embedding"
description = "an embedding model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]

[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["nova"]
"#,
    )
    .await;

    let (status, body) = voices_body(&gateway).await;
    assert_eq!(status, 200);
    assert_eq!(body, r#"{"voices":[{"id":"nova","name":"nova"}]}"#);
    gateway.shutdown().await;
}

/// Auth runs before the union is computed: a request with no
/// Authorization header is refused 401 with the `unauthorized` envelope,
/// and no voice entry leaves the handler.
#[tokio::test]
async fn unauthenticated_voices_request_is_refused_before_the_union_is_computed() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = catalog_gateway(
        backend,
        r#"
[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["alloy"]
"#,
    )
    .await;

    let response =
        send_within(reqwest::Client::new().get(format!("http://{}/v1/audio/voices", gateway.addr)))
            .await;
    assert_eq!(response.status().as_u16(), 401);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("unauthorized")
    );
    assert!(
        body.get("voices").is_none(),
        "the union is never computed for an unauthenticated request: {body}"
    );
    gateway.shutdown().await;
}
