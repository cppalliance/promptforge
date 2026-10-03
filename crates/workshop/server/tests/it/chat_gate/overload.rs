//! The stop gate: a stop mid-generation returns to waiting without an
//! error, and the run keeps its conversation.

use super::*;

/// GATE 5 - stop. Current-chat behavior: the stop button kills
/// generation mid-stream without an error, and the chat is immediately
/// usable again with the conversation so far.
#[tokio::test]
async fn gate_cancel_mid_generation_returns_to_waiting_and_next_input_works() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "hang").await;
    // Generation is provably live: a text chunk of the never-finishing
    // stream has reached the wire.
    let delta = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "error",
                "the hanging turn is not an error: {frame}"
            );
            frame["type"] == "agent_delta" && frame["kind"] == "text"
        })
        .await;
    assert_eq!(delta["content"], "nev");

    socket.send_json(&json!({ "type": "cancel" })).await;

    // A stop is a stop reason: the chat's pcall takes the dropped round,
    // and its loop asks again, and next_wait_token refuses error frames
    // on the way - which asserts exactly the no-error contract.
    let fresh = next_wait_token(&mut socket).await;
    assert_ne!(fresh, token, "the chat's next ask opens a fresh wait");
    answer(&mut socket, &fresh, "after cancel").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:hang\n\nafter cancel",
        "the next input after a mid-generation stop runs a full turn"
    );
    assert!(
        turn.events
            .iter()
            .all(|event| event["event"]["content"] != "echo:hang"),
        "the stopped generation never completes into a reply"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 2, "the stopped round and the next one");
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![pair("user", "hang\n\nafter cancel")],
            "the next round's request holds the earlier turn: the stopped \
             input stays in the retained message list beside the new one"
        );
    }
    socket.close().await;
}
