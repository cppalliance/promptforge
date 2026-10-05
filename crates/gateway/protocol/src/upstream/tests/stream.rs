//! Tests for streaming chat: SSE chunk parsing, malformed-chunk skipping, and cancellation.

use super::*;

#[tokio::test]
async fn default_stream_is_model_unavailable_and_object_safe() {
    // Upstreams without a streaming implementation decline the workload
    // with ModelUnavailable naming the caller's model. The call goes
    // through `Arc<dyn Upstream>` to prove the boxed-stream signature
    // stays object-safe.
    struct ChatOnly;

    #[async_trait]
    impl Upstream for ChatOnly {
        async fn send(
            &self,
            _req: ChatRequest,
            _upstream_model: &str,
        ) -> Result<ChatResponse, ProtocolError> {
            unreachable!("not under test")
        }
    }

    let upstream: std::sync::Arc<dyn Upstream> = std::sync::Arc::new(ChatOnly);
    match upstream
        .stream(request("local-chat"), "ignored-alias")
        .await
    {
        Err(ProtocolError::ModelUnavailable(model)) => assert_eq!(model, "local-chat"),
        Err(other) => panic!("expected ModelUnavailable, got {other:?}"),
        Ok(_) => panic!("default must decline"),
    }
}

fn chunk_line(model: &str, content: &str) -> String {
    format!(
        "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"model\":\"{model}\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]}}\n\n"
    )
}

#[tokio::test]
async fn stream_parses_chunks_rewrites_model_and_stops_at_done() {
    // UP-008: same contract as `send` - the caller's model name is restored
    // on every chunk while the backend sees the upstream model, and the
    // upstream's [DONE] sentinel ends the stream without being yielded.
    let body = format!(
        "{}{}{}data: [DONE]\n\n",
        chunk_line("backend-model", "Hel"),
        chunk_line("backend-model", "lo"),
        chunk_line("backend-model", "!"),
    );
    let (base, handle) = serve_once("200 OK", &body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .stream(request("caller-model"), "backend-model")
        .await
        .expect("stream opens");
    let mut chunks = Vec::new();
    while let Some(item) = streamed.chunks.next().await {
        chunks.push(item.expect("chunk ok"));
    }
    assert_eq!(chunks.len(), 3);
    assert!(chunks.iter().all(|chunk| chunk.model == "caller-model"));
    let text: String = chunks
        .iter()
        .filter_map(|chunk| chunk.choices[0].delta.get("content"))
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(text, "Hello!");
    let sent = handle.join().expect("join");
    assert!(
        sent.contains("\"stream\":true"),
        "stream flag forwarded: {sent}"
    );
    assert!(sent.contains("backend-model"), "forwarded body: {sent}");
    assert!(
        !sent.contains("caller-model"),
        "caller model leaked: {sent}"
    );
}

#[tokio::test]
async fn stream_non_success_status_is_upstream_status_before_any_chunk() {
    // A non-2xx is consumed as a normal error before the stream starts;
    // the caller never sees a chunk stream that dies mid-flight.
    let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .stream(request("m"), "u")
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, ProtocolError::UpstreamStatus { status: 500, .. }),
        "expected UpstreamStatus 500, got {err:?}"
    );
    let _ = handle.join();
}

#[tokio::test]
async fn stream_malformed_chunks_are_logged_and_skipped() {
    // An undecodable or shape-invalid chunk is logged and skipped; the
    // stream continues with the next good chunk instead of ending.
    let (logs, _guard) = capture_warnings();
    let body = format!(
        "{}data: not json\n\ndata: {{\"model\":\"m\",\"choices\":[]}}\n\n{}data: [DONE]\n\n",
        chunk_line("m", "a"),
        chunk_line("m", "b"),
    );
    let (base, handle) = serve_once("200 OK", &body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .stream(request("m"), "u")
        .await
        .expect("stream opens");
    let mut chunks = Vec::new();
    while let Some(item) = streamed.chunks.next().await {
        chunks.push(item.expect("malformed chunks never surface as items"));
    }
    let text: String = chunks
        .iter()
        .filter_map(|chunk| chunk.choices[0].delta.get("content"))
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(text, "ab", "both good chunks survive the malformed ones");
    let logs = logs.contents();
    assert_eq!(
        logs.matches("skipping").count(),
        2,
        "each malformed chunk is logged once: {logs}"
    );
    let _ = handle.join();
}

#[tokio::test]
async fn stream_done_sentinel_is_never_logged_as_malformed() {
    // [DONE] is not JSON; it is recognized before parsing, so a healthy
    // stream ends without a spurious malformed-chunk warning.
    let (logs, _guard) = capture_warnings();
    let body = format!("{}data: [DONE]\n\n", chunk_line("m", "a"));
    let (base, handle) = serve_once("200 OK", &body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .stream(request("m"), "u")
        .await
        .expect("stream opens");
    let mut count = 0;
    while let Some(item) = streamed.chunks.next().await {
        item.expect("chunk ok");
        count += 1;
    }
    assert_eq!(count, 1);
    assert!(
        logs.contents().is_empty(),
        "a healthy stream logs no warnings: {}",
        logs.contents()
    );
    let _ = handle.join();
}

/// A mock streaming backend: answers with one chunk and no
/// `Content-Length` (so the body stays open until close), then reports
/// whether the client hung up.
fn serve_one_chunk_then_watch() -> (String, JoinHandle<bool>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock backend");
    let addr = listener.local_addr().expect("addr");
    let body = chunk_line("backend-model", "po");
    let handle = thread::spawn(move || -> bool {
        let Ok((mut stream, _)) = listener.accept() else {
            return false;
        };
        let mut buf = [0_u8; 4096];
        // Consume the request head; the read timeout ends the wait once
        // the client is awaiting a response (same pattern as serve_once).
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        if stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(body.as_bytes()))
            .and_then(|()| stream.flush())
            .is_err()
        {
            return false;
        }
        // Watch for the client hanging up: a clean EOF or a reset both
        // mean the connection is gone; a timeout means it is still open.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        loop {
            match stream.read(&mut buf) {
                Ok(0) => return true,
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    return false;
                }
                Err(_) => return true,
            }
        }
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn dropping_the_stream_aborts_the_upstream_connection() {
    // Client-disconnect cancellation is Drop all the way down: dropping
    // the chunk stream drops the upstream response, which aborts the
    // upstream connection. The watch is joined off the runtime thread so
    // the client connection task can run the close.
    let (base, handle) = serve_one_chunk_then_watch();
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .stream(request("m"), "u")
        .await
        .expect("stream opens");
    let first = streamed.chunks.next().await.expect("first item");
    assert!(first.is_ok(), "first chunk parses: {first:?}");
    drop(streamed);
    let closed = tokio::task::spawn_blocking(move || handle.join().expect("join"))
        .await
        .expect("watch task");
    assert!(
        closed,
        "dropping the stream must abort the upstream connection"
    );
}
