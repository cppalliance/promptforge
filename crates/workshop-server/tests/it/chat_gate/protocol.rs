/// GATE 2 - live streaming. Current-chat behavior: while the model
/// generates, the client sees answer text and reasoning arrive as live
/// chunks, and the completed reply supersedes them under the same id.
#[tokio::test]
async fn gate_streaming_delivers_text_and_reasoning_deltas_then_the_reply() {
    let server = spawn_chat_server(&["test-model"]).await;
    let mut socket = connect_chat(&server.ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ping").await;
    let turn = collect_turn(&mut socket).await;

    let reasoning: String = turn
        .deltas
        .iter()
        .filter(|delta| delta["kind"] == "reasoning")
        .filter_map(|delta| delta["content"].as_str())
        .collect();
    assert_eq!(
        reasoning, "mm",
        "reasoning streams live on its own side channel during generation"
    );
    assert!(
        turn.deltas
            .iter()
            .filter(|delta| delta["kind"] == "text")
            .count()
            >= 2,
        "the mock splits content, so generation provably streams in chunks"
    );
    assert_eq!(
        delta_text(&turn),
        "echo:ping",
        "the live text chunks assemble the reply"
    );

    let reply = turn
        .events
        .last()
        .expect("the turn ends with its reply event");
    assert_eq!(reply["event"]["kind"], "agent_message");
    assert_eq!(
        reply["event"]["content"], "echo:ping",
        "the completed reply arrives after the deltas it supersedes"
    );
    assert!(
        turn.deltas
            .iter()
            .all(|delta| delta["reply"] == reply["reply"]),
        "deltas and the completed reply share the superseding id"
    );
    socket.close().await;
}

/// The model-visible-input mock: the first completion request is answered
/// with a `user_input` tool call; a request carrying the tool result is
/// answered with `echo:<tool result content>`.
fn ask_then_echo_completions(captured: &CapturedRequests, body: &str) -> Response {
    let request: serde_json::Value = serde_json::from_str(body).expect("the request is JSON");
    captured
        .lock()
        .expect("the capture lock is healthy")
        .push(request.clone());
    let messages = request["messages"]
        .as_array()
        .expect("the request carries a messages array");
    let null = serde_json::Value::Null;
    let mut sse = String::new();
    if let Some(answer) = messages
        .iter()
        .rev()
        .find(|message| message["role"] == "tool")
        .and_then(|message| message["content"].as_str())
    {
        let reply = format!("echo:{answer}");
        for event in [
            sse_chunk("test-model", &json!({ "role": "assistant" }), &null),
            sse_chunk("test-model", &json!({ "content": reply }), &null),
            sse_chunk("test-model", &json!({}), &json!("stop")),
        ] {
            sse.push_str(&sse_line(&event));
        }
    } else {
        for event in [
            sse_chunk("test-model", &json!({ "role": "assistant" }), &null),
            sse_chunk(
                "test-model",
                &json!({ "tool_calls": [{
                    "index": 0,
                    "id": "call_1",
                    "type": "function",
                    "function": { "name": "user_input", "arguments": "{}" },
                }] }),
                &null,
            ),
            sse_chunk("test-model", &json!({}), &json!("tool_calls")),
        ] {
            sse.push_str(&sse_line(&event));
        }
    }
    sse.push_str("data: [DONE]\n\n");
    ([(header::CONTENT_TYPE, "text/event-stream")], sse).into_response()
}

/// GATE 11 - model-visible user input. The model can ask the operator
/// mid-loop through the broker's tool surface: its `user_input` call opens
/// the same durable wait the direct call uses, the operator's answer lands
/// as the correlated tool result, and the loop continues to a reply.
#[tokio::test]
async fn gate_model_visible_user_input_completes_the_tool_exchange() {
    let captured = CapturedRequests::default();
    let mock = Arc::clone(&captured);
    let gateway_url = spawn_gateway(Router::new().route(
        "/v1/chat/completions",
        post(move |body: String| {
            let captured = Arc::clone(&mock);
            async move { ask_then_echo_completions(&captured, &body) }
        }),
    ))
    .await;
    let (ws_base, _state, _dir) = serve_chat_over(gateway_url, &["test-model"]).await;
    let mut socket = connect_chat(&ws_base).await;
    let _session = launch_chat(&mut socket).await;

    let token = next_wait_token(&mut socket).await;
    answer(&mut socket, &token, "ask me something").await;

    // The model's tool call opens a mid-turn wait on the same broker;
    // answering it lets the loop finish the turn.
    let mut events = Vec::new();
    let mut mid_turn_wait = None;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let frame = socket.recv_json().await;
            match frame["type"].as_str() {
                Some("input_required") => {
                    let token = frame["token"]
                        .as_str()
                        .expect("the wait announces its token")
                        .to_owned();
                    mid_turn_wait = Some(token.clone());
                    answer(&mut socket, &token, "the operator's answer").await;
                }
                Some("agent_event") => {
                    let done = frame["event"]["kind"] == "agent_message";
                    events.push(frame);
                    if done {
                        break;
                    }
                }
                Some("error") => panic!("no error frame may interrupt the turn: {frame}"),
                _ => {}
            }
        }
    })
    .await
    .expect("the turn completes within the deadline");
    assert!(
        mid_turn_wait.is_some(),
        "the model's user_input call announced its own wait"
    );
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|event| event["event"]["kind"].as_str())
        .collect();
    assert_eq!(
        kinds,
        [
            "user_message",
            "tool_call",
            "user_message",
            "tool_call_update",
            "agent_message"
        ],
        "the durable record of the exchange: the turn input, the model's call, \
         the operator's mid-loop answer, the correlated result, the reply"
    );
    assert_eq!(events[0]["event"]["content"], "ask me something");
    assert_eq!(
        events[4]["event"]["content"], "echo:the operator's answer",
        "the reply follows the answered tool exchange"
    );
    socket.close().await;

    let requests = captured.lock().expect("the capture lock is healthy");
    assert_eq!(
        requests.len(),
        2,
        "the loop dispatches before and after the wait"
    );
    let second = requests[1]["messages"]
        .as_array()
        .expect("the second request carries messages");
    assert_eq!(second[1]["role"], "assistant");
    assert_eq!(
        second[1]["tool_calls"][0]["function"]["name"], "user_input",
        "the assistant record carries the model's call"
    );
    assert_eq!(second[2]["role"], "tool");
    assert_eq!(
        second[2]["tool_call_id"], "call_1",
        "the tool result is correlated to the call"
    );
    assert_eq!(
        second[2]["content"], "the operator's answer",
        "the operator's answer is the tool result, byte-exact"
    );
}
