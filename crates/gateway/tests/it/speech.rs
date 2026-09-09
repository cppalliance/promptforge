//! Speech route: remote passthrough of opaque audio bytes, voice validation,
//! dominion queue admission, the kind guard, the upstream-error envelope
//! mapping, and the bounded background relay's terminal paths (byte ceiling,
//! total lifetime, upstream idle, blocked downstream, cancellation), each
//! proving the permit goes back by the admission of a later request.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::post;
use futures_util::StreamExt as _;
use gateway::{Config, Gateway, ProfileName, ProfilesContext};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::support::{
    PHASE_TIMEOUT, RecordedRequest, Recorder, ReleaseTx, TestServer, join_within, json_within,
    next_arrival, parse_sse, send_within, spawn_backend, text_within,
};

/// Canned audio bytes with non-UTF8 content, so a text-handling mistake on
/// the passthrough path shows up as a byte difference.
const CANNED_AUDIO: &[u8] = b"\xff\xfb\x90\x00ID3 fake mp3 frames \x00\x01\x02 stream";

fn speech_body() -> Value {
    serde_json::json!({
        "model": "tts-model",
        "input": "hello <laugh> from the gateway",
        "voice": "alloy"
    })
}

fn spawn_speech(
    client: &reqwest::Client,
    url: &str,
) -> tokio::task::JoinHandle<reqwest::Result<reqwest::Response>> {
    let client = client.clone();
    let url = url.to_string();
    tokio::spawn(async move {
        client
            .post(url)
            .bearer_auth("test-token")
            .json(&speech_body())
            .send()
            .await
    })
}

/// Reads a full binary body bounded by [`PHASE_TIMEOUT`] (IT-003).
async fn bytes_within(response: reqwest::Response) -> Vec<u8> {
    tokio::time::timeout(PHASE_TIMEOUT, response.bytes())
        .await
        .expect("HTTP body read exceeded the phase timeout")
        .expect("HTTP body read failed")
        .to_vec()
}

/// A fake speech backend that records each request, then replies 200 with
/// the canned audio bytes and the given `Content-Type` (or none at all, so
/// the route's format-to-MIME fallback is exercised).
async fn recording_speech_backend(content_type: Option<&'static str>) -> (SocketAddr, Recorder) {
    async fn speech(
        State((recorder, content_type)): State<(Recorder, Option<&'static str>)>,
        method: Method,
        uri: axum::http::Uri,
        headers: HeaderMap,
        body: Bytes,
    ) -> Response {
        let authorization = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = serde_json::from_slice(&body).unwrap_or(Value::Null);
        recorder.lock().unwrap().push(RecordedRequest {
            method: method.to_string(),
            path: uri.path().to_string(),
            authorization,
            body,
        });
        let mut response = Response::new(Body::from(CANNED_AUDIO));
        if let Some(content_type) = content_type {
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        }
        response
    }

    let recorder: Recorder = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((Arc::clone(&recorder), content_type));
    (spawn_backend(router).await, recorder)
}

/// A fake speech backend that hands the test a release handle on each
/// arrival: the first audio chunk flows immediately and the second waits
/// for the handle, so a test can hold the stream open mid-flight. No
/// sleeps: arrival and release are rendezvous.
async fn gated_audio_backend() -> (SocketAddr, UnboundedReceiver<ReleaseTx>) {
    async fn speech(State(arrivals): State<UnboundedSender<ReleaseTx>>) -> Response {
        let (release, released) = oneshot::channel();
        let _ = arrivals.send(release);
        let first = futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-chunk-1;"))
        });
        let rest = futures_util::stream::once(async move {
            let _ = released.await;
            Ok(Bytes::from_static(b"audio-chunk-2;"))
        });
        let mut response = Response::new(Body::from_stream(first.chain(rest)));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<ReleaseTx>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend that answers every request with the given status
/// and body, for the upstream-error envelope tests.
async fn status_speech_backend(status: StatusCode, body: &'static str) -> SocketAddr {
    async fn speech(
        State((status, body)): State<(StatusCode, &'static str)>,
    ) -> (StatusCode, &'static str) {
        (status, body)
    }
    spawn_backend(
        Router::new()
            .route("/audio/speech", post(speech))
            .with_state((status, body)),
    )
    .await
}

/// Start a gateway serving one remote speech model. `voices` renders the
/// catalog list (`Some(&[])` renders an explicit empty list, `None` omits
/// the field). With `pool`, the endpoint binds to a dominion capped at that
/// many in-flight requests with the given waiting depth and policy;
/// without it the endpoint is an unlimited pass-through.
async fn speech_gateway(
    backend: SocketAddr,
    voices: Option<&[&str]>,
    pool: Option<(usize, usize, &str)>,
) -> TestServer {
    let voices = voices.map_or_else(String::new, |list| {
        let list = list
            .iter()
            .map(|voice| format!("\"{voice}\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("voices = [{list}]")
    });
    let (dominion, binding) = match pool {
        Some((concurrency, depth, policy)) => (
            format!(
                r#"
[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = {concurrency}
max_queue = {depth}
policy = "{policy}"
"#
            ),
            "\ndominion = \"pool\"",
        ),
        None => (String::new(), ""),
    };
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
trust_loopback = false
{dominion}
[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""{binding}

[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model for integration"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
{voices}
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    TestServer::start(gateway).await
}

/// IT-005/006 for the speech route: the backend records the request, so we
/// assert exactly what the gateway forwarded - method, path, the rewritten
/// upstream model, the intact input and voice, and the structural mp3 pin
/// reaching the provider when the client omits `response_format` - and that
/// the client's bearer is not leaked upstream. The response body is the
/// upstream's bytes, unchanged, under the upstream's own `Content-Type`.
#[tokio::test]
async fn remote_passthrough_streams_audio_bytes_unchanged() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/mpeg"),
        "the upstream content type is forwarded"
    );
    assert!(
        response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .is_none(),
        "a streamed audio body never carries Content-Length"
    );
    let body = bytes_within(response).await;
    assert_eq!(body, CANNED_AUDIO, "audio bytes pass through unchanged");

    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.path, "/audio/speech");
    assert_eq!(
        request.body.get("model").and_then(Value::as_str),
        Some("backend-tts"),
        "public model name rewritten to the upstream alias"
    );
    assert_eq!(
        request.body.get("input").and_then(Value::as_str),
        Some("hello <laugh> from the gateway"),
        "input forwarded intact, emotion tag and all: angle-bracket markup is never sanitized"
    );
    assert_eq!(
        request.body.get("voice").and_then(Value::as_str),
        Some("alloy"),
        "voice forwarded intact"
    );
    assert_eq!(
        request.body.get("response_format").and_then(Value::as_str),
        Some("mp3"),
        "the structural mp3 pin reaches the provider when the field is omitted"
    );
    assert_ne!(
        request.authorization.as_deref(),
        Some("Bearer test-token"),
        "caller bearer must not leak to the upstream"
    );
    gateway.shutdown().await;
}

/// When the upstream omits `Content-Type`, the route falls back to the
/// requested format's MIME type (the OpenAI spellings).
#[tokio::test]
async fn content_type_falls_back_to_the_format_mime_mapping() {
    let (backend, _recorder) = recording_speech_backend(None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;
    let client = reqwest::Client::new();
    for (format, mime) in [
        ("mp3", "audio/mpeg"),
        ("opus", "audio/ogg"),
        ("aac", "audio/aac"),
        ("flac", "audio/flac"),
        ("wav", "audio/wav"),
        ("pcm", "audio/pcm"),
    ] {
        let response = send_within(
            client
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": "tts-model",
                    "input": "format check",
                    "voice": "alloy",
                    "response_format": format,
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 200, "format {format}");
        assert_eq!(
            response
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some(mime),
            "format {format} falls back to its MIME type"
        );
    }
    gateway.shutdown().await;
}

/// A model configured for a non-speech kind is rejected on the speech route
/// with 400 and `kind_mismatch` before any queue admission or upstream call.
#[tokio::test]
async fn non_speech_kinds_are_rejected_on_the_speech_route() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let toml = format!(
        r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""

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
name = "reranker"
kind = "classifier"
description = "a classifier model"
context = 8192
upstream = "backend-model"
endpoints = ["fake"]
"#
    );
    let config = Config::from_toml_str(&toml).unwrap();
    let gateway = Gateway::from_config(&config, ProfilesContext::default()).unwrap();
    let gateway = TestServer::start(gateway).await;

    for model in ["chat-model", "embed-model", "reranker"] {
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&serde_json::json!({
                    "model": model,
                    "input": "say something",
                    "voice": "alloy"
                })),
        )
        .await;
        assert_eq!(response.status().as_u16(), 400, "model {model}");
        let body = json_within(response).await;
        assert_eq!(
            body.pointer("/error/code").and_then(Value::as_str),
            Some("kind_mismatch"),
            "model {model}"
        );
    }
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a kind-mismatched request must never reach the backend"
    );
    gateway.shutdown().await;
}

/// A voice outside the model's catalog list is rejected with 400
/// `invalid_voice` naming the valid voices, before any upstream call.
#[tokio::test]
async fn unknown_voice_is_rejected_naming_the_valid_voices() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "coral"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 400);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_voice")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("invalid_request_error")
    );
    let message = body
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap();
    assert!(
        message.contains("alloy") && message.contains("nova"),
        "the error names the valid voices: {message}"
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "a voice rejection never reaches the backend"
    );
    gateway.shutdown().await;
}

/// The voice check runs before dominion queue admission: with the only
/// concurrency slot held and the pool on the fail-fast `reject` policy, a
/// bad voice still earns 400 rather than the pool's 429, while a valid
/// voice earns the 429 - proving the pool really was full.
#[tokio::test]
async fn voice_validation_precedes_queue_admission() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), Some((1, 100, "reject"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    let bad_voice = send_within(client.post(&url).bearer_auth("test-token").json(
        &serde_json::json!({
            "model": "tts-model",
            "input": "say something",
            "voice": "coral"
        }),
    ))
    .await;
    assert_eq!(
        bad_voice.status().as_u16(),
        400,
        "voice validation fires before admission, so a full pool cannot turn it into a 429"
    );
    let body = json_within(bad_voice).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("invalid_voice")
    );

    let good_voice = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(
        good_voice.status().as_u16(),
        429,
        "the pool really was full: a valid request is rejected at admission"
    );
    let body = json_within(good_voice).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_rejected")
    );

    release_first.send(()).unwrap();
    let first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    assert_eq!(bytes_within(first).await, b"audio-chunk-1;audio-chunk-2;");
    gateway.shutdown().await;
}

/// A model with an empty catalog `voices` list exposes no fixed voice set:
/// any voice name passes the route's check and is forwarded verbatim.
#[tokio::test]
async fn an_empty_voices_list_skips_the_voice_check() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&[]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "anything-goes"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("voice").and_then(Value::as_str),
        Some("anything-goes"),
        "the unchecked voice is forwarded verbatim"
    );
    gateway.shutdown().await;
}

/// The OpenAI object voice form (`{"id": ...}`) is validated by its `id`
/// against the catalog list and forwarded verbatim.
#[tokio::test]
async fn voice_object_form_is_validated_by_id_and_forwarded() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy", "nova"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": { "id": "nova" }
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("voice"),
        Some(&serde_json::json!({ "id": "nova" })),
        "the object form is forwarded unchanged"
    );
    gateway.shutdown().await;
}

/// The speech handler admits through the model's dominion queue exactly
/// like chat: with one in-flight slot and one waiting slot, the third
/// request is 503 `queue_full`.
#[tokio::test]
async fn queue_full_returns_503_when_waiting_slots_exhausted() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 1, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // Exactly one of these acquires the single waiting slot; the other is 503.
    // The race is bounded by PHASE_TIMEOUT (IT-003): if admission broke so
    // neither request completes, the test fails instead of hanging.
    let mut second = spawn_speech(&client, &url);
    let mut third = spawn_speech(&client, &url);
    let (rejected, survivor) = tokio::time::timeout(PHASE_TIMEOUT, async {
        tokio::select! {
            r = &mut second => (r, third),
            r = &mut third => (r, second),
        }
    })
    .await
    .expect("neither queued request completed within the phase timeout");
    let rejected = rejected.unwrap().unwrap();
    assert_eq!(rejected.status().as_u16(), 503);
    let body = json_within(rejected).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("queue_full")
    );

    release_first.send(()).unwrap();
    let first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let _ = bytes_within(first).await;

    let release_survivor = next_arrival(&mut arrivals).await;
    release_survivor.send(()).unwrap();
    let survivor = join_within(survivor).await.unwrap();
    assert_eq!(survivor.status().as_u16(), 200);
    let _ = bytes_within(survivor).await;
    gateway.shutdown().await;
}

/// Bounded negative wait for a request that must not be admitted: long
/// enough that a released slot would deterministically let the request
/// reach the backend over loopback, and comfortably under the relay's
/// scaled 200 ms upstream-idle budget so the held stream's relay cannot
/// trip idle and free the slot mid-wait (the same ceiling [`DRIP_GAP`]
/// stays under).
const ADMISSION_GRACE: Duration = Duration::from_millis(100);

/// Under concurrency=1, a speech request holds the dominion queue permit
/// for the audio stream's whole lifetime: a second request is not admitted
/// until the first stream has ended.
#[tokio::test]
async fn stream_permit_is_held_until_the_audio_stream_ends() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    let release_first = next_arrival(&mut arrivals).await;

    // The second request cannot be admitted while the first stream holds the
    // only concurrency slot: a bounded negative wait proves no arrival, where
    // a bare try_recv would pass vacuously on a current-thread runtime that
    // never polled the spawned request.
    let second = spawn_speech(&client, &url);
    let arrived = tokio::time::timeout(ADMISSION_GRACE, arrivals.recv()).await;
    assert!(
        arrived.is_err(),
        "second request must not reach the backend while the stream is open"
    );

    let first_response = join_within(first).await.unwrap();
    assert_eq!(first_response.status().as_u16(), 200);
    release_first.send(()).unwrap();
    // Drain the body so the relay finishes and releases the permit.
    let body = bytes_within(first_response).await;
    assert_eq!(body, b"audio-chunk-1;audio-chunk-2;", "stream completed");

    // After the stream ends, the second is admitted and reaches the backend.
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    let second = join_within(second).await.unwrap();
    assert_eq!(second.status().as_u16(), 200);
    let _ = bytes_within(second).await;
    gateway.shutdown().await;
}

/// A client disconnect mid-stream cancels the upstream stream: dropping the
/// response body drops the relay, which drops the gateway's upstream
/// connection, which the backend observes as its own response body being
/// dropped. Drop is the entire mechanism - there is no explicit cancel
/// path. The released permit admits a later request under concurrency=1.
#[tokio::test]
async fn client_disconnect_aborts_the_upstream_stream_and_releases_the_permit() {
    /// Signals once the backend's response body is dropped mid-stream.
    struct NotifyOnDrop(UnboundedSender<()>);
    impl Drop for NotifyOnDrop {
        fn drop(&mut self) {
            let _ = self.0.send(());
        }
    }

    let (dropped, mut observed) = mpsc::unbounded_channel::<()>();
    let backend = spawn_backend(Router::new().route(
        "/audio/speech",
        post(move || {
            let dropped = dropped.clone();
            async move {
                let first = futures_util::stream::once(async {
                    Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-chunk-1;"))
                });
                let rest = futures_util::stream::once(async move {
                    let _notify = NotifyOnDrop(dropped);
                    futures_util::future::pending::<()>().await;
                    unreachable!("the stream never yields a second chunk")
                });
                let mut response = Response::new(Body::from_stream(first.chain(rest)));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
                response
            }
        }),
    ))
    .await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let mut response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
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

    // The permit went back: a second request is admitted under concurrency=1.
    let mut second = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(second.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, second.chunk())
        .await
        .expect("second chunk read exceeded the phase timeout")
        .expect("second chunk read failed");
    assert!(
        chunk.is_some(),
        "the second request was admitted and answered after the disconnect released the permit"
    );
    drop(second);
    gateway.shutdown().await;
}

/// A mid-stream upstream failure after HTTP 200 cannot become an error
/// envelope - audio bytes already flowed - so the relay propagates the
/// failure and the client's body read fails on the truncation; the bytes
/// that did arrive are pure audio with no JSON spliced in.
#[tokio::test]
async fn mid_stream_upstream_error_fails_the_body_read_without_an_envelope() {
    const AUDIO_PREFIX: &[u8] = b"audio-so-far;";

    // The backend sends one chunk, then waits for the test to trigger the
    // failure, so the error is guaranteed to land after the client holds a
    // 200 and real audio bytes: a rendezvous, not a race.
    let (fail, wait_fail) = oneshot::channel::<()>();
    let wait_fail = Arc::new(Mutex::new(Some(wait_fail)));
    let backend = spawn_backend(Router::new().route(
        "/audio/speech",
        post(move || {
            let wait_fail = Arc::clone(&wait_fail);
            async move {
                let wait = wait_fail
                    .lock()
                    .unwrap()
                    .take()
                    .expect("the test sends one request");
                let first = futures_util::stream::once(async {
                    Ok::<_, std::io::Error>(Bytes::from_static(AUDIO_PREFIX))
                });
                let rest = futures_util::stream::once(async move {
                    let _ = wait.await;
                    Err(std::io::Error::other("upstream died mid-stream"))
                });
                let mut response = Response::new(Body::from_stream(first.chain(rest)));
                response
                    .headers_mut()
                    .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
                response
            }
        }),
    ))
    .await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let mut response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(
        response.status().as_u16(),
        200,
        "the failure is mid-stream, after the 200"
    );
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed")
        .expect("the first audio chunk arrived");
    let mut received = first.to_vec();
    fail.send(()).expect("the backend is waiting on the signal");
    let failed = loop {
        match tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
            .await
            .expect("body read exceeded the phase timeout")
        {
            Ok(Some(chunk)) => received.extend_from_slice(&chunk),
            Ok(None) => break false,
            Err(_) => break true,
        }
    };
    assert!(
        failed,
        "the client's body read fails on a mid-stream upstream error"
    );
    assert_eq!(
        received, AUDIO_PREFIX,
        "only audio bytes arrived; no JSON envelope was spliced into the stream"
    );
    gateway.shutdown().await;
}

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

/// An upstream 429 maps to the speech-only 429 envelope
/// (`rate_limit_error` / `upstream_rate_limited`), so an OpenAI client sees
/// a retryable rate-limit error rather than a server failure.
#[tokio::test]
async fn upstream_429_maps_to_rate_limited() {
    let backend = status_speech_backend(StatusCode::TOO_MANY_REQUESTS, "provider throttled").await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 429);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_rate_limited")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("rate_limit_error")
    );
    gateway.shutdown().await;
}

/// An upstream 503 maps to the speech-only 503 envelope
/// (`server_error` / `upstream_unavailable`) rather than the shared
/// mapping's 502.
#[tokio::test]
async fn upstream_503_maps_to_unavailable() {
    let backend = status_speech_backend(StatusCode::SERVICE_UNAVAILABLE, "provider down").await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 503);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("upstream_unavailable")
    );
    assert_eq!(
        body.pointer("/error/type").and_then(Value::as_str),
        Some("server_error")
    );
    gateway.shutdown().await;
}

/// An upstream error body carries provider internals - stack text, internal
/// hosts, request ids - and none of it may reach the client: the envelope
/// message is the gateway's own fixed string on both the speech-only
/// variants and the shared protocol arm.
#[tokio::test]
async fn upstream_error_bodies_never_reach_the_client() {
    const INTERNALS: &str = "java.lang.IllegalStateException: voice clone failed\n\
                             at com.acme.tts.Synth.speak(Synth.java:412)\n\
                             host http://tts-internal.acme.corp:9090\n\
                             x-request-id req-01HZX8AEFGH";
    for (status, expected) in [
        (StatusCode::TOO_MANY_REQUESTS, 429),
        (StatusCode::INTERNAL_SERVER_ERROR, 502),
    ] {
        let backend = status_speech_backend(status, INTERNALS).await;
        let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;
        let response = send_within(
            reqwest::Client::new()
                .post(format!("http://{}/v1/audio/speech", gateway.addr))
                .bearer_auth("test-token")
                .json(&speech_body()),
        )
        .await;
        assert_eq!(response.status().as_u16(), expected, "status {status}");
        let body = tokio::time::timeout(PHASE_TIMEOUT, response.text())
            .await
            .expect("body read exceeded the phase timeout")
            .expect("body read failed");
        for marker in ["java.lang", "tts-internal.acme.corp", "req-01HZX8AEFGH"] {
            assert!(
                !body.contains(marker),
                "provider internals must not leak ({marker}) into: {body}"
            );
        }
        gateway.shutdown().await;
    }
}

/// Auth runs before the body is parsed: an unauthenticated request with a
/// malformed JSON body is refused 401, and nothing reaches the backend.
#[tokio::test]
async fn unauthenticated_speech_is_refused_before_the_body_is_parsed() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body("{not json"),
    )
    .await;
    assert_eq!(response.status().as_u16(), 401);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("unauthorized")
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "an unauthenticated request never reaches the backend"
    );
    gateway.shutdown().await;
}

/// An unknown model on the speech route is a 404 `model_not_found`
/// envelope, exactly as on the chat route, and never reaches the backend.
#[tokio::test]
async fn unknown_model_on_the_speech_route_returns_model_not_found() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "ghost",
                "input": "say something",
                "voice": "alloy"
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 404);
    let body = json_within(response).await;
    assert_eq!(
        body.pointer("/error/code").and_then(Value::as_str),
        Some("model_not_found")
    );
    assert!(
        recorder.lock().unwrap().is_empty(),
        "an unknown model never reaches the backend"
    );
    gateway.shutdown().await;
}

/// `stream_format` rides the verbatim passthrough: the provider sees the
/// framing selector exactly as the client sent it.
#[tokio::test]
async fn stream_format_sse_is_forwarded_to_the_upstream_body() {
    let (backend, recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let _ = bytes_within(response).await;
    let seen = recorder.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "backend saw exactly one request");
    assert_eq!(
        seen[0].body.get("stream_format").and_then(Value::as_str),
        Some("sse"),
        "the framing selector is forwarded into the outbound upstream body"
    );
    gateway.shutdown().await;
}

/// With `stream_format = "sse"` the fallback media type follows the framing
/// selector: an upstream that omits `Content-Type` is labeled
/// `text/event-stream`, never an audio type.
#[tokio::test]
async fn sse_framing_falls_back_to_event_stream_when_content_type_is_missing() {
    let (backend, _recorder) = recording_speech_backend(None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("text/event-stream"),
        "the fallback takes the framing selector, never an audio type"
    );
    gateway.shutdown().await;
}

/// A present upstream `Content-Type` is forwarded verbatim even with
/// `stream_format = "sse"`: the framing selector drives only the fallback.
#[tokio::test]
async fn sse_framing_still_forwards_a_present_upstream_content_type() {
    let (backend, _recorder) = recording_speech_backend(Some("audio/mpeg")).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&serde_json::json!({
                "model": "tts-model",
                "input": "say something",
                "voice": "alloy",
                "stream_format": "sse",
            })),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("audio/mpeg"),
        "a present upstream content type wins over the framing fallback"
    );
    gateway.shutdown().await;
}

// Bounded-relay boundary tests. The gateway builds this suite with its
// `test-fixtures` feature (the crate dev-depends on itself with it), which
// scales the relay's bounds down: byte ceiling 16 MiB, upstream read idle
// 200 ms, blocked downstream delivery 400 ms, total stream lifetime 2 s, and
// the profile-switch drain deadline 1 s. These mirrors name the same numbers
// so the boundary tests run in milliseconds; the relay's own constants are
// the source of truth.

/// The relay's test-scaled response byte ceiling.
const RELAY_BYTE_CEILING: u64 = 16 * 1024 * 1024;
/// A drip gap comfortably under the scaled 200 ms upstream-idle budget.
const DRIP_GAP: Duration = Duration::from_millis(60);
/// A header delay past the scaled idle budget but far under the
/// first-response budget: time-to-headers is never the relay's business.
const HEADER_DELAY: Duration = Duration::from_millis(300);

/// Waits for a backend arrival ping, bounded by [`PHASE_TIMEOUT`].
async fn next_ping(arrivals: &mut UnboundedReceiver<()>) {
    tokio::time::timeout(PHASE_TIMEOUT, arrivals.recv())
        .await
        .expect("timed out waiting for backend arrival")
        .expect("arrivals channel closed");
}

/// Reads a response body to its end or its failure, returning the bytes
/// that arrived and whether the read failed. A relay terminal path surfaces
/// over HTTP only as a failed read (never a clean EOF): hyper aborts the
/// response on a body-stream error, so the terminal item's message stays
/// server-side and the tests discriminate the bounds by stream shape and
/// timing instead. The whole read is bounded by [`PHASE_TIMEOUT`], so a
/// stream that never ends and never fails is a test failure, never a hang.
async fn read_to_end_or_error(response: reqwest::Response) -> (Vec<u8>, bool) {
    let mut response = response;
    let mut received = Vec::new();
    let deadline = tokio::time::Instant::now() + PHASE_TIMEOUT;
    loop {
        let item = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .expect("HTTP body read exceeded the phase timeout");
        match item {
            Ok(Some(chunk)) => received.extend_from_slice(&chunk),
            Ok(None) => return (received, false),
            Err(_) => return (received, true),
        }
    }
}

/// Proves the first stream's permit went back: under a one-slot dominion a
/// second request is admitted only once the relay holding the slot ended,
/// so its arrival at the backend is the release proof. The admitted request
/// is answered and then dropped mid-stream.
async fn assert_permit_released_by_admission(
    client: &reqwest::Client,
    url: &str,
    arrivals: &mut UnboundedReceiver<()>,
) {
    let second = spawn_speech(client, url);
    next_ping(arrivals).await;
    let second = join_within(second)
        .await
        .expect("the second request sends once the permit is free");
    assert_eq!(
        second.status().as_u16(),
        200,
        "a later request is admitted once the ended relay released the permit"
    );
    drop(second);
}

/// A fake speech backend streaming exactly `total` bytes in 64 KiB chunks,
/// pinging the arrivals channel per request so a test can prove a later
/// request was admitted after the first stream's relay ended.
async fn fixed_size_audio_backend(total: u64) -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State((total, arrivals)): State<(u64, UnboundedSender<()>)>) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold(0_u64, move |sent| async move {
            let remaining = total - sent;
            if remaining == 0 {
                return None;
            }
            let len = usize::try_from(remaining.min(64 * 1024)).unwrap();
            let chunk = Bytes::from(vec![0xAB; len]);
            Some((
                Ok::<_, std::convert::Infallible>(chunk),
                sent + u64::try_from(len).unwrap(),
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((total, arrivals));
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend dripping one small chunk every `gap`: `Some(n)`
/// chunks then a clean end, or `None` chunks forever. Arrivals are
/// signalled per request.
async fn dripping_audio_backend(
    gap: Duration,
    chunks: Option<usize>,
) -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(
        State((gap, chunks, arrivals)): State<(Duration, Option<usize>, UnboundedSender<()>)>,
    ) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold(chunks, move |remaining| async move {
            if remaining == Some(0) {
                return None;
            }
            tokio::time::sleep(gap).await;
            let remaining = remaining.map(|left| left - 1);
            Some((
                Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-drip;")),
                remaining,
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((gap, chunks, arrivals));
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend that sends one audio chunk and then pends forever,
/// so the relay's upstream-idle budget is the only thing that can end the
/// stream. Arrivals are signalled per request.
async fn stalling_audio_backend() -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State(arrivals): State<UnboundedSender<()>>) -> Response {
        let _ = arrivals.send(());
        let first = futures_util::stream::once(async {
            Ok::<_, std::convert::Infallible>(Bytes::from_static(b"audio-chunk-1;"))
        });
        let mut response = Response::new(Body::from_stream(
            first.chain(futures_util::stream::pending()),
        ));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend pouring unbounded 64 KiB chunks as fast as the
/// connection takes them, so a client that stops reading backpressures the
/// relay's bounded channel. Arrivals are signalled per request.
async fn saturating_audio_backend() -> (SocketAddr, UnboundedReceiver<()>) {
    async fn speech(State(arrivals): State<UnboundedSender<()>>) -> Response {
        let _ = arrivals.send(());
        let stream = futures_util::stream::unfold((), |()| async {
            Some((
                Ok::<_, std::convert::Infallible>(Bytes::from(vec![0xCD; 64 * 1024])),
                (),
            ))
        });
        let mut response = Response::new(Body::from_stream(stream));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (arrivals, receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state(arrivals);
    (spawn_backend(router).await, receiver)
}

/// A fake speech backend that waits `delay` before sending any headers, then
/// answers with the canned audio: time-to-headers is the first-response
/// budget's business, never the relay's per-read idle budget.
async fn slow_headers_audio_backend(delay: Duration) -> SocketAddr {
    async fn speech(State(delay): State<Duration>) -> Response {
        tokio::time::sleep(delay).await;
        let mut response = Response::new(Body::from(CANNED_AUDIO));
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }
    spawn_backend(
        Router::new()
            .route("/audio/speech", post(speech))
            .with_state(delay),
    )
    .await
}

/// The relay's response-byte ceiling: a stream totaling exactly the ceiling
/// is accepted whole and ends cleanly.
#[tokio::test]
async fn relay_accepts_a_stream_at_the_exact_byte_ceiling() {
    let (backend, mut arrivals) = fixed_size_audio_backend(RELAY_BYTE_CEILING).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!failed, "an exactly-at-ceiling stream ends cleanly");
    assert_eq!(
        u64::try_from(received.len()).unwrap(),
        RELAY_BYTE_CEILING,
        "every byte arrived"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// One byte past the ceiling fails the client's body read: the relay emits
/// its terminal error item instead of the crossing chunk, so no more than
/// the ceiling ever reaches the client and the read fails rather than
/// ending cleanly. Only the byte ceiling can produce that shape: the stream
/// is read eagerly (no idle, no blockage) and finishes far under the total
/// deadline. The lower bound is fuzzy by the chunks hyper had in flight
/// when the terminal item aborted the response. The permit goes back.
#[tokio::test]
async fn relay_fails_the_stream_one_byte_over_the_byte_ceiling() {
    let (backend, mut arrivals) = fixed_size_audio_backend(RELAY_BYTE_CEILING + 1).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(failed, "the read fails one byte over the ceiling");
    let received = u64::try_from(received.len()).unwrap();
    assert!(
        received <= RELAY_BYTE_CEILING,
        "the crossing chunk is never forwarded: {received} <= {RELAY_BYTE_CEILING}"
    );
    assert!(
        received + 1024 * 1024 >= RELAY_BYTE_CEILING,
        "the stream ran to the ceiling; only in-flight chunks were lost to the abort: {received}"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// The total-lifetime deadline ends a stream whose drip would otherwise run
/// forever. Only the deadline can fire here: the drip stays under the
/// per-read idle budget, the client reads eagerly, and the bytes are
/// nowhere near the ceiling. The permit goes back.
#[tokio::test]
async fn relay_ends_a_drip_at_the_total_lifetime_deadline() {
    let (backend, mut arrivals) = dripping_audio_backend(DRIP_GAP, None).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!received.is_empty(), "the drip flowed before the deadline");
    assert!(
        failed,
        "the total deadline fails the read, never a clean EOF"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// A drip whose chunks arrive under the upstream-idle budget is healthy:
/// the idle budget is per-read, never a cap on the stream's length.
#[tokio::test]
async fn relay_tolerates_a_drip_under_the_idle_budget() {
    let (backend, _arrivals) = dripping_audio_backend(DRIP_GAP, Some(5)).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let (received, failed) = read_to_end_or_error(response).await;
    assert!(!failed, "a sub-idle drip ends cleanly");
    assert_eq!(received, b"audio-drip;".repeat(5), "every drip arrived");
    gateway.shutdown().await;
}

/// An upstream that goes silent after headers trips the per-read idle
/// budget: the read fails well before the total-lifetime deadline (the only
/// other bound that could end a silent stream), and the permit goes back.
#[tokio::test]
async fn relay_fails_an_upstream_that_goes_idle_after_headers() {
    let (backend, mut arrivals) = stalling_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let mut response = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    let first = tokio::time::timeout(PHASE_TIMEOUT, response.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(first.is_some(), "the first chunk arrived before the stall");
    let stalled = tokio::time::Instant::now();
    let (_rest, failed) = read_to_end_or_error(response).await;
    assert!(failed, "the idle budget fails the read, never a clean EOF");
    assert!(
        stalled.elapsed() < Duration::from_secs(1),
        "the idle budget fires well ahead of the 2 s total deadline"
    );

    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    gateway.shutdown().await;
}

/// A client that stops reading backpressures the relay's bounded channel;
/// the blocked-delivery budget ends the stream and frees the permit while
/// the client still holds the unread response, and the client's eventual
/// read fails instead of seeing a clean EOF.
#[tokio::test]
async fn relay_fails_a_client_that_stops_reading() {
    let (backend, mut arrivals) = saturating_audio_backend().await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let mut first = send_within(
        client
            .post(&url)
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(first.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, first.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(chunk.is_some(), "the stream started");

    // Stop reading. The relay fills its channel and the blocked-delivery
    // budget ends the stream; the permit release is observable as the second
    // request's admission under the one-slot dominion, well ahead of the 2 s
    // total deadline (the only other bound that could end this stream).
    let stalled = tokio::time::Instant::now();
    assert_permit_released_by_admission(&client, &url, &mut arrivals).await;
    assert!(
        stalled.elapsed() < Duration::from_secs(1),
        "the blocked-delivery budget fires well ahead of the 2 s total deadline"
    );

    // The stalled client's resumed read drains the buffered chunks and then
    // fails on the terminal error item - never a clean EOF.
    let (_buffered, failed) = read_to_end_or_error(first).await;
    assert!(failed, "the stalled client's read fails, never a clean EOF");
    gateway.shutdown().await;
}

/// Headers arriving after the relay's (scaled) per-read idle budget but
/// within the first-response budget are accepted: the idle budget guards
/// only an opened body, never the wait for headers.
#[tokio::test]
async fn headers_delayed_past_the_idle_budget_are_accepted() {
    let backend = slow_headers_audio_backend(HEADER_DELAY).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), None).await;

    let response = send_within(
        reqwest::Client::new()
            .post(format!("http://{}/v1/audio/speech", gateway.addr))
            .bearer_auth("test-token")
            .json(&speech_body()),
    )
    .await;
    assert_eq!(response.status().as_u16(), 200);
    assert_eq!(bytes_within(response).await, CANNED_AUDIO);
    gateway.shutdown().await;
}

/// An upstream body error after the permit is held propagates as exactly
/// one terminal error item, and the permit goes back: a later request is
/// admitted and answered under the one-slot dominion.
#[tokio::test]
async fn relay_releases_the_permit_after_an_upstream_body_error() {
    // The first request streams one chunk and then, once the test fires the
    // trigger (a rendezvous, so the failure lands after real bytes flowed),
    // fails; every later request streams the canned audio to a clean end.
    type FailWait = Arc<Mutex<Option<oneshot::Receiver<()>>>>;
    async fn speech(
        State((calls, wait_fail, arrivals)): State<(
            Arc<Mutex<usize>>,
            FailWait,
            UnboundedSender<()>,
        )>,
    ) -> Response {
        let _ = arrivals.send(());
        let call = {
            let mut calls = calls.lock().unwrap();
            *calls += 1;
            *calls
        };
        let stream = if call == 1 {
            let wait = wait_fail
                .lock()
                .unwrap()
                .take()
                .expect("only the first request waits on the trigger");
            let first = futures_util::stream::once(async {
                Ok::<_, std::io::Error>(Bytes::from_static(b"audio-chunk-1;"))
            });
            let failure = futures_util::stream::once(async move {
                let _ = wait.await;
                Err(std::io::Error::other("upstream died mid-stream"))
            });
            Body::from_stream(first.chain(failure))
        } else {
            Body::from(CANNED_AUDIO)
        };
        let mut response = Response::new(stream);
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static("audio/mpeg"));
        response
    }

    let (fail, wait_fail) = oneshot::channel::<()>();
    let (arrivals, mut receiver) = mpsc::unbounded_channel::<()>();
    let router = Router::new()
        .route("/audio/speech", post(speech))
        .with_state((
            Arc::new(Mutex::new(0_usize)),
            Arc::new(Mutex::new(Some(wait_fail))),
            arrivals,
        ));
    let backend = spawn_backend(router).await;
    let gateway = speech_gateway(backend, Some(&["alloy"]), Some((1, 10, "queue"))).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    next_ping(&mut receiver).await;
    let mut first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, first.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(
        chunk.is_some(),
        "real audio bytes flowed before the failure"
    );
    fail.send(()).expect("the backend is waiting on the signal");
    let (_rest, failed) = read_to_end_or_error(first).await;
    assert!(
        failed,
        "the upstream body error is the terminal item: the read fails, never a clean EOF"
    );

    assert_permit_released_by_admission(&client, &url, &mut receiver).await;
    gateway.shutdown().await;
}

/// Start a gateway with two profiles (`alpha` active, `beta` idle) that both
/// route the same speech model through a one-slot dominion, so a switch
/// cancels the open stream and the new profile's admission of a later
/// request proves the permit went back.
async fn speech_profile_gateway(backend: SocketAddr) -> (tempfile::TempDir, TestServer) {
    let catalog = |backend: SocketAddr| {
        format!(
            r#"
config-version = 2

[server]
bind = "127.0.0.1:0"
api_key = "test-token"
trust_loopback = false

[[dominion]]
id = "pool"
kind = "remote"
max_concurrency = 1
max_queue = 10
policy = "queue"

[[endpoint]]
id = "fake"
protocol = "openai"
base_url = "http://{backend}"
api_key = ""
dominion = "pool"

[[model]]
name = "tts-model"
kind = "speech"
description = "a speech model for integration"
context = 8192
upstream = "backend-tts"
endpoints = ["fake"]
voices = ["alloy"]

[[profile]]
name = "alpha"
models = ["tts-model"]

[[profile]]
name = "beta"
models = ["tts-model"]
"#
        )
    };
    let temp = tempfile::TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    std::fs::write(&path, catalog(backend)).expect("write config");
    std::fs::write(
        gateway_config::profile_state_path(&path),
        "active_profile = \"alpha\"\n",
    )
    .expect("write state");
    let alpha = ProfileName::parse("alpha").expect("name");
    let config = Config::from_toml_str(&catalog(backend))
        .expect("catalog parses")
        .select_profile(&alpha)
        .expect("alpha selects");
    let context = ProfilesContext::new(Some(path), Some(alpha));
    let server =
        TestServer::start(Gateway::from_config(&config, context).expect("gateway builds")).await;
    (temp, server)
}

/// Drives a profile switch to completion over its SSE stream and returns
/// the events.
async fn switch_to(http: &reqwest::Client, addr: SocketAddr, name: &str) -> Vec<Value> {
    let response = send_within(
        http.post(format!("http://{addr}/admin/switch-profile"))
            .bearer_auth("test-token")
            .json(&serde_json::json!({ "name": name })),
    )
    .await;
    assert_eq!(response.status(), reqwest::StatusCode::OK);
    parse_sse(&text_within(response).await)
}

/// A profile switch cancels an open speech stream: the relay emits one
/// terminal error item, so the client's next read fails rather than seeing
/// a clean EOF (the `relay_sse` RequestCancelled envelope is the precedent
/// for failing rather than truncating). The switch's drain only completes
/// once the request's guard is dropped, and the guard and the dominion
/// permit live and die together in the relay task, so a completed switch
/// proves the permit went back; the new profile then admits and answers a
/// later request.
#[tokio::test]
async fn profile_switch_cancels_the_stream_and_releases_the_permit() {
    let (backend, mut arrivals) = gated_audio_backend().await;
    let (_temp, gateway) = speech_profile_gateway(backend).await;
    let client = reqwest::Client::new();
    let url = format!("http://{}/v1/audio/speech", gateway.addr);

    let first = spawn_speech(&client, &url);
    // Held, never fired: the stream stays mid-flight until the switch
    // cancels it. Consuming the arrival also keeps the second request's
    // handle next in the channel.
    let _release_first = next_arrival(&mut arrivals).await;
    let mut first = join_within(first).await.unwrap();
    assert_eq!(first.status().as_u16(), 200);
    let chunk = tokio::time::timeout(PHASE_TIMEOUT, first.chunk())
        .await
        .expect("first chunk read exceeded the phase timeout")
        .expect("first chunk read failed");
    assert!(chunk.is_some(), "the stream is mid-flight");

    let addr = gateway.addr;
    let switch =
        tokio::spawn(async move { switch_to(&reqwest::Client::new(), addr, "beta").await });

    // The switch's (test-scaled) drain deadline passes, the cancellation
    // fires, and the client's next read fails on the synthesized error item.
    // The relay's own total deadline is 2 s against the drain's 1 s, so the
    // cancellation is the only bound that can end this stream.
    let (_rest, failed) = read_to_end_or_error(first).await;
    assert!(
        failed,
        "a profile switch fails the open body read, never a clean EOF"
    );

    let events = join_within(switch).await;
    assert_eq!(
        events.last(),
        Some(&serde_json::json!({"status": "ready", "profile": "beta"})),
        "the switch completed, so the cancelled request's guard is gone"
    );

    // The permit went back with the guard: the new profile admits and
    // answers a speech request on the one-slot dominion.
    let second = spawn_speech(&client, &url);
    let release_second = next_arrival(&mut arrivals).await;
    release_second.send(()).unwrap();
    let second = join_within(second).await.unwrap();
    assert_eq!(second.status().as_u16(), 200);
    assert_eq!(bytes_within(second).await, b"audio-chunk-1;audio-chunk-2;");
    gateway.shutdown().await;
}
