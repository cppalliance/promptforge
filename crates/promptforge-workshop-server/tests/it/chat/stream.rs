//! Streaming relay behavior of the `/ws` chat socket: delta, reasoning,
//! done, and error frame sequences, malformed-frame answers, tape
//! durability, and the backoff reset on delivered tokens.

use axum::Router;
use axum::body::Body;
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use futures_util::{SinkExt, stream};
use serde_json::json;
use tokio_tungstenite::tungstenite;

use crate::common::{JsonSocket, TestServer, spawn_gateway};

use super::{
    REASONING_STREAM_BODY, STREAM_BODY, UPSTREAM_ERROR, authorized, mock_chat_stream,
    read_non_status_frame, send_chat, send_chat_json, spawn_chat_server, streaming_gateway,
    tape_events,
};

/// Streams `REASONING_STREAM_BODY` as a mock reasoning model.
async fn mock_chat_stream_reasons(headers: HeaderMap, body: String) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        REASONING_STREAM_BODY,
    )
        .into_response()
}

/// Answers with one good SSE event, then aborts the body mid-stream.
///
/// The pause after the first chunk gives hyper time to flush the headers
/// and the event before the body errors, so the client observes a stream
/// that fails mid-way rather than a connection that never answered.
async fn mock_chat_stream_dies(headers: HeaderMap, body: String) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    let chunks = stream::unfold(0u8, |step| async move {
        match step {
            0 => Some((
                Ok::<_, std::io::Error>(axum::body::Bytes::from_static(
                    b"data: {\"choices\":[{\"delta\":{\"content\":\"po\"}}]}\n\n",
                )),
                1,
            )),
            1 => {
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                Some((Err(std::io::Error::other("injected upstream failure")), 2))
            }
            _ => None,
        }
    });
    (
        [(header::CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(chunks),
    )
        .into_response()
}

/// Declines a streaming request with an ordinary JSON error envelope.
async fn mock_chat_declines_stream(headers: HeaderMap, body: String) -> Response {
    assert!(authorized(&headers));
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    assert_eq!(body["stream"], true, "the stream flag is forwarded");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::CONTENT_TYPE, "application/json")],
        UPSTREAM_ERROR,
    )
        .into_response()
}

#[tokio::test]
async fn a_chat_streams_deltas_in_order_then_done_and_tapes_the_exchange() {
    let base_url = spawn_gateway(streaming_gateway(STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    send_chat_json(&mut socket, 1).await;

    // The role-priming event carries no content and yields no frame; every
    // reply frame echoes the request id.
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po", "id": 1})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng", "id": 1})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "done", "id": 1})
    );

    // The terminal frame follows the tape write, so holding `done` means
    // the tape is durable.
    let events = server.tape_events();
    assert_eq!(events.len(), 1, "exactly one tape event per chat frame");
    assert_eq!(events[0]["model"], "test-model");
    assert_eq!(
        events[0]["response"], "pong",
        "the tape holds the assembled content, not the raw frames"
    );
    socket.close().await;
}

#[tokio::test]
async fn reasoning_deltas_arrive_as_reasoning_frames_and_stay_off_the_tape() {
    let base_url = spawn_gateway(streaming_gateway(REASONING_STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;
    // Untagged on purpose: an absent id is omitted from every reply frame,
    // not serialized as null.
    socket
        .send_json(&json!({
            "type": "chat",
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        }))
        .await;

    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "reasoning", "content": "hmm "}),
        "the reasoning side channel arrives as reasoning frames"
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "reasoning", "content": "okay"})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po"})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng"})
    );
    assert_eq!(socket.recv_non_status().await, json!({"type": "done"}));

    let events = server.tape_events();
    assert_eq!(events.len(), 1, "exactly one tape event per chat frame");
    assert_eq!(
        events[0]["response"], "pong",
        "the tape holds the answer content only, never the reasoning"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_malformed_frame_is_answered_with_an_error_and_the_session_survives() {
    let base_url = spawn_gateway(streaming_gateway(STREAM_BODY)).await;
    let server = TestServer::spawn(&base_url);
    let mut socket = JsonSocket::connect(&server.ws_url("/ws")).await;

    for bad in [
        "not json",
        r#"{"type":"bogus"}"#,
        r#"{"type":"chat","model":"test-model"}"#,
    ] {
        socket.send_text(bad).await;
        let frame = socket.recv_non_status().await;
        assert_eq!(
            frame["type"], "error",
            "a malformed frame is answered, not fatal: {bad}"
        );
        assert!(
            frame["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "the error frame names the failure: {frame}"
        );
    }

    // The session survives: a well-formed frame still streams a full reply.
    send_chat_json(&mut socket, 2).await;
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "po", "id": 2})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "delta", "content": "ng", "id": 2})
    );
    assert_eq!(
        socket.recv_non_status().await,
        json!({"type": "done", "id": 2})
    );
    socket.close().await;
}

#[tokio::test]
async fn a_delivered_token_resets_the_backoff() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
    // The backoff stands escalated, as after an outage; the first
    // streamed token is the useful work that returns it to base.
    let _ = state.backoff().next_delay();
    let _ = state.backoff().next_delay();
    assert!(state.backoff().is_escalated_for_test());
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
    assert!(
        !state.backoff().is_escalated_for_test(),
        "a delivered token is useful work and resets the backoff"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_delivered_reasoning_token_resets_the_backoff() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_reasons)))
            .await;
    let (url, _tape_dir, state) = spawn_chat_server(&base_url).await;
    // The reasoning side channel streams before any answer content,
    // so a reset observed on its first chunk proves a reasoning
    // token counts as useful work on its own.
    let _ = state.backoff().next_delay();
    let _ = state.backoff().next_delay();
    assert!(state.backoff().is_escalated_for_test());
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(
        first,
        serde_json::json!({"type": "reasoning", "content": "hmm "})
    );
    assert!(
        !state.backoff().is_escalated_for_test(),
        "a streamed reasoning token is useful work and resets the backoff"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn chat_frames_relay_deltas_in_order_then_done() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, _tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    // The role-priming event carries no content and yields no frame.
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
    let second = read_non_status_frame(&mut socket).await;
    assert_eq!(
        second,
        serde_json::json!({"type": "delta", "content": "ng"})
    );
    let third = read_non_status_frame(&mut socket).await;
    assert_eq!(third, serde_json::json!({"type": "done"}));
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn reasoning_deltas_relay_as_reasoning_frames_and_stay_off_the_tape() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_reasons)))
            .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(
        first,
        serde_json::json!({"type": "reasoning", "content": "hmm "}),
        "the reasoning side channel arrives as reasoning frames"
    );
    let second = read_non_status_frame(&mut socket).await;
    assert_eq!(
        second,
        serde_json::json!({"type": "reasoning", "content": "okay"})
    );
    let third = read_non_status_frame(&mut socket).await;
    assert_eq!(third, serde_json::json!({"type": "delta", "content": "po"}));
    let fourth = read_non_status_frame(&mut socket).await;
    assert_eq!(
        fourth,
        serde_json::json!({"type": "delta", "content": "ng"})
    );
    let fifth = read_non_status_frame(&mut socket).await;
    assert_eq!(fifth, serde_json::json!({"type": "done"}));

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 1, "exactly one event per chat frame");
    assert_eq!(
        events[0]["response"], "pong",
        "the tape holds the answer content only, never the reasoning"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_completed_chat_tapes_one_event_with_the_assembled_response() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;
    // The terminal frame is sent after the tape write, so holding `done`
    // means the tape is durable.
    loop {
        let frame = read_non_status_frame(&mut socket).await;
        if frame["type"] == "done" {
            break;
        }
    }

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 1, "exactly one event per chat frame");
    let event = &events[0];
    assert_eq!(event["kind"], "chat");
    assert_eq!(event["model"], "test-model");
    assert_eq!(
        event["request"]["type"], "chat",
        "the frame is taped as received"
    );
    assert_eq!(event["request"]["messages"][0]["content"], "ping");
    assert_eq!(
        event["response"], "pong",
        "the tape holds the assembled content, not the raw frames"
    );
    assert!(event["latency_ms"].is_u64(), "latency_ms is an integer");
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_mid_stream_gateway_error_sends_an_error_frame_and_tapes_the_note() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream_dies)))
            .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first, serde_json::json!({"type": "delta", "content": "po"}));
    let second = read_non_status_frame(&mut socket).await;
    assert_eq!(second["type"], "error");
    let message = second["message"].as_str().expect("the error is a string");
    assert!(!message.is_empty(), "the error frame names the failure");

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 1, "an errored stream still tapes one event");
    let note = events[0]["response"]["error"]
        .as_str()
        .expect("the error note is a string");
    assert!(!note.is_empty(), "the error note names the failure");
    assert_eq!(
        events[0]["response"]["content"], "po",
        "the partial content is taped alongside the error"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_declined_stream_sends_an_error_frame_and_tapes_the_envelope() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_declines_stream)))
            .await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;

    let frame = read_non_status_frame(&mut socket).await;
    assert_eq!(frame["type"], "error");
    assert_eq!(frame["message"], "model unloaded");

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 1, "a declined stream tapes exactly one event");
    assert_eq!(
        events[0]["response"]["error"]["code"], "upstream_unavailable",
        "the gateway's own envelope is taped"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn malformed_frames_are_answered_with_error_frames() {
    let (url, _tape_dir, _state) = spawn_chat_server("http://127.0.0.1:1").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    for bad in [
        "not json",
        r#"{"type":"bogus"}"#,
        r#"{"type":"chat","model":"test-model"}"#,
    ] {
        socket
            .send(tungstenite::Message::Text(bad.into()))
            .await
            .expect("the frame is sent");
        let frame = read_non_status_frame(&mut socket).await;
        assert_eq!(
            frame["type"], "error",
            "a malformed frame is answered, not fatal: {bad}"
        );
    }
    // The session survives: a well-formed frame still gets through to
    // the (unreachable) gateway and answers with its own error.
    send_chat(&mut socket).await;
    let frame = read_non_status_frame(&mut socket).await;
    assert_eq!(frame["type"], "error");
    socket.close(None).await.expect("close the socket");
}
