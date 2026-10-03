//! Run lifecycle gates: the dropdown's pick serving each next round, and
//! delayed startup convergence on the server broker's catalog wait.

use workshop_run_log::{RecordFilter, RecordKind, RunId, RunLog};

use super::*;

/// The model each answered round of `run` names in the run log, in
/// round order, read once the run is recorded whole.
async fn answered_models(server: &GateServer, run: RunId) -> Vec<String> {
    let log_file = server.dir.path().join("harness").join("runs.db");
    let log = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Ok(log) = RunLog::open(&log_file).await
                && let Ok(row) = log.run(run).await
                && row.outcome.is_some()
            {
                return log;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the run is recorded and closed within the deadline");
    let answers = RecordFilter {
        kind: Some(RecordKind::Answer),
        last: None,
    };
    log.records(run, answers)
        .await
        .expect("the answers read back")
        .iter()
        .filter_map(|stored| stored.record.payload.pointer("/Chat/Ok/model"))
        .map(|model| model.as_str().expect("a round's model is text").to_owned())
        .collect()
}

/// GATE 3 - model switch. The dropdown's pick is read at each round, so
/// a model picked between turns serves the live run's next round, and
/// that round's reply event and its answer in the run log name it.
#[tokio::test]
async fn gate_a_model_picked_between_turns_serves_the_next_round() {
    let server = spawn_chat_server(&["model-a", "model-b"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "one").await;
    let turn = collect_turn(&mut socket).await;
    let reply = turn.events.last().expect("the first turn completes");
    assert_eq!(
        reply["event"]["model"], "model-a",
        "the first turn runs on the selected model"
    );

    server
        .state
        .menu()
        .set_selected("model-b")
        .expect("model-b is in the retained catalog");

    let token = wait_after(&mut socket, &turn).await;
    answer(&mut socket, &token, "two").await;
    let turn = collect_turn(&mut socket).await;
    let reply = turn.events.last().expect("the second turn completes");
    assert_eq!(
        reply["event"]["model"], "model-b",
        "the next round's reply names the model picked between turns"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests[0]["model"], "model-a");
        assert_eq!(
            requests[1]["model"], "model-b",
            "the next round is sent to the new pick"
        );
    }

    let run = server
        .state
        .agents()
        .run_id(&session)
        .expect("the conversation's run has begun");
    assert!(server.state.agents().close(&session), "the session closes");
    assert_eq!(
        answered_models(&server, run).await,
        ["model-a", "model-b"],
        "each round's answer record names the model that served it"
    );
    socket.close().await;
}

/// GATE 8 - delayed startup convergence. Launch acknowledgment may precede
/// the Gateway catalog, but the run waits in the server's broker until
/// the catalog holds a chat-capable model, then binds the selection made
/// meanwhile. Transcription-only publication neither readies nor starts
/// chat.
#[tokio::test]
async fn gate_delayed_catalog_starts_chat_only_after_a_chat_model_arrives() {
    let server = spawn_chat_server(&[]).await;
    server.state.menu().set_gateway_reachable(true);
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;
    let mut workbench = JsonSocket::connect(&format!("{}/ws", server.ws_base)).await;
    let initial = workbench
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "models")
        .await;
    assert_eq!(initial["models"], json!([]));

    assert_chat_quiet(&mut socket).await;
    server.state.catalog().publish(vec![
        json!({"id": "whisper-base-en", "kind": "transcription", "object": "model"}),
        json!({"id": "whisper-small-en", "kind": "transcription", "object": "model"}),
        json!({"id": "realtime-transcribe", "kind": "transcription", "object": "model"}),
    ]);
    server.state.menu().reconcile_catalog();
    assert!(
        server.state.menu().set_selected("whisper-base-en").is_err(),
        "a transcription-only entry cannot become the selected chat binding"
    );
    let speech_only = workbench
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "models")
        .await;
    assert_eq!(
        speech_only["models"],
        json!([]),
        "the shared catalog feeding both model menus publishes no speech-only choices"
    );
    assert_chat_quiet(&mut socket).await;

    server.state.catalog().publish(vec![
        json!({"id": "whisper-base-en", "kind": "transcription", "object": "model"}),
        json!({"id": "claude-opus-4-6", "kind": "chat", "object": "model"}),
        json!({"id": "whisper-small-en", "kind": "transcription", "object": "model"}),
        json!({"id": "realtime-transcribe", "kind": "transcription", "object": "model"}),
    ]);
    server.state.menu().reconcile_catalog();
    server
        .state
        .menu()
        .set_selected("claude-opus-4-6")
        .expect("the chat model is selectable");
    let chat_only = workbench
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "models")
        .await;
    assert_eq!(
        chat_only["models"],
        json!([{"id": "claude-opus-4-6", "kind": "chat", "object": "model"}]),
        "both chat-facing choosers receive only the chat-capable model"
    );
    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "after startup").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:after startup");
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 1, "exactly one completion was dispatched");
        assert_eq!(requests[0]["model"], "claude-opus-4-6");
    }
    workbench.close().await;
    socket.close().await;
}
