//! Tests for the speech passthrough: byte streaming, split deadlines, and the declining default.

use super::*;

fn speech_request(model: &str) -> SpeechRequest {
    SpeechRequest {
        model: model.to_owned(),
        input: "hello world".to_owned(),
        voice: crate::wire::SpeechVoice::Name("tara".to_owned()),
        response_format: crate::wire::SpeechResponseFormat::Mp3,
        speed: None,
        instructions: None,
        stream_format: None,
        rest: Map::new(),
    }
}

/// A one-shot mock audio backend: serves a single canned binary body with
/// the given `Content-Type` (or none) and returns its base URL plus the
/// captured raw request for assertions.
fn serve_audio(content_type: Option<&str>, body: &[u8]) -> (String, JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock audio backend");
    let addr = listener.local_addr().expect("addr");
    let content_type = content_type.map(str::to_owned);
    let body = body.to_vec();
    let handle = thread::spawn(move || -> String {
        let (mut stream, _) = listener.accept().expect("accept");
        // Same bounded-capture pattern as serve_once: the read timeout ends
        // the wait once the client is awaiting a response.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut request = Vec::new();
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => request.extend_from_slice(&buf[..n]),
            }
        }
        let content_type = content_type
            .map(|value| format!("Content-Type: {value}\r\n"))
            .unwrap_or_default();
        let head = format!(
            "HTTP/1.1 200 OK\r\n{content_type}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
        let _ = stream.flush();
        String::from_utf8_lossy(&request).into_owned()
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn speech_rewrites_caller_model_and_posts_to_audio_speech() {
    // UP-008: same contract as chat - the upstream model is what the
    // backend sees; the caller's model name never leaks into the body.
    let (base, handle) = serve_audio(Some("audio/mpeg"), b"fake-mp3");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let streamed = upstream
        .send_speech(speech_request("caller-model"), "backend-tts")
        .await
        .expect("send ok");
    assert_eq!(streamed.content_type, "audio/mpeg");
    let sent = handle.join().expect("join");
    assert!(sent.contains("POST /audio/speech"), "{sent}");
    assert!(sent.contains("backend-tts"), "forwarded body: {sent}");
    assert!(
        sent.contains("\"voice\":\"tara\""),
        "forwarded body: {sent}"
    );
    assert!(
        !sent.contains("caller-model"),
        "caller model leaked: {sent}"
    );
}

#[tokio::test]
async fn speech_forwards_the_bearer_credential() {
    // The endpoint credential is sent on the audio request exactly as it
    // is on the chat and embeddings requests.
    let (base, handle) = serve_audio(Some("audio/mpeg"), b"fake-mp3");
    let upstream = OpenAiUpstream::new(&base, Secret::new("test-key".to_owned()));
    let _streamed = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect("send ok");
    let sent = handle.join().expect("join").to_ascii_lowercase();
    assert!(
        sent.contains("authorization: bearer test-key"),
        "bearer forwarded: {sent}"
    );
}

#[tokio::test]
async fn speech_non_success_status_is_upstream_status() {
    // A backend 429/503 surfaces as the protocol-level UpstreamStatus
    // shape with the capped body - never a client-facing envelope (the
    // route owns the envelope mapping).
    for (status_line, status) in [
        ("429 Too Many Requests", 429),
        ("503 Service Unavailable", 503),
    ] {
        let (base, handle) = serve_once(status_line, "backend says no");
        let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
        let err = upstream
            .send_speech(speech_request("m"), "u")
            .await
            .expect_err("should fail");
        match err {
            ProtocolError::UpstreamStatus { status: got, body } => {
                assert_eq!(got, status);
                assert_eq!(body, "backend says no");
            }
            other => panic!("expected UpstreamStatus {status}, got {other:?}"),
        }
        let _ = handle.join();
    }
}

#[tokio::test]
async fn speech_streams_bytes_untransformed() {
    // Byte passthrough: audio frames are opaque, so the body stream is the
    // upstream's bytes verbatim - including invalid UTF-8, which no
    // decoding layer ever touches.
    let body: Vec<u8> = (0..=255).cycle().take(4097).collect();
    let (base, handle) = serve_audio(Some("audio/mpeg"), &body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect("send ok");
    assert_eq!(streamed.content_type, "audio/mpeg");
    let mut collected = Vec::new();
    while let Some(item) = streamed.body.next().await {
        collected.extend_from_slice(&item.expect("chunk ok"));
    }
    assert_eq!(collected, body, "audio bytes pass through untransformed");
    let _ = handle.join();
}

#[tokio::test]
async fn speech_content_type_is_empty_when_the_upstream_omits_it() {
    // An upstream without a Content-Type yields an empty string, the
    // route's signal to apply its format-to-MIME fallback.
    let (base, handle) = serve_audio(None, b"raw-audio");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect("send ok");
    assert_eq!(streamed.content_type, "");
    while let Some(item) = streamed.body.next().await {
        item.expect("chunk ok");
    }
    let _ = handle.join();
}

#[tokio::test]
async fn speech_times_out_on_a_stalled_server() {
    // A backend that accepts and then stalls must fail as a transport
    // error on the read deadline, never hang the caller. A timeout is
    // NEVER connect: the request may have reached the provider.
    let (base, handle) = serve_stalled();
    let client = reqwest::Client::builder()
        .read_timeout(std::time::Duration::from_millis(300))
        .build()
        .expect("client");
    let upstream = OpenAiUpstream::with_client(&base, Secret::new(String::new()), client);
    let err = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect_err("stalled server must time out");
    assert!(
        matches!(err, ProtocolError::UpstreamTransport(_)),
        "expected UpstreamTransport, got {err:?}"
    );
    let _ = handle.join();
}

/// Between the two test-scaled speech deadlines: past the per-read body
/// idle budget (200 ms) so the accept arm proves that budget does not
/// govern time-to-headers, within the first-response budget (1 s) so
/// the request is still accepted.
const SLOW_HEADER_PAUSE: Duration = Duration::from_millis(500);

/// Past the test-scaled first-response budget (1 s) by a wide margin.
/// The mock still answers eventually, so only the deadline can fail the
/// request: removing the deadline turns the reject arm back into an
/// accepted response and the test red.
const STALLED_HEADER_PAUSE: Duration = Duration::from_millis(3000);

/// A mock audio backend that waits `pause` after the request arrives
/// before sending any headers, then serves the canned body: the
/// slow-headers arm of the deadline-separation pair.
fn serve_slow_headers(pause: Duration, body: &[u8]) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock audio backend");
    let addr = listener.local_addr().expect("addr");
    let body = body.to_vec();
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        // Same bounded-capture pattern as serve_once: the read timeout
        // ends the wait once the client is awaiting a response.
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        thread::sleep(pause);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(&body);
        let _ = stream.flush();
    });
    (format!("http://{addr}"), handle)
}

/// A mock audio backend that sends headers and a first body chunk, then
/// holds the stream open without sending more: the stalled-body arm of
/// the deadline-separation pair. The thread exits as soon as the client
/// hangs up, bounded by a read timeout.
fn serve_chunk_then_stall(chunk: &[u8]) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock audio backend");
    let addr = listener.local_addr().expect("addr");
    let chunk = chunk.to_vec();
    let handle = thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut buf = [0_u8; 4096];
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
        }
        // No Content-Length: the body stays open until close, so the
        // client must detect the stall itself.
        let head = "HTTP/1.1 200 OK\r\nContent-Type: audio/mpeg\r\n\r\n";
        if stream
            .write_all(head.as_bytes())
            .and_then(|()| stream.write_all(&chunk))
            .and_then(|()| stream.flush())
            .is_err()
        {
            return;
        }
        // Hold the body open until the client hangs up (EOF or reset);
        // the read timeout bounds the wait if it never does.
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        loop {
            match stream.read(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
        }
    });
    (format!("http://{addr}"), handle)
}

#[tokio::test]
async fn speech_slow_headers_within_the_first_response_budget_are_accepted() {
    // Deadline separation: headers arriving after the per-read body idle
    // budget but within the first-response budget are accepted. Before
    // the split, the audio client's `read_timeout` also governed the
    // header wait and killed this request at the body-idle deadline.
    let (base, handle) = serve_slow_headers(SLOW_HEADER_PAUSE, b"fake-mp3");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect("slow headers within the first-response budget are accepted");
    assert_eq!(streamed.content_type, "audio/mpeg");
    let mut collected = Vec::new();
    while let Some(item) = streamed.body.next().await {
        collected.extend_from_slice(&item.expect("chunk ok"));
    }
    assert_eq!(collected, b"fake-mp3");
    handle.join().expect("join");
}

#[tokio::test]
async fn speech_headers_stalling_past_the_first_response_budget_fail() {
    // The reject arm of the first-response split: headers arriving past
    // the FIRST_RESPONSE_TIMEOUT budget fail the request as a transport
    // error, never hang the caller. A timeout is NEVER connect: the
    // request may have reached the provider. The mock answers after
    // STALLED_HEADER_PAUSE, so only the deadline can produce the error.
    let (base, handle) = serve_slow_headers(STALLED_HEADER_PAUSE, b"fake-mp3");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect_err("headers past the first-response budget must fail");
    assert!(
        matches!(err, ProtocolError::UpstreamTransport(_)),
        "expected UpstreamTransport, got {err:?}"
    );
    assert_eq!(err.envelope()["error"]["code"], "upstream_transport");
    let _ = handle.join();
}

#[tokio::test]
async fn speech_opened_body_stalling_past_the_read_idle_budget_fails() {
    // The other half of the split: once headers arrive, the per-read
    // idle budget guards the body. A body that stalls past it fails the
    // stream with a transport error, never a clean EOF.
    let (base, handle) = serve_chunk_then_stall(b"first-chunk");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let mut streamed = upstream
        .send_speech(speech_request("m"), "u")
        .await
        .expect("headers arrive promptly");
    let first = streamed.body.next().await.expect("first chunk");
    assert!(first.is_ok(), "first chunk arrives: {first:?}");
    let second = streamed
        .body
        .next()
        .await
        .expect("the stall surfaces as an item");
    assert!(
        matches!(second, Err(ProtocolError::UpstreamTransport(_))),
        "a stalled body fails as transport, got {second:?}"
    );
    assert!(
        streamed.body.next().await.is_none(),
        "the stream ends after the error item"
    );
    // Joined off the runtime thread so the client connection task can
    // run the close (same pattern as the drop-cancellation test).
    drop(streamed);
    tokio::task::spawn_blocking(move || handle.join().expect("join"))
        .await
        .expect("watch task");
}

#[tokio::test]
async fn default_send_speech_is_model_unavailable_and_object_safe() {
    // Upstreams without a speech implementation decline the workload with
    // ModelUnavailable naming the caller's model. The call goes through
    // `Arc<dyn Upstream>` to prove the signature stays object-safe.
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
        .send_speech(speech_request("local-tts"), "ignored-alias")
        .await
    {
        Err(ProtocolError::ModelUnavailable(model)) => assert_eq!(model, "local-tts"),
        Err(other) => panic!("expected ModelUnavailable, got {other:?}"),
        Ok(_) => panic!("default must decline"),
    }
}
