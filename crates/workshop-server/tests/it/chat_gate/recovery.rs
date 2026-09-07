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
    let port = url::Url::parse(&replacement)
        .expect("the replacement URL parses")
        .port()
        .expect("the replacement URL carries a port");
    gateway_updater(&server.state)
        .replace_sidecar(&shared_sidecar::ConnectionFile {
            port,
            api_key: "replacement-key".to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "test".to_owned(),
            started_at: "2026-09-07T14:14:31Z".to_owned(),
        })
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

/// GATE 4 - restart. Current-chat behavior it replaces: a conversation
/// does not die with its process. The persisted JSONL alone restores it,
/// and the relaunched agent resumes waiting for input - the supervisor's
/// own relaunch shape driven with the log reloaded from disk.
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
        4,
        "the whole conversation restores: input, tool result, thinking, reply"
    );
    assert_eq!(
        restored.get(0).map(|event| event.content),
        Some("ping".to_owned())
    );
    assert_eq!(
        restored.get(3).map(|event| event.content),
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

    // Answering proves the conversation itself was restored: the next
    // round shows the model the old exchange plus the new input.
    let mut entries = restored.subscribe();
    deliver_input_response(
        restored.as_ref(),
        &relaunch.waits,
        &session,
        "chat",
        InputResponse {
            token,
            text: "and back".to_owned(),
        },
    )
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
            vec![
                pair("user", "ping"),
                pair("assistant", "echo:ping"),
                pair("user", "and back"),
            ],
            "the reloaded JSONL alone rebuilt the conversation the model sees"
        );
    }

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
    // user_input and the next turn is a normal one.
    let fresh = next_wait_token(&mut socket).await;
    answer(&mut socket, &fresh, "recovered").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:recovered",
        "the next input still works after the failure"
    );
    let reply = turn.events.last().expect("the recovery turn completes");
    assert_eq!(reply["event"]["content"], "echo:recovered");
    socket.close().await;
}

/// GATE 7 - selection-loss recovery. A selection can vanish after the
/// browser accepted an input but before the built-in reads its fresh
/// `ui()` snapshot. The missing binding is a failed model turn, not a
/// silent pcall: one error reaches the socket, no request reaches the
/// gateway, and the loop accepts a recovery input.
#[tokio::test]
async fn gate_binding_loss_surfaces_one_error_and_recovers_after_selection() {
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

    let mut errors = Vec::new();
    let fresh = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            match frame["type"].as_str() {
                Some("error") => errors.push(frame),
                Some("input_required") => {
                    break frame["token"]
                        .as_str()
                        .expect("the recovery wait carries its token")
                        .to_owned();
                }
                _ => {}
            }
        }
    })
    .await
    .expect("the failed turn returns to input");
    assert_eq!(
        errors.len(),
        1,
        "the failed turn produces one visible error"
    );
    assert!(
        errors[0]["message"]
            .as_str()
            .is_some_and(|message| message.contains("Model turn failed")),
        "the visible error names the failed model boundary: {}",
        errors[0]
    );
    assert_eq!(
        server
            .captured
            .lock()
            .expect("the capture lock is healthy")
            .len(),
        0,
        "a missing binding never reaches the gateway"
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
        "echo:recovered after selection",
        "the next input completes after selection becomes valid"
    );
    assert_eq!(
        server
            .captured
            .lock()
            .expect("the capture lock is healthy")
            .len(),
        1,
        "only the recovered turn reaches the gateway"
    );
    socket.close().await;
}
