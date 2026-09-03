//! End-to-end agent-session tests over the `/agents/ws` socket: launch,
//! the full turn cycle with reply-id coalescing and indexed durable
//! frames, reconnect replay, turn-cancel, session isolation, status-bus
//! order, backoff reset, and teardown wait cleanup - all in-process
//! against an SSE mock gateway.

// clippy.toml's allow-expect-in-tests covers #[test] functions only, not
// the helpers they share; failing a test by panicking with the invariant
// named is exactly what these are for.
#![expect(
    clippy::expect_used,
    reason = "test helpers fail by panicking with the invariant named"
)]

use std::time::Duration;

use axum::Router;
use axum::http::header;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::json;

use workshop_server::{
    AgentsConfig, AppState, Config, GatewayConfig, ServerConfig, router,
};

use crate::common::{JsonSocket, spawn_gateway};

/// The echo agent: loops on `user_input`, runs one chat round per input,
/// and returns on `quit`.
const ECHO_AGENT: &str = r"
models.use('test-model')
while true do
    local input = tool_call('user_input', {})
    if input.text == 'quit' then return end
    models.chat({ { role = 'user', content = input.text } })
end
";

/// Streams `echo:<last user message>` as an SSE completion: a reasoning
/// chunk, the content split across two chunks, the finish chunk, and the
/// `[DONE]` sentinel - so a turn provably yields multiple live deltas.
async fn echo_completions(body: String) -> Response {
    let body: serde_json::Value = serde_json::from_str(&body).expect("the request is JSON");
    let text = body["messages"]
        .as_array()
        .and_then(|messages| messages.last())
        .and_then(|message| message["content"].as_str())
        .expect("the request carries a user message");
    let reply = format!("echo:{text}");
    let (first, second) = reply.split_at(reply.len() / 2);
    let chunk = |delta: serde_json::Value, finish: serde_json::Value| {
        json!({
            "model": "test-model",
            "choices": [{ "index": 0, "delta": delta, "finish_reason": finish }],
        })
        .to_string()
    };
    let events = [
        chunk(json!({ "role": "assistant" }), serde_json::Value::Null),
        chunk(
            json!({ "reasoning_content": "mm" }),
            serde_json::Value::Null,
        ),
        chunk(json!({ "content": first }), serde_json::Value::Null),
        chunk(json!({ "content": second }), serde_json::Value::Null),
        chunk(json!({}), json!("stop")),
    ];
    let mut sse = String::new();
    for event in events {
        sse.push_str("data: ");
        sse.push_str(&event);
        sse.push_str("\n\n");
    }
    sse.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
}

/// Binds the workshop router against an echoing SSE mock gateway, with
/// one discovered agent (`echo`) and the retained catalog already
/// holding `test-model`. Returns the server's base `ws://` URL, the
/// tempdir keeping the state alive, and the shared state handle.
async fn spawn_agent_server() -> (String, tempfile::TempDir, AppState) {
    let base_url =
        spawn_gateway(Router::new().route("/v1/chat/completions", post(echo_completions))).await;
    let dir = tempfile::TempDir::new().expect("tempdir");
    let agents_dir = dir.path().join("agents");
    std::fs::create_dir(&agents_dir).expect("the agents directory creates");
    std::fs::write(agents_dir.join("echo.lua"), ECHO_AGENT).expect("the echo agent writes");
    let config = Config {
        gateway: GatewayConfig {
            base_url,
            api_key: "test-key".to_string(),
        },
        server: ServerConfig {
            state_dir: dir.path().to_path_buf(),
            ..ServerConfig::default()
        },
        agents: AgentsConfig { path: agents_dir },
    };
    let state = AppState::new(&config).expect("state builds in tests");
    // The session's model catalog is built from the retained catalog at
    // launch, so the catalog lands before any test launches.
    state
        .catalog()
        .publish(vec![json!({ "id": "test-model", "object": "model" })]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind the agent test server");
    let addr = listener.local_addr().expect("agent test server address");
    let served = state.clone();
    tokio::spawn(async move {
        axum::serve(listener, router(served))
            .await
            .expect("agent test server serves");
    });
    (format!("ws://{addr}"), dir, state)
}

/// Connects to `/agents/ws` and consumes the connect-time agent list.
async fn connect(base: &str) -> JsonSocket {
    let mut socket = JsonSocket::connect(&format!("{base}/agents/ws")).await;
    assert_eq!(
        socket.recv_json().await,
        json!({ "type": "agents", "agents": ["chat", "echo"] }),
        "the connect-time push lists the discovered agents plus the built-in chat"
    );
    socket
}

/// Launches the echo agent on `socket` and returns the session id from
/// the acknowledgment frame.
async fn launch_echo(socket: &mut JsonSocket) -> String {
    socket
        .send_json(&json!({ "type": "launch", "agent": "echo" }))
        .await;
    let frame = socket.recv_json().await;
    assert_eq!(frame["type"], "agent_session");
    assert_eq!(frame["agent"], "echo");
    frame["session"]
        .as_str()
        .expect("the acknowledgment carries the session id")
        .to_owned()
}

/// Receives frames until the next `input_required` and returns its
/// token, asserting no error frame slips through on the way.
pub(crate) async fn next_wait_token(socket: &mut JsonSocket) -> String {
    let frame = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "error",
                "no error frame may interrupt: {frame}"
            );
            frame["type"] == "input_required"
        })
        .await;
    frame["token"]
        .as_str()
        .expect("the wait announces its token")
        .to_owned()
}

/// Answers the wait holding `token` with `text`.
pub(crate) async fn answer(socket: &mut JsonSocket, token: &str, text: &str) {
    socket
        .send_json(&json!({ "type": "input_response", "token": token, "text": text }))
        .await;
}

/// Everything one turn produced, collected until its completed reply
/// event: the delta frames, the durable event frames, and any wait
/// tokens announced along the way (the next turn's `input_required` may
/// hit the wire before the reply's own event frame - frame families
/// promise order within themselves, not across each other).
pub(crate) struct Turn {
    pub(crate) deltas: Vec<serde_json::Value>,
    pub(crate) events: Vec<serde_json::Value>,
    pub(crate) waits: Vec<String>,
}

/// Collects frames until the turn's `agent_message` event arrives,
/// splitting deltas, durable events, and announced waits, and refusing
/// error frames.
pub(crate) async fn collect_turn(socket: &mut JsonSocket) -> Turn {
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
                Some("error") => panic!("no error frame may interrupt a turn: {frame}"),
                // Status frames interleave freely.
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

/// The wait token following `turn`: one already captured during the
/// collection, else the next announced on the socket.
pub(crate) async fn wait_after(socket: &mut JsonSocket, turn: &Turn) -> String {
    match turn.waits.first() {
        Some(token) => token.clone(),
        None => next_wait_token(socket).await,
    }
}

/// Concatenates the turn's text-delta contents.
pub(crate) fn delta_text(turn: &Turn) -> String {
    turn.deltas
        .iter()
        .filter(|delta| delta["kind"] == "text")
        .filter_map(|delta| delta["content"].as_str())
        .collect()
}

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
        [
            "user_message",
            "tool_call_update",
            "agent_thought",
            "agent_message"
        ],
        "the durable record of one turn: input, the user_input tool's own \
         result, thinking, reply"
    );
    let indices: Vec<u64> = turn
        .events
        .iter()
        .filter_map(|event| event["index"].as_u64())
        .collect();
    assert_eq!(
        indices,
        [0, 1, 2, 3],
        "durable frames carry monotonically increasing log indices"
    );
    assert_eq!(turn.events[0]["event"]["content"], "ping");
    assert!(
        turn.events[0].get("reply").is_none(),
        "a user_message settles no deltas and carries no reply id"
    );
    assert!(
        turn.events[1].get("reply").is_none(),
        "a tool result settles no deltas and carries no reply id"
    );
    assert_eq!(
        turn.events[2]["reply"], 0,
        "the thinking event supersedes the reasoning deltas of its round"
    );
    assert_eq!(turn.events[3]["event"]["content"], "echo:ping");
    assert_eq!(
        turn.events[3]["reply"], 0,
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
    assert_eq!(indices, [4, 5, 6, 7], "indices continue across turns");
    assert_eq!(turn.events[3]["reply"], 1);
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
            [0, 1, 2, 3],
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
        dir.path().join("agents").join("boom.lua"),
        "tool_call('user_input', {})\nerror('kaboom')",
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
