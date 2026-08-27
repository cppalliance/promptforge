//! The buffered gateway relay: the `/chat` and `/v1/models` handlers and
//! the helpers that shape their responses and tape their round-trips.

use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::app::AppState;
use crate::gateway::{GatewayError, GatewayResponse};
use crate::protocol::{Activity, ChatRequest};
use crate::status::StatusBus;
use crate::tape::{Tape, TapeEvent};

/// Relays the gateway's model catalog to the caller verbatim.
///
/// While the heartbeat reports the gateway down, the catalog is not
/// attempted: the route answers 502 with a user-visible message instead.
pub(crate) async fn models(State(state): State<AppState>) -> Response {
    if !state.health().is_reachable() {
        return gateway_unreachable();
    }
    let status = state.status();
    status.info(
        "Loading models...",
        "fetching the gateway model catalog",
        Activity::General,
    );
    let result = state.gateway.list_models().await;
    report_gateway_outcome(&status, &result, "GET /v1/models");
    relay(result)
}

/// Reports a gateway call's outcome on the status bus: back to idle on
/// success, otherwise the error label matching the failure shape.
fn report_gateway_outcome(
    status: &StatusBus,
    result: &Result<GatewayResponse, GatewayError>,
    route: &str,
) {
    match result {
        Ok(upstream) if upstream.status.is_success() => status.idle(),
        Ok(upstream) => status.error(
            format!("Gateway error: {}", upstream.status),
            format!("{route} answered a non-success status"),
            Activity::General,
        ),
        Err(error) => status.error("Connection lost", error.to_string(), Activity::General),
    }
}

/// Forwards a buffered chat completion to the gateway, tapes the
/// round-trip, and relays the reply verbatim.
///
/// A completed round-trip is recorded on the session tape; a tape failure is
/// logged and never changes the response. Streaming moved to `GET /ws`: a
/// request carrying `"stream": true` is rejected with 400.
pub(crate) async fn chat(State(state): State<AppState>, body: String) -> Response {
    let request_value: serde_json::Value = match serde_json::from_str(&body) {
        Ok(value) => value,
        Err(error) => return bad_request(&error),
    };
    if request_value
        .get("stream")
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
        return stream_unsupported();
    }
    let request: ChatRequest = match serde_json::from_value(request_value.clone()) {
        Ok(request) => request,
        Err(error) => return bad_request(&error),
    };
    // A gateway the heartbeat knows is down is not attempted, matching the
    // /ws chat short-circuit.
    if !state.health().is_reachable() {
        return gateway_unreachable();
    }
    let status = state.status();
    status.info(
        "Submitting request...",
        "a buffered chat completion",
        Activity::General,
    );
    status.info(
        "Waiting for response...",
        "the gateway has the request",
        Activity::General,
    );
    let started = Instant::now();
    let result = state.gateway.chat_completion(&request).await;
    report_gateway_outcome(&status, &result, "POST /v1/chat/completions");
    let latency = started.elapsed();
    if let Ok(upstream) = &result {
        let response_value = value_from_bytes(&upstream.body);
        tape_round_trip(
            &state.tape,
            request.model,
            request_value,
            response_value,
            latency,
        )
        .await;
    }
    relay(result)
}

/// Renders the 502 envelope for a gateway the heartbeat knows is down: the
/// request is not attempted, and the message is user-visible.
pub(crate) fn gateway_unreachable() -> Response {
    (
        axum::http::StatusCode::BAD_GATEWAY,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": "Gateway unreachable",
                "code": "gateway_unreachable",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Renders the 400 envelope for a chat request that asked for a stream.
fn stream_unsupported() -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": "streaming moved to GET /ws; POST /chat is buffered only",
                "code": "stream_unsupported",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Parses a gateway body as JSON, falling back to a plain string.
pub(crate) fn value_from_bytes(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body)
        .unwrap_or_else(|_| serde_json::Value::String(String::from_utf8_lossy(body).into_owned()))
}

/// Records one chat round-trip on the session tape.
///
/// A tape failure is logged and never changes the response.
pub(crate) async fn tape_round_trip(
    tape: &Arc<Tape>,
    model: String,
    request: serde_json::Value,
    response: serde_json::Value,
    latency: Duration,
) {
    let written = {
        let tape = Arc::clone(tape);
        tokio::task::spawn_blocking(move || {
            let event = TapeEvent::chat(model, request, response, latency)?;
            tape.record(&event)
        })
        .await
    };
    match written {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::error!(%error, "session tape event was not recorded"),
        Err(error) => tracing::error!(%error, "session tape writer did not finish"),
    }
}

/// Renders the 400 envelope for an unparseable chat body.
pub(crate) fn bad_request(error: &serde_json::Error) -> Response {
    (
        axum::http::StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::json!({
            "error": {
                "message": format!("invalid chat request: {error}"),
                "code": "bad_request",
            }
        })
        .to_string(),
    )
        .into_response()
}

/// Turns a gateway call outcome into the workshop's HTTP response.
///
/// Success (any status) is relayed byte-for-byte; a transport failure
/// becomes `502 Bad Gateway` with a small JSON error envelope.
pub(crate) fn relay(result: Result<GatewayResponse, GatewayError>) -> Response {
    match result {
        Ok(upstream) => (
            upstream.status,
            [(header::CONTENT_TYPE, "application/json")],
            upstream.body,
        )
            .into_response(),
        Err(error) => (
            axum::http::StatusCode::BAD_GATEWAY,
            [(header::CONTENT_TYPE, "application/json")],
            serde_json::json!({
                "error": {
                    "message": error.to_string(),
                    "code": "gateway_unreachable",
                }
            })
            .to_string(),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::body::Body;
    use axum::http::{HeaderMap, Request, StatusCode};
    use axum::routing::{get, post};
    use axum::{Json, Router};
    use tower::ServiceExt;

    use crate::app::fixtures::{body_bytes, spawn_gateway, state_for};
    use crate::app::router;
    use crate::catalog::CatalogBus;
    use crate::gateway::GatewayClient;
    use crate::heartbeat::GatewayHealth;
    use crate::transcribe::VoiceSlot;
    use crate::workspace::Workspace;

    const CATALOG: &str = r#"{"object":"list","data":[{"id":"test-model","object":"model","created":1,"owned_by":"promptforge"}]}"#;
    const COMPLETION: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1,"model":"test-model","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}"#;
    const UPSTREAM_ERROR: &str =
        r#"{"error":{"message":"model unloaded","code":"upstream_unavailable"}}"#;
    const CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}]}"#;
    const STREAM_CHAT_BODY: &str =
        r#"{"model":"test-model","messages":[{"role":"user","content":"ping"}],"stream":true}"#;

    fn authorized(headers: &HeaderMap) -> bool {
        headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            == Some("Bearer test-key")
    }

    async fn mock_models(headers: HeaderMap) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ([(header::CONTENT_TYPE, "application/json")], CATALOG).into_response()
    }

    async fn mock_chat(headers: HeaderMap, Json(body): Json<serde_json::Value>) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        assert_eq!(body["model"], "test-model");
        assert!(body["messages"].is_array());
        ([(header::CONTENT_TYPE, "application/json")], COMPLETION).into_response()
    }

    async fn mock_broken_models() -> Response {
        (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "application/json")],
            UPSTREAM_ERROR,
        )
            .into_response()
    }

    async fn mock_chat_not_json(headers: HeaderMap) -> Response {
        if !authorized(&headers) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        (
            [(header::CONTENT_TYPE, "text/plain")],
            "gateway replied in plain text",
        )
            .into_response()
    }

    async fn spawn_mock_gateway() -> String {
        spawn_gateway(
            Router::new()
                .route("/v1/models", get(mock_models))
                .route("/v1/chat/completions", post(mock_chat)),
        )
        .await
    }

    async fn spawn_broken_mock_gateway() -> String {
        spawn_gateway(Router::new().route("/v1/models", get(mock_broken_models))).await
    }

    fn chat_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(CHAT_BODY))
            .expect("static request parts are valid")
    }

    fn stream_chat_request() -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(STREAM_CHAT_BODY))
            .expect("static request parts are valid")
    }

    #[tokio::test]
    async fn models_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], CATALOG.as_bytes());
    }

    #[tokio::test]
    async fn chat_completions_are_relayed_byte_for_byte() {
        let base_url = spawn_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }

    #[tokio::test]
    async fn gateway_error_status_is_relayed_byte_for_byte() {
        let base_url = spawn_broken_mock_gateway().await;
        let (state, _tape_dir) = state_for(&base_url);
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(&body_bytes(response).await[..], UPSTREAM_ERROR.as_bytes());
    }

    #[tokio::test]
    async fn unreachable_gateway_becomes_bad_gateway() {
        // Port 1 is never listening, so the connect fails deterministically.
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
    }

    #[tokio::test]
    async fn a_gateway_known_down_short_circuits_the_catalog_with_bad_gateway() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        state.health().publish(false);
        let request = Request::builder()
            .uri("/v1/models")
            .body(Body::empty())
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
        assert_eq!(
            json["error"]["message"], "Gateway unreachable",
            "the short-circuit message is user-visible"
        );
    }

    #[tokio::test]
    async fn a_gateway_known_down_short_circuits_buffered_chat_with_bad_gateway() {
        let (state, tape_dir) = state_for("http://127.0.0.1:1");
        state.health().publish(false);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "gateway_unreachable");
        assert_eq!(json["error"]["message"], "Gateway unreachable");
        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        assert!(
            raw.trim().is_empty(),
            "no upstream attempt means no tape event"
        );
    }

    #[tokio::test]
    async fn malformed_chat_body_is_a_bad_request() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let request = Request::builder()
            .method("POST")
            .uri("/chat")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(r#"{"not_model":true}"#))
            .expect("static request parts are valid");
        let response = router(state)
            .oneshot(request)
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn chat_round_trip_writes_exactly_one_tape_event() {
        let base_url = spawn_mock_gateway().await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        assert!(raw.ends_with('\n'), "the tape line is complete: {raw:?}");
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 1, "exactly one event per round-trip");
        let event: serde_json::Value =
            serde_json::from_str(lines[0]).expect("the tape line is valid JSON");
        assert_eq!(event["kind"], "chat");
        assert_eq!(event["model"], "test-model");
        assert_eq!(event["request"]["messages"][0]["content"], "ping");
        assert_eq!(event["response"]["id"], "chatcmpl-1");
        assert!(event["latency_ms"].is_u64(), "latency_ms is an integer");
        let ts = event["ts"].as_str().expect("ts is a string");
        time::OffsetDateTime::parse(ts, &time::format_description::well_known::Rfc3339)
            .expect("ts is RFC 3339");
    }

    #[tokio::test]
    async fn non_json_gateway_body_is_taped_as_a_string() {
        let base_url =
            spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_not_json)))
                .await;
        let (state, tape_dir) = state_for(&base_url);
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);

        let raw =
            std::fs::read_to_string(tape_dir.path().join("tape.jsonl")).expect("the tape exists");
        let event: serde_json::Value =
            serde_json::from_str(raw.lines().next().expect("one event per round-trip"))
                .expect("the tape line is valid JSON");
        assert_eq!(event["response"], "gateway replied in plain text");
    }

    #[tokio::test]
    async fn a_streaming_chat_request_is_rejected_with_bad_request() {
        let (state, _tape_dir) = state_for("http://127.0.0.1:1");
        let response = router(state)
            .oneshot(stream_chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = body_bytes(response).await;
        let json: serde_json::Value = serde_json::from_slice(&body).expect("error body is JSON");
        assert_eq!(json["error"]["code"], "stream_unsupported");
    }

    #[tokio::test]
    async fn tape_write_failure_does_not_fail_the_chat_response() {
        struct FailingWriter;
        impl std::io::Write for FailingWriter {
            fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("injected tape failure"))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let base_url = spawn_mock_gateway().await;
        let gateway = GatewayClient::new(&base_url, "test-key").expect("client builds in tests");
        let state = AppState {
            gateway,
            tape: Arc::new(Tape::with_writer_for_test(FailingWriter)),
            voice: VoiceSlot::default(),
            status: StatusBus::new(),
            health: GatewayHealth::new(),
            catalog: CatalogBus::new(),
            workspace: Workspace::new(),
        };
        let response = router(state)
            .oneshot(chat_request())
            .await
            .expect("the router is infallible");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(&body_bytes(response).await[..], COMPLETION.as_bytes());
    }
}
