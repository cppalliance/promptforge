/// GATE 3 - model switch. Current-chat behavior: selecting another model
/// takes effect on the next turn, and the reply is attributed to the
/// model that produced it.
#[tokio::test]
async fn gate_model_switch_takes_effect_next_turn_with_attribution() {
    let server = spawn_chat_server(&["model-a", "model-b"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

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
        "the switch takes effect next turn; the reply event carries the new model id"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests[0]["model"], "model-a");
        assert_eq!(
            requests[1]["model"], "model-b",
            "the request itself names the newly selected model"
        );
    }
    socket.close().await;
}

/// GATE 8 - delayed startup convergence. Launch acknowledgment may precede
/// the Gateway catalog, but the run itself waits for a chat-capable model.
/// Transcription-only publication neither readies nor starts chat.
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

    assert_chat_quiet(&mut socket, Duration::from_millis(150)).await;
    server.state.catalog().publish(vec![
        json!({"id": "whisper-base-en", "kind": "transcription", "object": "model"}),
        json!({"id": "whisper-small-en", "kind": "transcription", "object": "model"}),
        json!({"id": "realtime-transcribe", "kind": "transcription", "object": "model"}),
    ]);
    server.state.menu().reconcile_catalog_for_test();
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
    assert_chat_quiet(&mut socket, Duration::from_millis(150)).await;

    server.state.catalog().publish(vec![
        json!({"id": "whisper-base-en", "kind": "transcription", "object": "model"}),
        json!({"id": "claude-opus-4-6", "kind": "chat", "object": "model"}),
        json!({"id": "whisper-small-en", "kind": "transcription", "object": "model"}),
        json!({"id": "realtime-transcribe", "kind": "transcription", "object": "model"}),
    ]);
    server.state.menu().reconcile_catalog_for_test();
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

/// GATE 9 - catalog replacement during a profile switch. The supervisor
/// relaunches on the new generation, and the relaunched run reads the new
/// selection from its fresh `ui()` snapshot. The message list starts
/// fresh: history lives in the section's Lua state until the deferred
/// persistence work lands.
#[tokio::test]
async fn gate_profile_switch_relaunches_chat_on_the_new_catalog() {
    let server = spawn_chat_server(&["model-a"]).await;
    server.state.menu().set_gateway_reachable(true);
    server.state.menu().set_profiles(
        vec!["main".to_owned(), "beta".to_owned()],
        Some("main".to_owned()),
    );
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "before switch").await;
    let first = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&first), "echo:before switch");
    let _pending_wait = wait_after(&mut socket, &first).await;

    let mut workbench = JsonSocket::connect(&format!("{}/ws", server.ws_base)).await;
    workbench
        .send_json(&json!({"type": "switch_profile", "name": "beta"}))
        .await;
    workbench
        .recv_until(Duration::from_secs(10), |frame| {
            frame["type"] == "workbench"
                && frame["active"] == "beta"
                && frame["selected"] == "model-b"
                && frame["chat_ready"] == true
        })
        .await;

    let fresh = next_wait_token(&mut socket).await;
    answer(&mut socket, &fresh, "after switch").await;
    let second = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&second), "echo:after switch");
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 2, "one completion runs on each catalog");
        assert_eq!(requests[0]["model"], "model-a");
        assert_eq!(requests[1]["model"], "model-b");
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![pair("user", "after switch")],
            "the relaunched run starts a fresh message list: history lives in the \
             section's Lua state until the deferred persistence work lands"
        );
    }
    workbench.close().await;
    socket.close().await;
}

/// GATE 10 - accepted-input replacement race. Catalog retirement waits
/// until the in-flight turn settles, then relaunches on the new
/// generation. On the unified runtime the raced turn reads its model from
/// the fresh `ui()` snapshot - the raw-id `models.get` hack - so it
/// dispatches once against the live selection and completes; the accepted
/// input is recorded exactly once.
#[tokio::test]
async fn gate_catalog_replacement_during_acceptance_settles_the_turn_exactly_once() {
    let server = spawn_chat_server(&["model-a"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let session = launch_chat(&mut socket).await;
    let token = next_wait_token(&mut socket).await;

    let state = server.state.clone();
    server
        .state
        .agents()
        .deliver_input_after_acceptance_for_test(
            &session,
            InputResponse {
                token,
                text: "accepted during replacement".to_owned(),
            },
            move || {
                state
                    .catalog()
                    .publish(vec![json!({"id": "model-b", "object": "model"})]);
                state.menu().reconcile_catalog_for_test();
                state
                    .menu()
                    .set_selected("model-b")
                    .expect("the replacement model becomes selected");
            },
        )
        .expect("the launched session remains registered")
        .expect("the accepted input resumes its original run");

    // The raced turn dispatches against the live selection and completes;
    // its settlement retires the run. Every wait the retiring run opens is
    // answered harmlessly (its run is cancelled before the answer can
    // dispatch) or cancelled outright; the relaunched run's wait runs the
    // next turn.
    let mut accepted_events = 0;
    let mut announced: Vec<String> = Vec::new();
    let second = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            match frame["type"].as_str() {
                Some("error") => panic!("the raced turn is not an error: {frame}"),
                Some("agent_event")
                    if frame["event"]["kind"] == "user_message"
                        && frame["event"]["content"] == "accepted during replacement" =>
                {
                    accepted_events += 1;
                }
                Some("input_required") => {
                    let token = frame["token"]
                        .as_str()
                        .expect("the wait carries its token")
                        .to_owned();
                    announced.push(token.clone());
                    answer(&mut socket, &token, "after replacement").await;
                }
                Some("input_cancelled") => {
                    let token = frame["token"].as_str().expect("the cancel carries its token");
                    assert!(
                        announced.iter().any(|announced| announced == token),
                        "only an announced wait is cancelled: {token}"
                    );
                }
                Some("agent_event")
                    if frame["event"]["kind"] == "agent_message"
                        && frame["event"]["content"] == "echo:after replacement" =>
                {
                    break frame;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the replacement relaunch completes a turn");
    assert_eq!(second["event"]["model"], "model-b");
    assert_eq!(accepted_events, 1, "accepted input is recorded exactly once");
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 2, "the raced turn and the recovery turn each dispatch once");
        assert_eq!(
            requests[0]["model"], "model-b",
            "the raced turn reads the live selection through the raw-id hack"
        );
        assert_eq!(
            role_content_pairs(&requests[0]),
            vec![pair("user", "accepted during replacement")],
        );
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![pair("user", "after replacement")],
            "the relaunched run starts a fresh message list"
        );
    }
    socket.close().await;
}
