/// GATE 2 - live streaming. Current-chat behavior: while the model
/// generates, the client sees answer text and reasoning arrive as live
/// chunks, and the completed reply supersedes them under the same id.
#[tokio::test]
async fn gate_streaming_delivers_text_and_reasoning_deltas_then_the_reply() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let turn = collect_turn(&mut socket).await;

    let reasoning: String = turn
        .deltas
        .iter()
        .filter(|delta| delta["kind"] == "reasoning")
        .filter_map(|delta| delta["content"].as_str())
        .collect();
    assert_eq!(
        reasoning, "mm",
        "reasoning streams live on its own side channel during generation"
    );
    assert!(
        turn.deltas
            .iter()
            .filter(|delta| delta["kind"] == "text")
            .count()
            >= 2,
        "the mock splits content, so generation provably streams in chunks"
    );
    assert_eq!(
        delta_text(&turn),
        "echo:ping",
        "the live text chunks assemble the reply"
    );

    let reply = turn
        .events
        .last()
        .expect("the turn ends with its reply event");
    assert_eq!(reply["event"]["kind"], "agent_message");
    assert_eq!(
        reply["event"]["content"], "echo:ping",
        "the completed reply arrives after the deltas it supersedes"
    );
    assert!(
        turn.deltas
            .iter()
            .all(|delta| delta["reply"] == reply["reply"]),
        "deltas and the completed reply share the superseding id"
    );
    socket.close().await;
}
