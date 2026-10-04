//! Tests for the chat path: model rewriting, error diagnostics, deadlines, and decoding.

use super::*;
use crate::upstream::openai::MAX_ERROR_MESSAGE_CHARS;

#[test]
fn new_trims_a_trailing_slash_from_base_url() {
    // UP-008: the base URL is normalized so the joined path is well-formed.
    let upstream = OpenAiUpstream::new("http://host:1234/v1/", Secret::new(String::new()));
    assert_eq!(upstream.base_url, "http://host:1234/v1");
}

#[tokio::test]
async fn rewrites_caller_model_and_forwards_upstream_model() {
    // UP-008: the caller's model name is restored on the response, while the
    // upstream (backend) model is what is actually sent to the backend.
    let (base, handle) = serve_once(
        "200 OK",
        r#"{"model":"backend-model","choices":[{"index":0,"message":{"role":"assistant","content":"ok"}}]}"#,
    );
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let response = upstream
        .send(request("caller-model"), "backend-model")
        .await
        .expect("send ok");
    assert_eq!(response.model, "caller-model");
    let sent = handle.join().expect("join");
    assert!(sent.contains("POST /chat/completions"), "{sent}");
    assert!(sent.contains("backend-model"), "forwarded body: {sent}");
    assert!(
        !sent.contains("caller-model"),
        "caller model leaked: {sent}"
    );
}

#[tokio::test]
async fn non_success_status_is_upstream_status_with_capped_body() {
    // UP-008: a backend error status surfaces as UpstreamStatus.
    let (base, handle) = serve_once("500 Internal Server Error", "backend exploded");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .send(request("m"), "u")
        .await
        .expect_err("should fail");
    match err {
        ProtocolError::UpstreamStatus { status, body } => {
            assert_eq!(status, 500);
            assert_eq!(body, "backend exploded");
        }
        other => panic!("expected UpstreamStatus, got {other:?}"),
    }
    let _ = handle.join();
}

#[tokio::test]
async fn server_error_logs_structured_diagnostics_without_the_raw_body() {
    // F5: a 5xx logs at WARN with the structured status/code/type and the
    // escaped error.message, but never the raw body's other content - an
    // upstream error body can echo prompt content or credentials.
    let (logs, _guard) = capture_logs(tracing::Level::DEBUG);
    let body = r#"{"error":{"message":"line1\nline2","type":"server_error","code":"internal"},"echo":{"prompt":"BODY_ONLY_SENTINEL"}}"#;
    let (base, handle) = serve_once("500 Internal Server Error", body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let _ = upstream
        .send(request("m"), "u")
        .await
        .expect_err("5xx fails");
    let _ = handle.join();
    let logs = logs.contents();
    assert!(
        logs.lines()
            .any(|line| line.contains("WARN") && line.contains("upstream returned a server error")),
        "a 5xx logs at warn: {logs}"
    );
    assert!(logs.contains("status=500"), "status is structured: {logs}");
    assert!(logs.contains("code=internal"), "code is structured: {logs}");
    assert!(
        logs.contains("type=server_error"),
        "type is structured: {logs}"
    );
    assert!(
        logs.contains(r"line1\nline2"),
        "the error message is control-escaped: {logs}"
    );
    assert!(
        !logs.contains("BODY_ONLY_SENTINEL"),
        "unrelated body content is never logged: {logs}"
    );
}

#[tokio::test]
async fn client_error_logs_at_info_not_warn() {
    // A 4xx is the caller's error, not the backend's: it logs at info so
    // the warn stream stays reserved for server-side failures.
    let (logs, _guard) = capture_logs(tracing::Level::INFO);
    let body = r#"{"error":{"message":"unknown model","type":"invalid_request_error","code":"model_not_found"}}"#;
    let (base, handle) = serve_once("400 Bad Request", body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let _ = upstream
        .send(request("m"), "u")
        .await
        .expect_err("4xx fails");
    let _ = handle.join();
    let logs = logs.contents();
    assert!(
        logs.lines()
            .any(|line| line.contains("INFO") && line.contains("upstream returned a client error")),
        "a 4xx logs at info: {logs}"
    );
    assert!(
        !logs
            .lines()
            .any(|line| line.contains("WARN") && line.contains("upstream returned a client error")),
        "a 4xx never logs at warn: {logs}"
    );
}

#[tokio::test]
async fn non_json_error_body_is_never_logged_raw() {
    // A body outside the OpenAI error shape yields no diagnostic fields;
    // the raw body must not be logged as a fallback.
    let (logs, _guard) = capture_logs(tracing::Level::DEBUG);
    let (base, handle) = serve_once("502 Bad Gateway", "RAW_BODY_SECRET");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let _ = upstream
        .send(request("m"), "u")
        .await
        .expect_err("5xx fails");
    let _ = handle.join();
    let logs = logs.contents();
    assert!(logs.contains("status=502"), "the status still logs: {logs}");
    assert!(
        !logs.contains("RAW_BODY_SECRET"),
        "an unparseable body is never logged raw: {logs}"
    );
}

#[tokio::test]
async fn control_characters_in_the_error_message_are_escaped() {
    // A crafted message cannot forge log lines or inject terminal control:
    // newlines, carriage returns, tabs, and escape bytes are escaped.
    let (logs, _guard) = capture_logs(tracing::Level::WARN);
    let body = r#"{"error":{"message":"forged\r\nwarning: fake\t\u001b[31mred"}}"#;
    let (base, handle) = serve_once("500 Internal Server Error", body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let _ = upstream
        .send(request("m"), "u")
        .await
        .expect_err("5xx fails");
    let _ = handle.join();
    let logs = logs.contents();
    assert!(
        logs.contains(r"forged\r\nwarning: fake\t"),
        "control characters are escaped: {logs}"
    );
    assert!(
        logs.contains(r"\u{1b}[31mred"),
        "an ANSI escape is escaped: {logs}"
    );
    assert!(
        !logs.contains("forged\r\nwarning"),
        "no raw CRLF survives in the message: {logs}"
    );
    assert!(
        !logs.contains("warning: fake\t"),
        "no raw tab survives in the message: {logs}"
    );
}

#[tokio::test]
async fn an_over_long_error_message_is_bounded() {
    // The message is bounded independently of the body cap so one event
    // cannot flood the log with a huge upstream message.
    let (logs, _guard) = capture_logs(tracing::Level::WARN);
    let message = "z".repeat(MAX_ERROR_MESSAGE_CHARS + 200);
    let body = format!("{{\"error\":{{\"message\":\"{message}\"}}}}");
    let (base, handle) = serve_once("500 Internal Server Error", &body);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let _ = upstream
        .send(request("m"), "u")
        .await
        .expect_err("5xx fails");
    let _ = handle.join();
    let logs = logs.contents();
    assert!(
        logs.contains(&"z".repeat(MAX_ERROR_MESSAGE_CHARS)),
        "the bound keeps a full-length prefix"
    );
    assert!(
        !logs.contains(&"z".repeat(MAX_ERROR_MESSAGE_CHARS + 1)),
        "the message never exceeds the bound: {}",
        logs.len()
    );
}

#[tokio::test]
async fn connect_refused_is_upstream_connect_not_transport() {
    // A refused connection means the request never left the gateway:
    // nothing was billed upstream and a retry is safe, so the error must
    // classify as `upstream_connect`, distinct from a mid-flight death.
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let upstream = OpenAiUpstream::new(&format!("http://{addr}"), Secret::new(String::new()));
    let err = upstream
        .send(request("m"), "u")
        .await
        .expect_err("connect refused must fail");
    assert!(
        matches!(err, ProtocolError::UpstreamConnect(_)),
        "expected UpstreamConnect, got {err:?}"
    );
    assert_eq!(err.envelope()["error"]["code"], "upstream_connect");
}

#[tokio::test]
async fn send_times_out_on_a_stalled_server() {
    // UP-008: a backend that accepts and then stalls must fail on the
    // request deadline as a transport error, never hang the caller.
    // A timeout is NEVER connect: the request may have reached the
    // provider, so it stays `upstream_transport`.
    let (base, handle) = serve_stalled();
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(300))
        .build()
        .expect("client");
    let upstream = OpenAiUpstream::with_client(&base, Secret::new(String::new()), client);
    let err = upstream
        .send(request("m"), "u")
        .await
        .expect_err("stalled server must time out");
    assert!(
        matches!(err, ProtocolError::UpstreamTransport(_)),
        "expected UpstreamTransport, got {err:?}"
    );
    assert_eq!(err.envelope()["error"]["code"], "upstream_transport");
    let _ = handle.join();
}

#[tokio::test]
async fn error_body_is_capped_at_the_boundary() {
    // UP-008: an over-limit error body is bounded (the handler additionally
    // caps to 2000 chars); an exact-size small body is preserved whole.
    let exact = "x".repeat(64);
    let (base, handle) = serve_once("503 Service Unavailable", &exact);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream.send(request("m"), "u").await.expect_err("error");
    match err {
        ProtocolError::UpstreamStatus { status, body } => {
            assert_eq!(status, 503);
            assert_eq!(body, exact);
        }
        other => panic!("expected UpstreamStatus, got {other:?}"),
    }
    let _ = handle.join();

    // An over-2000-char error body is truncated by the handler's char cap.
    let huge = "y".repeat(5000);
    let (base, handle) = serve_once("500 Internal Server Error", &huge);
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream.send(request("m"), "u").await.expect_err("error");
    match err {
        ProtocolError::UpstreamStatus { body, .. } => {
            assert_eq!(body.chars().count(), 2000, "error body char-capped");
        }
        other => panic!("expected UpstreamStatus, got {other:?}"),
    }
    let _ = handle.join();
}

#[tokio::test]
async fn malformed_success_body_is_a_protocol_error_not_transport() {
    // UP-008: a 200 with a non-JSON body is a protocol/decode failure, not a
    // transport death (so it never triggers a spurious recovery).
    let (base, handle) = serve_once("200 OK", "definitely not json");
    let upstream = OpenAiUpstream::new(&base, Secret::new(String::new()));
    let err = upstream
        .send(request("m"), "u")
        .await
        .expect_err("should fail");
    assert!(
        matches!(err, ProtocolError::UpstreamProtocol(_)),
        "expected UpstreamProtocol, got {err:?}"
    );
    let _ = handle.join();
}
