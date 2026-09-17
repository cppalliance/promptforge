//! Progress-subscription tests: the `GET /admin/progress` event stream
//! decodes block-by-block, heartbeat comments are skipped, a malformed
//! block degrades to one error item without ending the stream, a read
//! failure or an oversized block ends it, and a non-success status is an
//! error rather than a stream.

use super::*;

use futures_util::StreamExt as _;

use shared_progress::{EventState, ProgressEvent};

use crate::gateway::progress::MAX_EVENT_BLOCK;

/// Serializes a wire-format progress event by hand, so the tests pin
/// the JSON shape the gateway emits rather than the progress crate's
/// constructors.
fn event_json(state: &serde_json::Value) -> String {
    serde_json::json!({
        "operation": 7,
        "path": "local-models/ggml/download",
        "label": "Download",
        "state": state,
    })
    .to_string()
}

/// A mock `GET /admin/progress` that requires the bearer token and
/// answers with `body` as the verbatim SSE payload.
fn mock_progress(body: String) -> axum::Router {
    use axum::Router;
    use axum::response::IntoResponse;
    use axum::routing::get;

    Router::new().route(
        "/admin/progress",
        get(move |headers: axum::http::HeaderMap| {
            let body = body.clone();
            async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok());
                if auth != Some("Bearer tok") {
                    return (axum::http::StatusCode::UNAUTHORIZED, "bad token").into_response();
                }
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
                    .into_response()
            }
        }),
    )
}

/// Subscribes against `app` and collects the whole event stream.
async fn collect_events(app: axum::Router) -> Vec<Result<ProgressEvent, GatewayError>> {
    let base_url = serve(app).await;
    let client = GatewayClient::new(&base_url, "tok").expect("client builds in tests");
    let events = client
        .subscribe_progress()
        .await
        .expect("a well-formed stream subscribes");
    events.collect().await
}

#[tokio::test]
async fn subscribe_progress_decodes_events_and_skips_heartbeat_comments() {
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 2.0}}));
    let updated = event_json(&serde_json::json!({"Updated": {"fraction": 0.5}}));
    let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
    let body = format!(
        ": heartbeat\n\ndata: {begun}\n\ndata: {updated}\n\n: heartbeat\n\ndata: {finished}\n\n"
    );

    let events = collect_events(mock_progress(body)).await;

    let states: Vec<EventState> = events
        .iter()
        .map(|item| item.as_ref().expect("every item decodes").state)
        .collect();
    assert_eq!(
        states,
        vec![
            EventState::Begun { weight: 2.0 },
            EventState::Updated { fraction: 0.5 },
            EventState::Finished { ok: true },
        ]
    );
}

#[tokio::test]
async fn subscribe_progress_classifies_a_non_success_status() {
    let base_url = serve(mock_progress(String::new())).await;
    let client = GatewayClient::new(&base_url, "wrong-token").expect("client builds in tests");

    let Err(error) = client.subscribe_progress().await else {
        panic!("a 401 response must surface as an error");
    };
    let GatewayError::Status { status, body } = &error else {
        panic!("a 401 response is a status error, got {error}");
    };
    assert_eq!(status.as_u16(), 401);
    assert_eq!(body, "bad token");
}

#[tokio::test]
async fn subscribe_progress_yields_one_error_per_bad_event_and_continues() {
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
    let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
    let body = format!("data: {begun}\n\ndata: {{not json\n\ndata: {finished}\n\n");

    let events = collect_events(mock_progress(body)).await;

    assert_eq!(events.len(), 3, "one item per data block");
    assert!(events[0].is_ok(), "the leading event decodes");
    let error = events[1]
        .as_ref()
        .expect_err("the undecodable line is one error item");
    assert!(
        matches!(error, GatewayError::Malformed { .. }),
        "an undecodable block is a malformed error, got {error}"
    );
    assert!(
        events[2].is_ok(),
        "a bad line must not end the stream: the trailing event still arrives"
    );
}

#[tokio::test]
async fn subscribe_progress_reassembles_an_event_split_across_chunks() {
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
    let wire = format!("data: {begun}\n\n");
    let (head, tail) = wire.split_at(wire.len() / 2);
    let (head, tail) = (head.to_owned(), tail.to_owned());
    let app = axum::Router::new().route(
        "/admin/progress",
        axum::routing::get(move || {
            let chunks = vec![
                Ok::<_, std::convert::Infallible>(head.clone()),
                Ok::<_, std::convert::Infallible>(tail.clone()),
            ];
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    axum::body::Body::from_stream(futures_util::stream::iter(chunks)),
                )
            }
        }),
    );

    let events = collect_events(app).await;

    assert_eq!(events.len(), 1, "the split block decodes as one event");
    assert!(events[0].is_ok(), "the reassembled event decodes");
}

#[tokio::test]
async fn subscribe_progress_yields_one_error_on_a_mid_stream_read_failure_then_ends() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // The server promises a large body, delivers one complete event, then
    // drops the connection: the read failure must surface as one error
    // item that ends the stream, not as a hang or a silent close.
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        if let Ok((mut sock, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf).await;
            let header = "HTTP/1.1 200 OK\r\n\
                 Content-Type: text/event-stream\r\n\
                 Content-Length: 1000000\r\n\r\n";
            let _ = sock.write_all(header.as_bytes()).await;
            let _ = sock
                .write_all(format!("data: {begun}\n\n").as_bytes())
                .await;
            // Socket drops here: the promised body never completes.
        }
    });
    let client = GatewayClient::new(&format!("http://{addr}"), "tok").expect("client builds");
    let events = client
        .subscribe_progress()
        .await
        .expect("a well-formed stream subscribes")
        .collect::<Vec<_>>()
        .await;

    assert_eq!(events.len(), 2, "the decoded event, then one error item");
    assert!(events[0].is_ok(), "the leading event decodes");
    let error = events[1]
        .as_ref()
        .expect_err("the truncated body is one error item");
    assert!(
        matches!(error, GatewayError::ReadBody(_)),
        "a mid-stream read failure is a body-read error, got {error}"
    );
}

#[tokio::test]
async fn subscribe_progress_bounds_an_event_block_that_never_terminates() {
    let body = "x".repeat(MAX_EVENT_BLOCK + 1);

    let events = collect_events(mock_progress(body)).await;

    assert_eq!(events.len(), 1, "the oversized block is one error item");
    let error = events[0]
        .as_ref()
        .expect_err("an unterminated oversized block must be refused");
    assert!(
        matches!(error, GatewayError::Malformed { .. }),
        "an oversized block is a malformed error, got {error}"
    );
    assert!(
        error.to_string().contains("exceeds"),
        "the bound must report the size limit, got {error}"
    );
}

#[tokio::test]
async fn subscribe_progress_decodes_crlf_terminated_blocks() {
    // A peer that terminates its lines with CRLF still dispatches: the
    // blank-line terminator is `\r\n\r\n`, which contains no `\n\n`.
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
    let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
    let body = format!("data: {begun}\r\n\r\ndata: {finished}\r\n\r\n");

    let events = collect_events(mock_progress(body)).await;

    let states: Vec<EventState> = events
        .iter()
        .map(|item| item.as_ref().expect("every item decodes").state)
        .collect();
    assert_eq!(
        states,
        vec![
            EventState::Begun { weight: 1.0 },
            EventState::Finished { ok: true },
        ]
    );
}

#[tokio::test]
async fn subscribe_progress_discards_an_incomplete_trailing_block() {
    // The body ends mid-block: only blank-line-terminated blocks
    // dispatch, so the partial event is dropped and the stream ends.
    let begun = event_json(&serde_json::json!({"Begun": {"weight": 1.0}}));
    let finished = event_json(&serde_json::json!({"Finished": {"ok": true}}));
    let body = format!("data: {begun}\n\ndata: {finished}");

    let events = collect_events(mock_progress(body)).await;

    assert_eq!(
        events.len(),
        1,
        "the unterminated trailing block is discarded"
    );
    assert!(events[0].is_ok(), "the complete event decodes");
}
