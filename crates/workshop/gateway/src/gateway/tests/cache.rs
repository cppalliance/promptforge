//! Cache API tests: the wire shapes decode, a hit buffers, a miss
//! streams, and a declined request is buffered rather than an error.

use super::*;

use std::path::PathBuf;

use axum::response::IntoResponse as _;
use futures_util::StreamExt as _;

#[test]
fn cache_event_decodes_each_wire_shape() {
    let downloading: CacheEvent =
        serde_json::from_str(r#"{"status":"downloading","bytes":5,"total":10}"#)
            .expect("downloading decodes");
    assert_eq!(
        downloading,
        CacheEvent::Downloading {
            bytes: 5,
            total: Some(10)
        }
    );
    let unknown_total: CacheEvent =
        serde_json::from_str(r#"{"status":"downloading","bytes":5,"total":null}"#)
            .expect("a null total decodes");
    assert_eq!(
        unknown_total,
        CacheEvent::Downloading {
            bytes: 5,
            total: None
        }
    );
    let ready: CacheEvent = serde_json::from_str(r#"{"status":"ready","path":"/cache/ggml.bin"}"#)
        .expect("ready decodes");
    assert_eq!(
        ready,
        CacheEvent::Ready {
            path: PathBuf::from("/cache/ggml.bin")
        }
    );
    let error: CacheEvent =
        serde_json::from_str(r#"{"status":"error","message":"boom"}"#).expect("error decodes");
    assert_eq!(
        error,
        CacheEvent::Error {
            message: "boom".to_string()
        }
    );
}

/// Mock cache route state: the last request's auth header and body,
/// captured so tests can assert what the client sent.
#[derive(Clone, Default)]
struct CacheProbe {
    authorized: std::sync::Arc<std::sync::atomic::AtomicBool>,
    sources: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl CacheProbe {
    fn sources(&self) -> Vec<String> {
        self.sources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[tokio::test]
async fn a_cache_hit_answers_a_buffered_ready_event() {
    let probe = CacheProbe::default();
    let seen = probe.clone();
    let app = axum::Router::new().route(
        "/v1/cache",
        axum::routing::post(
            move |headers: axum::http::HeaderMap, body: axum::Json<serde_json::Value>| {
                let seen = seen.clone();
                async move {
                    seen.authorized.store(
                        headers
                            .get(axum::http::header::AUTHORIZATION)
                            .and_then(|value| value.to_str().ok())
                            == Some("Bearer test-key"),
                        std::sync::atomic::Ordering::Relaxed,
                    );
                    seen.sources
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .push(
                            body["source"]
                                .as_str()
                                .expect("source is a string")
                                .to_string(),
                        );
                    axum::Json(serde_json::json!({
                        "path": "/cache/ggml-large-v3-turbo.bin",
                        "status": "ready",
                    }))
                    .into_response()
                }
            },
        ),
    );
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "test-key").expect("client builds in tests");
    let response = client
        .cache_ensure("https://example.com/models/ggml-large-v3-turbo.bin")
        .await
        .expect("the request completes");
    let CacheResponse::Buffered(answer) = response else {
        panic!("a cache hit is buffered, got {response:?}");
    };
    assert!(answer.status.is_success());
    let event: CacheEvent =
        serde_json::from_slice(&answer.body).expect("the hit body is a ready event");
    assert_eq!(
        event,
        CacheEvent::Ready {
            path: PathBuf::from("/cache/ggml-large-v3-turbo.bin")
        }
    );
    assert!(probe.authorized.load(std::sync::atomic::Ordering::Relaxed));
    assert_eq!(
        probe.sources(),
        ["https://example.com/models/ggml-large-v3-turbo.bin"]
    );
}

#[tokio::test]
async fn a_cache_miss_answers_a_download_stream() {
    let app = axum::Router::new().route(
        "/v1/cache",
        axum::routing::post(|| async {
            (
                [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                concat!(
                    "data: {\"status\":\"downloading\",\"bytes\":5,\"total\":null}\n\n",
                    "data: {\"status\":\"downloading\",\"bytes\":10,\"total\":12}\n\n",
                    "data: {\"status\":\"ready\",\"path\":\"/cache/ggml.bin\"}\n\n",
                ),
            )
        }),
    );
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .cache_ensure("https://example.com/models/ggml.bin")
        .await
        .expect("the request completes");
    let CacheResponse::Download { mut payloads, .. } = response else {
        panic!("a cache miss streams, got {response:?}");
    };
    let mut events = Vec::new();
    while let Some(item) = payloads.next().await {
        let payload = item.expect("the stream is clean");
        events.push(
            serde_json::from_str::<CacheEvent>(&payload).expect("each payload is a cache event"),
        );
    }
    assert_eq!(
        events,
        [
            CacheEvent::Downloading {
                bytes: 5,
                total: None
            },
            CacheEvent::Downloading {
                bytes: 10,
                total: Some(12)
            },
            CacheEvent::Ready {
                path: PathBuf::from("/cache/ggml.bin")
            },
        ],
        "the stream carries progress samples then the terminal ready"
    );
}

#[tokio::test]
async fn a_declined_cache_request_is_buffered_not_an_error() {
    let app = axum::Router::new().route(
        "/v1/cache",
        axum::routing::post(|| async {
            (
                axum::http::StatusCode::BAD_REQUEST,
                axum::Json(serde_json::json!({
                    "error": {"message": "bad source", "code": "malformed_request"}
                })),
            )
        }),
    );
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "").expect("client builds in tests");
    let response = client
        .cache_ensure("not-a-url")
        .await
        .expect("a declined request still completes");
    let CacheResponse::Buffered(answer) = response else {
        panic!("a declined request is buffered, got {response:?}");
    };
    assert_eq!(answer.status, reqwest::StatusCode::BAD_REQUEST);
}
