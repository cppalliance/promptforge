//! Gateway replacement against a live agent conversation: the Harness
//! holds no gateway, so a replacement published while `chat` waits for
//! input retires nothing, and the waiting turn's round reaches the
//! replacement.

use super::*;

/// A gateway that records each completion body into `requests` and echoes
/// the last user message, with the typed catalog every test selects from.
fn recording_gateway(requests: Arc<Mutex<Vec<serde_json::Value>>>) -> Router {
    with_typed_catalog(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let requests = Arc::clone(&requests);
            async move {
                record_request(&requests, &body);
                echo_completions(body).await
            }
        }),
    ))
}

#[tokio::test]
async fn a_replacement_published_while_chat_waits_keeps_the_wait_and_answers_on_the_replacement() {
    let original_requests = Arc::new(Mutex::new(Vec::new()));
    let original = spawn_gateway(recording_gateway(Arc::clone(&original_requests))).await;
    let (base, _dir, state) = spawn_agent_server_for_gateway(original).await;
    state
        .menu()
        .set_selected("test-model")
        .expect("the retained catalog holds test-model");
    let mut socket = connect(&base).await;
    let session = launch(&mut socket, "chat").await;
    let token = next_wait_token(&mut socket).await;

    let replacement_requests = Arc::new(Mutex::new(Vec::new()));
    let replacement = spawn_gateway(recording_gateway(Arc::clone(&replacement_requests))).await;
    replace_gateway(&state, &replacement, 1_757_000_000);

    // The original token still answers: nothing cancelled the wait.
    answer(&mut socket, &token, "after replacement").await;
    let reply = socket
        .recv_until(Duration::from_secs(10), |frame| {
            assert_ne!(
                frame["type"], "input_cancelled",
                "a replacement retires no wait: {frame}"
            );
            assert_ne!(
                frame["type"], "error",
                "no error frame may interrupt: {frame}"
            );
            frame["type"] == "agent_event" && frame["event"]["kind"] == "agent_message"
        })
        .await;
    assert_eq!(reply["event"]["content"], "echo:after replacement");
    assert!(
        original_requests
            .lock()
            .expect("the request capture lock is healthy")
            .is_empty(),
        "the original gateway receives no post-publication round"
    );
    {
        let requests = replacement_requests
            .lock()
            .expect("the request capture lock is healthy");
        assert_eq!(
            requests.len(),
            1,
            "the waiting turn's round reaches the replacement"
        );
        assert_eq!(requests[0]["model"], "test-model");
    }
    assert_eq!(
        state.agents().run_id(&session),
        Some(workshop_run_log::RunId::from_raw(1)),
        "the replacement keeps the conversation's one run, the first the log began"
    );
    socket.close().await;
}
