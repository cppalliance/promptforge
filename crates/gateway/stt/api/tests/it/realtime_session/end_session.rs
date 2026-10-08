//! A Realtime session the gateway must end is reported to the client and
//! logged once, without transcript text.

use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory, scripted_service};
use serde_json::Value;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::{capture_debug_logs, encoded, update};

const FRAME_DEADLINE: Duration = Duration::from_secs(5);
const CAPTION_WORDS: &str = "confidential caption words";
const FINAL_WORDS: &str = "confidential final words";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[expect(
    clippy::expect_used,
    reason = "the helper fails the test when the server stops answering"
)]
async fn next_frame(socket: &mut Socket) -> Message {
    tokio::time::timeout(FRAME_DEADLINE, socket.next())
        .await
        .expect("a server frame arrives before the deadline")
        .expect("the server keeps the socket open until it closes it")
        .expect("the server frame is valid")
}

#[expect(
    clippy::expect_used,
    reason = "the helper fails the test on a non-JSON frame"
)]
async fn next_event(socket: &mut Socket) -> Value {
    let Message::Text(text) = next_frame(socket).await else {
        panic!("the server sends JSON text before it closes");
    };
    serde_json::from_str(text.as_str()).expect("the server frame is JSON")
}

#[expect(
    clippy::expect_used,
    reason = "the helper fails the test when the client event cannot be sent"
)]
async fn send(socket: &mut Socket, event: Value) {
    socket
        .send(Message::Text(event.to_string().into()))
        .await
        .expect("the client event sends");
}

async fn next_of_type(socket: &mut Socket, expected: &str) -> Value {
    loop {
        let event = next_event(socket).await;
        if event["type"] == expected {
            return event;
        }
    }
}

#[tokio::test]
async fn a_gateway_ended_session_logs_one_error_line_without_transcript_text() {
    let (logs, _logs_guard) = capture_debug_logs();
    let interim = ScriptedDecoder::new();
    interim.push_text(CAPTION_WORDS);
    let final_decoder = ScriptedDecoder::new();
    final_decoder.push_text(FINAL_WORDS);
    let mut service = scripted_service(
        ScriptedModelFactory::new(interim).with_final(final_decoder),
        15,
        50,
    )
    .expect("scripted generation starts");
    service.fail_realtime_finish_ready();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test server binds");
    let address = listener.local_addr().expect("the server has an address");
    let server = tokio::spawn(axum::serve(listener, service.routes()).into_future());
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/v1/realtime?intent=transcription"
    ))
    .await
    .expect("the WebSocket upgrades");

    next_of_type(&mut socket, "session.created").await;
    send(
        &mut socket,
        serde_json::from_str(&update("", true)).expect("update parses"),
    )
    .await;
    next_of_type(&mut socket, "session.updated").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "input_audio_buffer.append",
            "audio": encoded(&vec![16_384; 24_000])
        }),
    )
    .await;
    let hypothesis = next_of_type(
        &mut socket,
        "conversation.item.input_audio_transcription.hypothesis",
    )
    .await;
    assert_eq!(
        hypothesis["transcript"], CAPTION_WORDS,
        "the session holds transcript text before it fails"
    );
    send(
        &mut socket,
        serde_json::json!({"type": "input_audio_buffer.commit"}),
    )
    .await;

    let error = next_of_type(&mut socket, "error").await;
    assert_eq!(error["error"]["type"], "server_error", "{error}");
    let Message::Close(Some(frame)) = next_frame(&mut socket).await else {
        panic!("the error event is followed by a close frame with a code");
    };
    assert_eq!(u16::from(frame.code), 1011, "{frame:?}");

    let lines = logs.lines_containing("ERROR");
    let [line] = lines.as_slice() else {
        panic!("the ended session logs exactly one error line: {lines:?}");
    };
    assert!(
        line.contains("internal_error"),
        "the line names the error code: {line}"
    );
    assert_eq!(
        logs.lines_containing("confidential"),
        Vec::<String>::new(),
        "no log line carries transcript text"
    );
    server.abort();
    service.shutdown();
}
