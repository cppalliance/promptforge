//! Protocol refusals over `/agents/ws`: every malformed or out-of-turn
//! frame is an error frame, and the socket survives its refusals.

use super::*;

#[tokio::test]
async fn refusals_are_error_frames_and_the_socket_survives() {
    let (base, _dir, _state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;

    socket
        .send_json(&json!({ "type": "launch", "agent": "ghost" }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "error");
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown agent")),
        "an unknown agent is refused by name: {frame}"
    );

    socket
        .send_json(&json!({ "type": "attach", "session": "not-a-session" }))
        .await;
    assert_eq!(socket.recv_json().await["type"], "error");

    socket.send_json(&json!({ "type": "cancel" })).await;
    assert_eq!(
        socket.recv_json().await["type"],
        "error",
        "a cancel before any session is attached is refused"
    );

    socket.send_text("{ not json").await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "error");
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("invalid JSON")),
        "a malformed frame is refused, not fatal: {frame}"
    );

    socket.send_json(&json!({ "type": "mystery" })).await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "error");
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("unknown frame type")),
        "an unknown type is refused naming the expected ones: {frame}"
    );

    socket
        .send_json(&json!({ "type": "input_response", "token": "t", "text": "hi" }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "error");
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("before a session is attached")),
        "an input_response before any session is attached is refused: {frame}"
    );

    // The socket survives its refusals: a real launch still works, and a
    // second launch on the same socket is refused - agent windows are
    // modal, one session per socket.
    let _session = launch_echo(&mut socket).await;
    socket
        .send_json(&json!({ "type": "launch", "agent": "echo" }))
        .await;
    let frame = socket
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "error")
        .await;
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("modal")),
        "a second launch on an attached socket is refused: {frame}"
    );

    // Attached, an input_response still validates its shape.
    socket
        .send_json(&json!({ "type": "input_response", "token": 7 }))
        .await;
    let frame = socket
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "error")
        .await;
    assert!(
        frame["message"]
            .as_str()
            .is_some_and(|message| message.contains("invalid input_response")),
        "a shapeless input_response is refused, not fatal: {frame}"
    );
    socket.close().await;
}
