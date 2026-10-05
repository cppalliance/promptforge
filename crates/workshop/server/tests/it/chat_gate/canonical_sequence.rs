//! The history gates: the operator's message frames as their own and
//! reaches the model as typed, and the conversation accumulates
//! byte-exact across turns.

use super::*;

/// GATE 11 - the operator's message. The chat asks through `input.ask()`:
/// the broker's wait frames show the wait and clear it when it dies, the
/// answer frames as the operator's `user_message` rather than a tool
/// result, and the model receives the text as typed.
#[tokio::test]
async fn gate_the_operators_message_frames_as_a_user_message_between_wait_frames() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let typed = "two  words\r\n\"quoted\" 🦀";
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, typed).await;
    let turn = collect_turn(&mut socket).await;
    let kinds: Vec<&str> = turn
        .events
        .iter()
        .filter_map(|event| event["event"]["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        ["user_message", "agent_thought", "agent_message"],
        "the ask shows as the operator's message, never as a tool call update"
    );
    assert_eq!(
        turn.events[0]["event"],
        json!({
            "kind": "user_message", "section": "Conversation", "turn": 0, "content": typed,
        }),
        "the operator's text frames byte-exact"
    );
    assert!(
        turn.events[0].get("reply").is_none(),
        "a user_message settles no deltas, so its reply id is omitted"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(
            role_content_pairs(&requests[0]),
            vec![pair("user", typed)],
            "the model receives the operator's text as typed"
        );
    }

    // A stop with only the next ask open leaves that wait open: its own
    // token answers it, and the turn it starts frames like the first.
    let open = wait_after(&mut socket, &turn).await;
    assert_ne!(open, token, "each ask opens its own wait");
    socket.send_json(&json!({ "type": "cancel" })).await;
    answer(&mut socket, &open, "after stop").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        turn.events[0]["event"]["content"], "after stop",
        "the open wait's own token answered it"
    );
    socket.close().await;
}

/// GATE 1 - multi-turn history. The conversation accumulates turn over
/// turn, and what the user typed reaches the model byte-exact with no
/// untrusted envelope around it.
#[tokio::test]
async fn gate_history_accumulates_across_three_turns_byte_exact() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let gnarly = "line1\r\nline2 \"quoted\" {\"text\":\"decoy\"} \\slash 🦀";
    let inputs = ["first ping", gnarly, "third"];
    let mut token = next_wait_token(&mut socket).await;
    for input in inputs {
        answer(&mut socket, &token, input).await;
        let turn = collect_turn(&mut socket).await;
        assert_eq!(delta_text(&turn), format!("echo:{input}"));
        token = wait_after(&mut socket, &turn).await;
    }

    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 3, "three turns are three model rounds");
        assert_eq!(
            role_content_pairs(&requests[0]),
            vec![pair("user", "first ping")],
            "the first round sends exactly the first input"
        );
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![
                pair("user", "first ping"),
                pair("assistant", "echo:first ping"),
                pair("user", gnarly),
            ],
            "the second round sends the first exchange plus the new input, \
             the gnarly user text byte-exact and envelope-free"
        );
        assert_eq!(
            role_content_pairs(&requests[2]),
            vec![
                pair("user", "first ping"),
                pair("assistant", "echo:first ping"),
                pair("user", gnarly),
                pair("assistant", &format!("echo:{gnarly}")),
                pair("user", "third"),
            ],
            "the third round sends the whole accumulated conversation"
        );
    }
    socket.close().await;
}
