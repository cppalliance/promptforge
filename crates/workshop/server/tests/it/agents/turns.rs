//! The turn cycle over `/agents/ws`: a full turn's deltas and indexed
//! durable events, reconnect replay with the pending wait resent, and
//! turn-cancel returning the session to waiting.

use super::*;

#[tokio::test]
async fn a_full_turn_streams_deltas_and_indexed_events_sharing_the_reply_id() {
    let (base, _dir, _state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let _session = launch_echo(&mut socket).await;

    // Turn one.
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let turn = collect_turn(&mut socket).await;

    assert_eq!(
        delta_text(&turn),
        "echo:ping",
        "the live text deltas assemble the reply"
    );
    assert!(
        turn.deltas
            .iter()
            .filter(|delta| delta["kind"] == "text")
            .count()
            >= 2,
        "the mock splits content, so the turn streams multiple live chunks"
    );
    assert!(
        turn.deltas.iter().all(|delta| delta["reply"] == 0),
        "every first-turn delta is stamped with superseding reply id 0: {:?}",
        turn.deltas
    );
    let kinds: Vec<&str> = turn
        .events
        .iter()
        .filter_map(|event| event["event"]["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        ["user_message", "agent_thought", "agent_message"],
        "the durable record of one turn: input, thinking, reply - the \
         direct user_input call is not a tool call, so no tool_call_update \
         exists"
    );
    let indices: Vec<u64> = turn
        .events
        .iter()
        .filter_map(|event| event["index"].as_u64())
        .collect();
    assert_eq!(
        indices,
        [0, 1, 2],
        "durable frames carry monotonically increasing log indices"
    );
    assert_eq!(turn.events[0]["event"]["content"], "ping");
    assert!(
        turn.events[0].get("reply").is_none(),
        "a user_message settles no deltas and carries no reply id"
    );
    assert_eq!(
        turn.events[1]["reply"], 0,
        "the thinking event supersedes the reasoning deltas of its round"
    );
    assert_eq!(turn.events[2]["event"]["content"], "echo:ping");
    assert_eq!(
        turn.events[2]["reply"], 0,
        "deltas and the completed reply share the superseding event id"
    );

    // The next input works: the full turn cycle repeats with the next
    // reply id and continuing indices.
    let token = wait_after(&mut socket, &turn).await;
    answer(&mut socket, &token, "pong").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:pong");
    assert!(
        turn.deltas.iter().all(|delta| delta["reply"] == 1),
        "the second round's deltas are stamped with the next reply id"
    );
    let indices: Vec<u64> = turn
        .events
        .iter()
        .filter_map(|event| event["index"].as_u64())
        .collect();
    assert_eq!(indices, [3, 4, 5], "indices continue across turns");
    assert_eq!(turn.events[2]["reply"], 1);
    socket.close().await;
}

#[tokio::test]
async fn reconnect_replays_the_log_and_resends_the_pending_wait() {
    let (base, _dir, _state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let live = collect_turn(&mut socket).await;
    let pending = wait_after(&mut socket, &live).await;
    // The socket dies mid-session; the session survives.
    socket.close().await;

    let mut socket = connect(&base).await;
    socket
        .send_json(&json!({ "type": "attach", "session": session }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(
        frame["type"], "agent_session",
        "attach is acknowledged: {frame}"
    );
    let replayed = collect_turn(&mut socket).await;
    assert_eq!(
        replayed.events, live.events,
        "reconnect replays the persisted entries byte-alike: same indices, stamps, events"
    );
    let resent = wait_after(&mut socket, &replayed).await;
    assert_eq!(
        resent, pending,
        "the unresolved wait is resent on reconnect with its retained token"
    );

    // The reattached session is live: answering the resent wait runs a
    // full turn.
    answer(&mut socket, &resent, "again").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:again");
    socket.close().await;
}

#[tokio::test]
async fn turn_cancel_returns_to_waiting_with_input_cancelled_and_no_error_frame() {
    let (base, _dir, state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    assert_eq!(
        state.agents().unresolved_waits(&session),
        Some(vec![token.clone()]),
        "the pending wait is retained by the session"
    );

    socket.send_json(&json!({ "type": "cancel" })).await;
    let cancelled = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "error",
                "cancellation is a stop reason, never an error: {frame}"
            );
            frame["type"] == "input_cancelled"
        })
        .await;
    assert_eq!(
        cancelled["token"], *token,
        "the pending wait dies as an explicit input_cancelled"
    );

    // The relaunched agent rebuilds from the retained log and returns to
    // waiting: a fresh wait opens, and the next input works.
    let fresh = next_wait_token(&mut socket).await;
    assert_ne!(fresh, token, "the relaunched run opens a fresh wait token");
    answer(&mut socket, &fresh, "after cancel").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:after cancel",
        "the next input after a turn-cancel runs a full turn"
    );
    socket.close().await;
}
