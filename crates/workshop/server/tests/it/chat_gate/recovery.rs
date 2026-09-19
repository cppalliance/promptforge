#[tokio::test]
async fn a_live_chat_session_restarts_on_the_replacement_port_and_key() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;
    let original_wait = next_wait_token(&mut socket).await;

    let replacement_captured = CapturedRequests::default();
    let captured = Arc::clone(&replacement_captured);
    let replacement = spawn_gateway(
        Router::new()
            .route(
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
            )
            // The relaunch resolves its model through the replacement's
            // catalog; without one it binds the fallback window and the
            // chat role's minimum refuses the run.
            .route("/v1/models", get(gate_models)),
    )
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

/// GATE 7 - selection-loss recovery, unified-runtime semantics: the run's
/// model is the dropdown selection bound at launch, so a selection that
/// vanishes mid-turn no longer skips anything - the frozen binding carries
/// the raced turn to completion, and the same run keeps serving turns
/// until a catalog replacement retires it.
#[tokio::test]
async fn gate_selection_loss_leaves_the_runs_frozen_binding_untouched() {
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

    // The selection is gone, but the run's binding was frozen at launch:
    // the raced turn dispatches and completes, and no error frame surfaces.
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:accepted before loss");

    server
        .state
        .catalog()
        .publish(vec![json!({ "id": "test-model", "object": "model" })]);
    server
        .state
        .menu()
        .set_selected("test-model")
        .expect("the retained model can be selected for recovery");
    let token = wait_after(&mut socket, &turn).await;
    answer(&mut socket, &token, "recovered after selection").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(
        delta_text(&turn),
        "echo:recovered after selection",
        "the same run answers the next input on its frozen binding"
    );
    {
        let requests = server.captured.lock().expect("the capture lock is healthy");
        assert_eq!(
            requests.len(),
            2,
            "both turns reach the gateway on the frozen model"
        );
        assert_eq!(requests[0]["model"], "test-model");
        assert_eq!(requests[1]["model"], "test-model");
        assert_eq!(
            role_content_pairs(&requests[1]),
            vec![
                pair("user", "accepted before loss"),
                pair("assistant", "echo:accepted before loss"),
                pair("user", "recovered after selection"),
            ],
            "the same run retains its message list across the selection loss"
        );
    }
    socket.close().await;
}
