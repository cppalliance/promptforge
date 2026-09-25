//! Gateway replacement against live agent sessions: an accepted input
//! interrupted mid-wait, the retained catalog generation replaying on
//! the replacement Gateway, and an unavailable catalog holding the
//! session instead of relaunching stale bindings.

use super::*;

#[tokio::test]
async fn gateway_replacement_interrupts_a_catalog_wait_on_accepted_input() {
    let started = Arc::new(Notify::new());
    let request_started = Arc::clone(&started);
    let original_requests = Arc::new(Mutex::new(Vec::new()));
    let captured_original = Arc::clone(&original_requests);
    let original = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let request_started = Arc::clone(&request_started);
            let captured_original = Arc::clone(&captured_original);
            async move {
                record_request(&captured_original, &body);
                hanging_completions(&request_started)
            }
        }),
    )))
    .await;
    let (base, _dir, state) = spawn_agent_server_for_gateway(original).await;
    state
        .catalog()
        .publish(vec![json!({ "id": "model-a", "object": "model" })]);
    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-a")
        .expect("the original model becomes selected");
    let mut socket = connect(&base).await;
    let session = launch_agent(&mut socket, "chat").await;
    let token = next_wait_token(&mut socket).await;

    let catalog_state = state.clone();
    state
        .agents()
        .deliver_input_after_acceptance_for_test(
            &session,
            workshop_server::InputResponse {
                token,
                text: "accepted across replacements".to_owned(),
            },
            move || {
                catalog_state.catalog().publish(vec![json!({
                    "id": "model-b",
                    "object": "model",
                })]);
            },
        )
        .expect("the session remains registered")
        .expect("the accepted input resumes its run");
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the accepted turn reaches the hanging Gateway");
    assert_eq!(
        original_requests
            .lock()
            .expect("the request capture lock is healthy")[0]["model"],
        "model-a",
        "the accepted turn reads the still-selected model-a while retirement is deferred"
    );

    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-b")
        .expect("the replacement model becomes selected");
    let replacement_requests = Arc::new(Mutex::new(Vec::new()));
    let captured_replacement = Arc::clone(&replacement_requests);
    let replacement = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let captured_replacement = Arc::clone(&captured_replacement);
            async move {
                record_request(&captured_replacement, &body);
                echo_completions(body).await
            }
        }),
    )))
    .await;
    replace_gateway(&state, &replacement, 1_757_000_000);

    let fresh = tokio::time::timeout(Duration::from_secs(10), next_wait_token(&mut socket))
        .await
        .expect("Gateway replacement overrides the catalog settlement wait");
    answer(&mut socket, &fresh, "after replacement").await;
    let turn = collect_turn(&mut socket).await;
    assert_replacement_request(&replacement_requests, "model-b", "after replacement");
    assert_eq!(
        delta_text(&turn),
        "echo:after replacement",
        "the relaunched run uses the replacement Gateway"
    );
    socket.close().await;
}

#[tokio::test]
async fn retained_catalog_generation_replays_on_the_replacement_gateway() {
    let started = Arc::new(Notify::new());
    let request_started = Arc::clone(&started);
    let original = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let request_started = Arc::clone(&request_started);
            async move {
                assert_eq!(
                    serde_json::from_str::<serde_json::Value>(&body).expect("the request is JSON")
                        ["model"],
                    "model-a"
                );
                hanging_completions(&request_started)
            }
        }),
    )))
    .await;
    let (base, _dir, state) = spawn_agent_server_for_gateway(original).await;
    state
        .catalog()
        .publish(vec![json!({ "id": "model-a", "object": "model" })]);
    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-a")
        .expect("the original model becomes selected");
    let mut socket = connect(&base).await;
    let session = launch_agent(&mut socket, "chat").await;
    let token = next_wait_token(&mut socket).await;

    let catalog_state = state.clone();
    state
        .agents()
        .deliver_input_after_acceptance_for_test(
            &session,
            workshop_server::InputResponse {
                token,
                text: "retained before replay".to_owned(),
            },
            move || {
                catalog_state
                    .catalog()
                    .publish(vec![json!({ "id": "model-b", "object": "model" })]);
            },
        )
        .expect("the session remains registered")
        .expect("the accepted input resumes its run");
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the replacement generation is observed before the old request starts");

    state
        .catalog()
        .publish(vec![json!({ "id": "model-a", "object": "model" })]);
    let replacement_requests = Arc::new(Mutex::new(Vec::new()));
    let captured_replacement = Arc::clone(&replacement_requests);
    let replacement = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let captured_replacement = Arc::clone(&captured_replacement);
            async move {
                record_request(&captured_replacement, &body);
                echo_completions(body).await
            }
        }),
    )))
    .await;
    replace_gateway(&state, &replacement, 1_757_000_001);

    let fresh = tokio::time::timeout(Duration::from_secs(10), next_wait_token(&mut socket))
        .await
        .expect("the retained generation relaunches instead of resolving stale model-b");
    answer(&mut socket, &fresh, "after retained replay").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:after retained replay");
    assert_replacement_request(&replacement_requests, "model-a", "after retained replay");
    socket.close().await;
}

#[tokio::test]
async fn unavailable_catalog_waits_without_relaunching_stale_bindings() {
    let started = Arc::new(Notify::new());
    let request_started = Arc::clone(&started);
    let original = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let request_started = Arc::clone(&request_started);
            async move { hanging_completions(&request_started) }
        }),
    )))
    .await;
    let (base, _dir, state) = spawn_agent_server_for_gateway(original).await;
    state
        .catalog()
        .publish(vec![json!({ "id": "model-a", "object": "model" })]);
    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-a")
        .expect("the original model becomes selected");
    let mut socket = connect(&base).await;
    let session = launch_agent(&mut socket, "chat").await;
    let token = next_wait_token(&mut socket).await;

    let catalog_state = state.clone();
    state
        .agents()
        .deliver_input_after_acceptance_for_test(
            &session,
            workshop_server::InputResponse {
                token,
                text: "retained while unavailable".to_owned(),
            },
            move || {
                catalog_state
                    .catalog()
                    .publish(vec![json!({ "id": "model-b", "object": "model" })]);
            },
        )
        .expect("the session remains registered")
        .expect("the accepted input resumes its run");
    tokio::time::timeout(Duration::from_secs(10), started.notified())
        .await
        .expect("the replacement generation is observed before the old request starts");

    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-b")
        .expect("the pending replacement model becomes selected");
    state.catalog().publish(Vec::new());
    let replacement_requests = Arc::new(Mutex::new(Vec::new()));
    let captured_replacement = Arc::clone(&replacement_requests);
    let replacement = spawn_gateway(with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let captured_replacement = Arc::clone(&captured_replacement);
            async move {
                record_request(&captured_replacement, &body);
                echo_completions(body).await
            }
        }),
    )))
    .await;
    let mut lifecycle = state
        .registry()
        .state::<harness_api::Harness>()
        .expect("the harness is registered")
        .session(&harness_api::SessionId::new(&session))
        .expect("the session remains registered")
        .subscribe_state();
    replace_gateway(&state, &replacement, 1_757_000_002);

    // The supervisor marks the retired run Closed and executes a relaunch
    // in one synchronous step on this single-threaded runtime, so Closed
    // is observable only when the session holds.
    tokio::time::timeout(
        Duration::from_secs(10),
        lifecycle.wait_for(|run| *run == harness_api::SessionState::Closed),
    )
    .await
    .expect("an unavailable catalog cannot relaunch model-b on the replacement Gateway")
    .expect("the session remains registered");
    assert_eq!(
        state.agents().unresolved_waits(&session),
        Some(Vec::new()),
        "the held session stays registered with no open wait"
    );

    state
        .catalog()
        .publish(vec![json!({ "id": "model-c", "object": "model" })]);
    state.menu().reconcile_catalog();
    state
        .menu()
        .set_selected("model-c")
        .expect("the newly available model becomes selected");
    let fresh = tokio::time::timeout(Duration::from_secs(10), next_wait_token(&mut socket))
        .await
        .expect("a later usable catalog relaunches the waiting session");
    answer(&mut socket, &fresh, "after unavailable").await;
    let turn = collect_turn(&mut socket).await;
    assert_eq!(delta_text(&turn), "echo:after unavailable");
    assert_replacement_request(&replacement_requests, "model-c", "after unavailable");
    socket.close().await;
}
