//! Session lifecycle over `/agents/ws`: session isolation, status-bus
//! ordering with the backoff reset, teardown wait cleanup, and a
//! terminal agent failure surfacing as an error frame.

use super::*;

#[tokio::test]
async fn two_sessions_do_not_cross_talk() {
    let (base, _dir, _state) = spawn_agent_server().await;
    let mut first = connect(&base).await;
    let mut second = connect(&base).await;
    let first_id = launch_echo(&mut first).await;
    let second_id = launch_echo(&mut second).await;
    assert_ne!(first_id, second_id, "every launch is its own session");

    let first_token = next_wait_token(&mut first).await;
    let second_token = next_wait_token(&mut second).await;
    answer(&mut first, &first_token, "alpha").await;
    answer(&mut second, &second_token, "beta").await;
    let first_turn = collect_turn(&mut first).await;
    let second_turn = collect_turn(&mut second).await;

    assert_eq!(delta_text(&first_turn), "echo:alpha");
    assert_eq!(delta_text(&second_turn), "echo:beta");
    for turn in [&first_turn, &second_turn] {
        let indices: Vec<u64> = turn
            .events
            .iter()
            .filter_map(|event| event["index"].as_u64())
            .collect();
        assert_eq!(
            indices,
            [0, 1, 2],
            "each session's log is its own: no foreign entries shift the indices"
        );
    }
    assert_eq!(
        first_turn.events[0]["event"]["content"], "alpha",
        "the first session's history holds only its own input"
    );
    assert_eq!(
        second_turn.events[0]["event"]["content"], "beta",
        "the second session's history holds only its own input"
    );
    first.close().await;
    second.close().await;
}

#[tokio::test]
async fn status_frames_fire_in_order_and_a_completed_reply_resets_the_backoff() {
    let (base, _dir, state) = spawn_agent_server().await;
    // The backoff stands escalated, as after an outage; the completed
    // reply is the useful work that returns it to base.
    let _ = state.backoff().next_delay();
    let _ = state.backoff().next_delay();
    assert!(state.backoff().is_escalated_for_test());

    // Status updates ride the main `/ws` socket as unsolicited frames.
    let mut status = JsonSocket::connect(&format!("{base}/ws")).await;
    let mut socket = connect(&base).await;
    let _session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:ping");

    // Thinking on turn dispatch, Generating on the first answer delta,
    // idle on completion - scanned in order through the status stream.
    let deadline = Duration::from_secs(10);
    let thinking = status
        .recv_until(deadline, |frame| {
            frame["type"] == "status" && frame["activity"] == "thinking"
        })
        .await;
    assert_eq!(thinking["label"], "Running agent turn");
    let generating = status
        .recv_until(deadline, |frame| {
            frame["type"] == "status" && frame["activity"] == "generating"
        })
        .await;
    assert_eq!(generating["label"], "Streaming response...");
    let idle = status
        .recv_until(deadline, |frame| {
            frame["type"] == "status" && frame["label"] == "Ready"
        })
        .await;
    assert_eq!(idle["activity"], "general");
    assert!(
        !state.backoff().is_escalated_for_test(),
        "a completed reply records useful work and resets the backoff"
    );
    socket.close().await;
    status.close().await;
}

#[tokio::test]
async fn teardown_cancels_pending_waits_and_leaks_none() {
    let (base, _dir, state) = spawn_agent_server().await;
    let mut socket = connect(&base).await;
    let session = launch_echo(&mut socket).await;
    let token = next_wait_token(&mut socket).await;
    assert_eq!(
        state.agents().unresolved_waits(&session),
        Some(vec![token.clone()]),
        "the wait is retained while the session runs"
    );

    assert!(state.agents().close(&session), "the session closes");
    let cancelled = socket
        .recv_until(Duration::from_secs(10), |frame| {
            frame["type"] == "input_cancelled"
        })
        .await;
    assert_eq!(
        cancelled["token"], *token,
        "teardown announces the dying wait instead of leaking it"
    );
    assert!(
        state.agents().unresolved_waits(&session).is_none(),
        "a closed session leaves the registry"
    );
    assert!(
        !state.agents().close(&session),
        "closing an already-closed session is a no-op"
    );
    socket.close().await;
}

#[tokio::test]
async fn a_terminal_agent_failure_reaches_the_socket_as_an_error_frame() {
    let (base, dir, state) = spawn_agent_server().await;
    // An agent that dies after its first input, so the socket is attached
    // and subscribed long before the failure fires.
    std::fs::write(
        dir.path().join("agents").join("boom.md"),
        r"---
name: boom
description: The terminally failing test agent.
promptforge: 0
---

# Boom

## Conversation

```lua
user_input()
error('kaboom')
```
",
    )
    .expect("the boom agent writes");
    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(
        socket.recv_json().await,
        json!({ "type": "agents", "agents": ["boom", "chat", "echo"] }),
        "the freshly written agent is discovered on this connect"
    );
    socket
        .send_json(&json!({ "type": "launch", "agent": "boom" }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "agent_session");
    let session = frame["session"]
        .as_str()
        .expect("the acknowledgment carries the session id")
        .to_owned();

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "go").await;
    let error = socket
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "error")
        .await;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("kaboom")),
        "the run's own failure reaches the SPA as an error frame, not just \
         the status bus: {error}"
    );
    // The failed run ends the session; the registry lets it go.
    tokio::time::timeout(Duration::from_secs(10), async {
        while state.agents().unresolved_waits(&session).is_some() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a failed run leaves the registry");
    socket.close().await;
}
