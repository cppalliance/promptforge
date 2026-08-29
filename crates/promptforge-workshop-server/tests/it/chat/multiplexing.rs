//! Multiplexing behavior of the `/ws` chat socket: concurrent tagged
//! chats interleaving freely while each keeps its stream order, the
//! untagged and duplicate-id refusals, sequential reuse of one socket,
//! and the idle push waiting for the last chat to settle.

use std::time::Duration;

use axum::Router;
use axum::routing::post;
use futures_util::SinkExt;
use tokio_tungstenite::tungstenite;

use crate::common::spawn_gateway;

use super::{
    mock_chat_stream, read_frame, read_non_status_frame, replies_until_dones, send_chat,
    send_tagged_chat, spawn_chat_server, spawn_indexed_drip_server, tape_events,
};

#[tokio::test]
async fn two_concurrent_chats_interleave_deltas_and_tape_one_event_each() {
    let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_tagged_chat(&mut socket, 1).await;
    send_tagged_chat(&mut socket, 2).await;
    let replies = replies_until_dones(&mut socket, 2).await;

    // Chat 1 reached the gateway first, so it got the mock's longer
    // first-request stream and outlives chat 2.
    let first_done = replies
        .iter()
        .position(|frame| frame["type"] == "done")
        .expect("a done frame arrived");
    assert_eq!(
        replies[first_done]["id"], 2,
        "the shorter chat settles first: {replies:?}"
    );
    assert!(
        replies[..first_done]
            .iter()
            .any(|frame| frame["type"] == "delta" && frame["id"] == 1),
        "chat 1's deltas arrive while chat 2 streams: {replies:?}"
    );
    assert!(
        replies[first_done..]
            .iter()
            .any(|frame| frame["type"] == "delta" && frame["id"] == 1),
        "chat 1 keeps streaming after chat 2 settles: {replies:?}"
    );
    for frame in replies.iter().filter(|frame| frame["type"] == "delta") {
        let id = frame["id"].as_u64().expect("every delta carries its id");
        let prefix = if id == 1 { "c0-" } else { "c1-" };
        assert!(
            frame["content"]
                .as_str()
                .expect("delta content is text")
                .starts_with(prefix),
            "chat {id} carries its own stream's content: {frame}"
        );
    }

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 2, "one tape event per chat");
    let responses: Vec<&str> = events
        .iter()
        .map(|event| event["response"].as_str().expect("a taped assembly"))
        .collect();
    assert!(
        responses.contains(&"c0-0c0-1c0-2c0-3c0-4c0-5c0-6c0-7"),
        "chat 1 taped its full assembly: {responses:?}"
    );
    assert!(
        responses.contains(&"c1-0c1-1c1-2c1-3"),
        "chat 2 taped its full assembly: {responses:?}"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn per_chat_frame_order_holds_while_chats_interleave() {
    let (url, _tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_tagged_chat(&mut socket, 1).await;
    send_tagged_chat(&mut socket, 2).await;
    let replies = replies_until_dones(&mut socket, 2).await;

    for (id, prefix, count) in [(1, "c0-", 8), (2, "c1-", 4)] {
        let chat: Vec<&serde_json::Value> =
            replies.iter().filter(|frame| frame["id"] == id).collect();
        let (terminal, deltas) = chat.split_last().expect("the chat produced frames");
        assert_eq!(
            terminal["type"], "done",
            "chat {id}'s terminal follows every delta"
        );
        let contents: Vec<&str> = deltas
            .iter()
            .map(|frame| {
                assert_eq!(frame["type"], "delta");
                frame["content"].as_str().expect("delta content is text")
            })
            .collect();
        let expected: Vec<String> = (0..count).map(|step| format!("{prefix}{step}")).collect();
        assert_eq!(
            contents, expected,
            "chat {id}'s deltas arrive in stream order despite the interleave"
        );
    }
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_second_untagged_chat_is_refused_while_one_streams() {
    let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_chat(&mut socket).await;
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first["type"], "delta", "the first chat streams");

    send_chat(&mut socket).await;
    // The refusal interleaves with the first chat's deltas.
    let refusal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = read_non_status_frame(&mut socket).await;
            if frame["type"] == "error" {
                break frame;
            }
            assert_eq!(frame["type"], "delta", "the first chat is untouched");
        }
    })
    .await
    .expect("the refusal arrives while the first chat streams");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the rule")
            .contains("untagged"),
        "the refusal names the untagged rule: {refusal}"
    );
    assert!(
        refusal.get("id").is_none(),
        "the refused chat had no id to echo"
    );

    // The first chat still streams to completion and tapes its event;
    // the refused one never opened, so it tapes nothing.
    replies_until_dones(&mut socket, 1).await;
    assert_eq!(tape_events(&tape_dir).len(), 1, "only the live chat taped");
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn a_chat_reusing_a_live_id_is_refused() {
    let (url, tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    send_tagged_chat(&mut socket, 7).await;
    let first = read_non_status_frame(&mut socket).await;
    assert_eq!(first["type"], "delta", "the first chat streams");

    send_tagged_chat(&mut socket, 7).await;
    let refusal = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = read_non_status_frame(&mut socket).await;
            if frame["type"] == "error" {
                break frame;
            }
            assert_eq!(frame["type"], "delta", "the first chat is untouched");
        }
    })
    .await
    .expect("the refusal arrives while the first chat streams");
    assert_eq!(refusal["id"], 7, "the refusal echoes the duplicate id");
    assert!(
        refusal["message"]
            .as_str()
            .expect("the refusal names the rule")
            .contains("already streaming"),
        "the refusal names the duplicate-id rule: {refusal}"
    );

    replies_until_dones(&mut socket, 1).await;
    assert_eq!(tape_events(&tape_dir).len(), 1, "only the live chat taped");
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn sequential_chats_on_one_socket_both_complete() {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(mock_chat_stream))).await;
    let (url, tape_dir, _state) = spawn_chat_server(&base_url).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");

    for round in 1..=2 {
        let frame = serde_json::json!({
            "type": "chat",
            "id": round,
            "model": "test-model",
            "messages": [{"role": "user", "content": "ping"}],
        })
        .to_string();
        socket
            .send(tungstenite::Message::Text(frame.into()))
            .await
            .expect("the chat frame is sent");
        let first = read_non_status_frame(&mut socket).await;
        assert_eq!(
            first,
            serde_json::json!({"type": "delta", "content": "po", "id": round}),
            "round {round}: the first delta carries the request id"
        );
        let second = read_non_status_frame(&mut socket).await;
        assert_eq!(
            second,
            serde_json::json!({"type": "delta", "content": "ng", "id": round})
        );
        let third = read_non_status_frame(&mut socket).await;
        assert_eq!(third, serde_json::json!({"type": "done", "id": round}));
    }

    let events = tape_events(&tape_dir);
    assert_eq!(events.len(), 2, "one tape event per chat frame");
    assert!(
        events.iter().all(|event| event["response"] == "pong"),
        "both rounds taped the assembled response"
    );
    socket.close(None).await.expect("close the socket");
}

#[tokio::test]
async fn idle_fires_only_after_the_last_in_flight_chat_settles() {
    let (url, _tape_dir, _state) = spawn_indexed_drip_server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("connect to /ws");
    // Every session's first frame is the retained status snapshot,
    // which reads Ready on this idle fixture; consume it so a Ready
    // seen below can only be the settle path's idle push. No
    // heartbeat runs here, so no other producer pushes Ready.
    let snapshot = read_frame(&mut socket).await;
    assert_eq!(snapshot["type"], "status", "the snapshot arrives first");
    // Chat 2's shorter stream settles first; the idle push must wait
    // for chat 1.
    send_tagged_chat(&mut socket, 1).await;
    send_tagged_chat(&mut socket, 2).await;

    tokio::time::timeout(Duration::from_secs(30), async {
        let mut settled = 0;
        loop {
            let frame = read_frame(&mut socket).await;
            if frame["type"] == "done" {
                settled += 1;
                continue;
            }
            if frame["type"] == "status" && frame["label"] == "Ready" {
                assert_eq!(
                    settled, 2,
                    "the idle push fired before the last chat settled"
                );
                break;
            }
        }
    })
    .await
    .expect("the idle push arrives once both chats settle");
    socket.close(None).await.expect("close the socket");
}
