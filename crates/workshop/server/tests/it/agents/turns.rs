//! The turn cycle over `/agents/ws`: a full turn's deltas and indexed
//! durable events, reconnect replay with the pending wait resent, and a
//! stop that keeps the conversation's run and its open question.

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
         script's ask frames as the operator's message, so no \
         tool_call_update exists"
    );
    let indices: Vec<u64> = turn
        .events
        .iter()
        .filter_map(|event| event["index"].as_u64())
        .collect();
    assert_eq!(
        indices,
        [0, 1, 2],
        "durable frames are stamped with monotonically increasing log indices"
    );
    assert_eq!(turn.events[0]["event"]["content"], "ping");
    assert!(
        turn.events[0].get("reply").is_none(),
        "a user_message settles no deltas, so its reply id is omitted"
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

/// Collects one turn as [`collect_turn`] does, refusing any
/// `input_cancelled` frame on the way.
async fn collect_turn_with_no_cancelled_wait(socket: &mut JsonSocket) -> Turn {
    let mut deltas = Vec::new();
    let mut events = Vec::new();
    let mut waits = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            match frame["type"].as_str() {
                Some("agent_delta") => deltas.push(frame),
                Some("agent_event") => {
                    let done = frame["event"]["kind"] == "agent_message";
                    events.push(frame);
                    if done {
                        break;
                    }
                }
                Some("input_required") => waits.push(
                    frame["token"]
                        .as_str()
                        .expect("the wait announces its token")
                        .to_owned(),
                ),
                Some("input_cancelled") => panic!("a stop cancels no question: {frame}"),
                Some("error") => panic!("a stop is never an error: {frame}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the turn completes within the deadline");
    Turn {
        deltas,
        events,
        waits,
    }
}

#[tokio::test]
async fn a_cancel_with_only_a_question_open_leaves_it_open_and_its_token_still_answers() {
    let (base, _dir, state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    assert_eq!(
        state.agents().unresolved_waits(&session),
        Some(vec![token.clone()]),
        "the pending wait is retained by the conversation"
    );

    socket.send_json(&json!({ "type": "cancel" })).await;
    assert_eq!(
        state.agents().unresolved_waits(&session),
        Some(vec![token.clone()]),
        "a stop drops no question to the operator"
    );

    // The original token still answers the question the stop left open,
    // and the turn it starts is a full one.
    answer(&mut socket, &token, "after cancel").await;
    let turn = collect_turn_with_no_cancelled_wait(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:after cancel",
        "the open question's own token runs the next turn"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_stop_keeps_the_conversations_one_run_while_transcript_indices_continue() {
    let (base, _dir, state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let first = collect_turn(&mut socket).await;
    let run = state
        .agents()
        .run_id(&session)
        .expect("the conversation's run has begun");

    let open = wait_after(&mut socket, &first).await;
    socket.send_json(&json!({ "type": "cancel" })).await;
    answer(&mut socket, &open, "pong").await;
    let second = collect_turn_with_no_cancelled_wait(&mut socket).await;

    assert_eq!(delta_text(&second), "echo:pong");
    let indices: Vec<u64> = first
        .events
        .iter()
        .chain(&second.events)
        .filter_map(|event| event["index"].as_u64())
        .collect();
    assert_eq!(
        indices,
        [0, 1, 2, 3, 4, 5],
        "the transcript keeps numbering across the stop"
    );
    assert_eq!(
        state.agents().run_id(&session),
        Some(run),
        "the stop keeps the conversation's one run"
    );
    socket.close().await;
}
