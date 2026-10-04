//! Speech route: remote passthrough of opaque audio bytes, voice validation,
//! dominion queue admission, the kind guard, the upstream-error envelope
//! mapping, and the bounded background relay's terminal paths (byte ceiling,
//! total lifetime, upstream idle, blocked downstream, cancellation), each
//! proving the permit goes back by the admission of a later request.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode};
use axum::response::Response;
use axum::routing::post;
use futures_util::StreamExt as _;
use gateway::{Config, Gateway, ProfilesContext};
use serde_json::Value;
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use tokio::sync::oneshot;

use crate::support::{
    PHASE_TIMEOUT, RecordedRequest, Recorder, ReleaseTx, TestServer, json_within, send_within,
    spawn_backend,
};

mod admission;
mod passthrough;
mod relay;
mod streaming;
mod voices;

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

/// Starts a gateway serving one remote speech model. `voices` renders the
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
config-version = 0

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

/// An upstream error body contains provider internals - stack text,
/// internal hosts, request ids - and none of it may reach the client: the
/// envelope message is the gateway's own fixed string on both the
/// speech-only variants and the shared protocol arm.
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
