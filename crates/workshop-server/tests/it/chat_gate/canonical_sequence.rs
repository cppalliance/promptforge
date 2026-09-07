/// GATE 1 - multi-turn history. Current-chat behavior: the conversation
/// accumulates turn over turn, and what the user typed reaches the model
/// byte-exact with no untrusted envelope around it.
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
            "the first round carries exactly the first input"
        );
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![
                pair("user", "first ping"),
                pair("assistant", "echo:first ping"),
                pair("user", gnarly),
            ],
            "the second round carries the first exchange plus the new input, \
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
            "the third round carries the whole accumulated conversation"
        );
    }
    socket.close().await;
}
