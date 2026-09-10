#[tokio::test]
async fn a_live_chat_session_restarts_on_the_replacement_port_and_key() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;
    let original_wait = next_wait_token(&mut socket).await;

    let replacement_captured = CapturedRequests::default();
    let captured = Arc::clone(&replacement_captured);
    let replacement = spawn_gateway(Router::new().route(
        "/v1/chat/completions",
        post(move |headers: axum::http::HeaderMap, body: String| {
            let captured = Arc::clone(&captured);
            async move {
                if headers
                    .get(header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    != Some("Bearer replacement-key")
                {
                    return StatusCode::UNAUTHORIZED.into_response();
                }
                gate_completions(&captured, &body)
            }
        }),
    ))
    .await;
    replace_gateway(
        &gateway_updater(&server.state),
        &replacement,
        "replacement-key",
    )
        .expect("the replacement publishes");

    let replacement_wait = next_wait_token(&mut socket).await;
    assert_ne!(
        replacement_wait, original_wait,
        "the endpoint generation retires and relaunches the waiting agent"
    );
    answer(&mut socket, &replacement_wait, "after gateway recovery").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:after gateway recovery");
    assert!(
        server
            .captured
            .lock()
            .expect("the original capture lock is healthy")
            .is_empty(),
        "the old endpoint receives no post-publication completion"
    );
    assert_eq!(
        replacement_captured
            .lock()
            .expect("the replacement capture lock is healthy")
            .len(),
        1,
        "the replacement endpoint and bearer complete the next turn"
    );
    socket.close().await;
}

/// GATE 4 - restart. The persisted JSONL alone restores the transcript,
/// and the relaunched agent resumes waiting for input - the supervisor's
/// own relaunch shape driven with the log reloaded from disk. The
/// model-facing message list is the accepted interim regression: it lives
/// in the section's Lua state, so a relaunch starts it fresh until the
/// deferred persistence work lands.
#[tokio::test]
async fn gate_restart_reloads_the_jsonl_and_resumes_waiting_for_input() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let live = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&live), "echo:ping");
    socket.close().await;
    assert!(
        server.state.agents().close(&session),
        "the session ends; only the JSONL survives"
    );

    let log_path = server
        .dir
        .path()
        .join("sessions")
        .join(format!("{session}.jsonl"));
    let restored =
        Arc::new(WorkshopObserver::load_from(&log_path).expect("the persisted JSONL reloads"));
    assert_eq!(
        restored.len(),
        3,
        "the whole turn restores: input, thinking, reply - the direct \
         user_input call is not a tool call, so no tool_call_update exists"
    );
    assert_eq!(
        restored.get(0).map(|event| event.content),
        Some("ping".to_owned())
    );
    assert_eq!(
        restored.get(2).map(|event| event.content),
        Some("echo:ping".to_owned())
    );

    let mut relaunch = spawn_restored_chat(&restored, &session, &server.gateway_url);

    // The relaunched agent resumes waiting: its first act is user_input.
    let frame = tokio::time::timeout(Duration::from_secs(10), relaunch.frames.recv())
        .await
        .expect("the relaunched agent asks for input")
        .expect("the frames channel is open");
    let InputFrame::Required { token } = frame else {
        panic!("the relaunched agent must open a wait, got {frame:?}");
    };

    // The unified runtime records consumer-side, so completing the wait
    // directly is the Markdown session's accept path. Answering proves the
    // relaunch runs a full turn; the fresh message list is the regression
    // the deferred persistence work will close.
    let mut entries = restored.subscribe();
    relaunch
        .waits
        .complete(&token, "and back".to_owned())
        .expect("the wait completes");
    let reply = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let event = entries.recv().await.expect("the log broadcast stays open");
            if event.kind == RuntimeEventKind::AssistantReply {
                break event;
            }
        }
    })
    .await
    .expect("the restarted agent completes a round");
    assert_eq!(reply.content, "echo:and back");
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(requests.len(), 2);
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![pair("user", "and back")],
            "the relaunched run starts a fresh message list: history lives in the \
             section's Lua state until the deferred persistence work lands"
        );
    }
    assert_eq!(
        restored.len(),
        6,
        "the transcript itself persists: the relaunched run keeps appending \
         input, thinking, reply to the reloaded log"
    );

    // Teardown: the loop is back on user_input; cancellation ends it.
    relaunch.cancel.cancel();
    let result = relaunch.run.await.expect("the relaunched run joins");
    assert!(
        matches!(result, Err(AgentError::Interrupted)),
        "cancellation ends the relaunched run cleanly, got {result:?}"
    );
}

/// GATE 6 - error survival. Current-chat behavior: a failed completion
/// surfaces an error to the operator and the chat keeps working - the
/// behavior that replaces the relay's gateway-health short-circuit.
#[tokio::test]
async fn gate_model_failure_surfaces_an_error_and_the_next_input_works() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "fail").await;
    let error = socket
        .recv_until(Duration::from_secs(10), |frame| frame["type"] == "error")
        .await;
    assert!(
        error["message"]
            .as_str()
            .is_some_and(|message| message.contains("Model turn failed")),
        "the failed model call surfaces as an error frame naming the boundary: {error}"
    );

    // The pcall'd failure never kills the program: the loop returns to
    // user_input and the next turn is a normal one. The failed input stays
    // in the retained message list, so the projection joins the two
    // consecutive user utterances with a blank line.
    let fresh = next_wait_token(&mut socket).await;
    answer(&mut socket, &fresh, "recovered").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:fail\n\nrecovered",
        "the next input still works after the failure, with the failed input retained"
    );
    let reply = turn.events.last().expect("the recovery turn completes");
    assert_eq!(reply["event"]["content"], "echo:fail\n\nrecovered");
    socket.close().await;
}

/// GATE 7 - selection-loss recovery. A selection can vanish after the
/// browser accepted an input but before the built-in reads its fresh
/// `ui()` snapshot. The missing selection skips the model call silently -
/// no error, no request - and the loop returns to input with the accepted
/// text retained in its message list, so the next valid selection answers
/// both.
#[tokio::test]
async fn gate_selection_loss_skips_the_turn_and_recovers_after_selection() {
    let server = spawn_chat_server(&["test-model"]).await;
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
                text: "accepted before loss".to_owned(),
            },
            move || {
                state.catalog().publish(Vec::new());
                state.menu().reconcile_catalog_for_test();
            },
        )
        .expect("the launched session remains registered")
        .expect("the submitted input completes its live wait");

    // No error frame may surface (next_wait_token refuses one), no request
    // may leave: the skipped turn simply returns to input.
    let fresh = next_wait_token(&mut socket).await;
    assert_eq!(
        server
            .captured
            .lock()
            .expect("the capture lock is healthy")
            .len(),
        0,
        "a missing selection never reaches the gateway"
    );

    server
        .state
        .catalog()
        .publish(vec![json!({ "id": "test-model", "object": "model" })]);
    server
        .state
        .menu()
        .set_selected("test-model")
        .expect("the retained model can be selected for recovery");
    answer(&mut socket, &fresh, "recovered after selection").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:accepted before loss\n\nrecovered after selection",
        "the next input completes after selection becomes valid"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(
            requests.len(),
            1,
            "only the recovered turn reaches the gateway"
        );
        assert_eq!(
            role_content_pairs(&requests[0]),
            vec![pair(
                "user",
                "accepted before loss\n\nrecovered after selection"
            )],
            "the skipped input was retained in the message list; the projection joins \
             the two consecutive user utterances with a blank line"
        );
    }
    socket.close().await;
}
