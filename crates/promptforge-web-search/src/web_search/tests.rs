use super::{
    MAX_COUNT, MAX_DOMAINS, MAX_ERROR_BODY, MAX_QUERY_LEN, MAX_RESPONSE_BODY, MAX_STRING_LEN,
    WebSearch,
};
use promptforge_tools::{OutputTrust, Tool, ToolErrorKind, ToolId};

use std::net::SocketAddr;
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;
use serde_json::Value;

/// A mock gateway whose task is owned by the test: dropping it aborts the
/// server task deterministically instead of leaking a detached task.
struct MockServer {
    addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Binds an ephemeral port, serves `router`, and returns the address.
    async fn spawn(router: Router) -> MockServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        MockServer { addr, handle }
    }

    fn url(&self) -> String {
        format!("http://{}", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// A router serving the canned success result at the tool's endpoint.
fn success_router() -> Router {
    async fn web_search(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            auth, "Bearer tok",
            "expected the bearer token to be forwarded"
        );
        assert_eq!(
            body.get("query").and_then(Value::as_str),
            Some("hi"),
            "expected the validated query to be forwarded in the body"
        );
        Json(serde_json::json!({
            "results": [
                { "title": "T", "url": "https://e.com", "description": "D" }
            ]
        }))
    }
    Router::new().route("/tools/web_search", post(web_search))
}

#[test]
fn debug_never_leaks_the_bearer_token() {
    let tool = WebSearch::new("http://localhost", "super-secret-token")
        .expect("valid web search configuration");
    let rendered = format!("{tool:?}");
    assert!(
        !rendered.contains("super-secret-token"),
        "the bearer token must never appear in Debug output, got: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "the token field must be redacted, got: {rendered}"
    );
}

#[test]
fn descriptor_is_stable_and_faithful() {
    let tool = WebSearch::new("http://localhost", "test").expect("valid web search configuration");

    assert_eq!(
        tool.id(),
        ToolId::new("promptforge", "web_search").expect("valid id")
    );
    assert_eq!(tool.wire_name(), "web_search");
    assert_eq!(
        tool.description(),
        "Search the web and return a list of results (title, url, description)."
    );
    assert_eq!(
        tool.parameters_schema(),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query.",
                    "minLength": 1,
                    "maxLength": MAX_QUERY_LEN
                },
                "count": {
                    "type": "integer",
                    "description": "Max number of results.",
                    "minimum": 1,
                    "maximum": MAX_COUNT
                },
                "freshness": {
                    "type": "string",
                    "description": "Freshness filter.",
                    "enum": ["pd", "pw", "pm", "py"]
                },
                "country": {
                    "type": "string",
                    "description": "Country code for the search.",
                    "maxLength": MAX_STRING_LEN
                },
                "search_lang": {
                    "type": "string",
                    "description": "Search language code.",
                    "maxLength": MAX_STRING_LEN
                },
                "safesearch": {
                    "type": "string",
                    "description": "SafeSearch level.",
                    "enum": ["off", "moderate", "strict"]
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Only keep results from these hostnames."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Drop results from these hostnames."
                }
            },
            "required": ["query"]
        })
    );
}

#[tokio::test]
async fn forwards_query_and_returns_untrusted_results() {
    let mock = MockServer::spawn(success_router()).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let raw = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect("call should succeed");

    assert_eq!(
        raw.trust(),
        OutputTrust::Untrusted,
        "external search content must be marked untrusted"
    );
    let parsed: Value = serde_json::from_str(raw.text()).expect("response should be valid JSON");
    assert_eq!(
        parsed["results"][0]["title"].as_str(),
        Some("T"),
        "expected the canned result title to survive the round-trip"
    );
}

#[tokio::test]
async fn forwards_validated_optional_fields() {
    async fn web_search(Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(body.get("count").and_then(Value::as_u64), Some(5));
        assert_eq!(body.get("freshness").and_then(Value::as_str), Some("pw"));
        assert_eq!(
            body.get("safesearch").and_then(Value::as_str),
            Some("strict")
        );
        assert_eq!(
            body.get("include_domains"),
            Some(&serde_json::json!(["example.com"]))
        );
        Json(serde_json::json!({ "results": [{ "url": "https://e.com" }] }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    tool.call(serde_json::json!({
        "query": "hi",
        "count": 5,
        "freshness": "pw",
        "safesearch": "strict",
        "include_domains": ["example.com"]
    }))
    .await
    .expect("a fully-specified valid request should succeed");
}

#[tokio::test]
async fn rejects_missing_query() {
    let tool = WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
    let err = tool
        .call(serde_json::json!({ "count": 3 }))
        .await
        .expect_err("missing query should be rejected before any network call");
    assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
}

#[tokio::test]
async fn rejects_empty_and_oversized_query() {
    let tool = WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
    assert_eq!(
        tool.call(serde_json::json!({ "query": "   " }))
            .await
            .expect_err("blank query")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    let long = "x".repeat(MAX_QUERY_LEN + 1);
    assert_eq!(
        tool.call(serde_json::json!({ "query": long }))
            .await
            .expect_err("oversized query")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
}

#[tokio::test]
async fn rejects_unknown_fields_and_bad_optional_types() {
    let tool = WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
    // Unknown field.
    let err = tool
        .call(serde_json::json!({ "query": "hi", "nonsense": 1 }))
        .await
        .expect_err("unknown field must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    assert!(
        std::error::Error::source(&err).is_some(),
        "a deserialization failure must preserve its serde source"
    );
    // Wrong type for count.
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "count": "five" }))
            .await
            .expect_err("count must be an integer")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    // Out-of-range count.
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "count": MAX_COUNT + 1 }))
            .await
            .expect_err("count above the cap")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "count": 0 }))
            .await
            .expect_err("zero count")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    // Unknown enum values.
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "freshness": "yesterday" }))
            .await
            .expect_err("unknown freshness")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "safesearch": "maybe" }))
            .await
            .expect_err("unknown safesearch")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
}

#[tokio::test]
async fn rejects_invalid_domain_lists() {
    let tool = WebSearch::new("http://127.0.0.1:0", "tok").expect("valid web search configuration");
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "include_domains": ["ok.com", "bad/host"] }))
            .await
            .expect_err("a hostname with a separator must be rejected")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
    let many: Vec<String> = (0..30).map(|i| format!("h{i}.com")).collect();
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi", "exclude_domains": many }))
            .await
            .expect_err("too many hostnames must be rejected")
            .kind(),
        ToolErrorKind::InvalidArguments
    );
}

#[test]
fn constructor_rejects_bad_urls_credentials_query_and_empty_token() {
    assert!(WebSearch::new("not-a-url", "tok").is_err(), "invalid URL");
    assert!(WebSearch::new("", "tok").is_err(), "empty URL");
    assert!(
        WebSearch::new("ftp://host/v1", "tok").is_err(),
        "non-http scheme"
    );
    assert!(
        WebSearch::new("http://user:pass@host/v1", "tok").is_err(),
        "embedded credentials must be rejected"
    );
    assert!(
        WebSearch::new("http://host/v1?q=1", "tok").is_err(),
        "a query component must be rejected"
    );
    assert!(
        WebSearch::new("http://host/v1#frag", "tok").is_err(),
        "a fragment must be rejected"
    );
    assert!(
        WebSearch::new("http://localhost", "").is_err(),
        "empty token must be rejected"
    );
    assert!(WebSearch::new("http://localhost", "tok").is_ok());
}

#[test]
fn constructor_errors_preserve_sources_without_leaking_secrets() {
    let err = WebSearch::new("not-a-url", "tok").expect_err("invalid URL must be rejected");
    assert!(
        std::error::Error::source(&err).is_some(),
        "the endpoint parse failure must be preserved as the source"
    );
    let err = WebSearch::new("http://user:pass@host/v1", "tok")
        .expect_err("embedded credentials must be rejected");
    let rendered = format!("{err:?}");
    assert!(
        !rendered.contains("user:pass@host"),
        "the rejected URL must not be echoed into diagnostics: {rendered}"
    );
    let err = WebSearch::new("http://localhost", "").expect_err("empty token must be rejected");
    assert!(
        std::error::Error::source(&err).is_some(),
        "the empty-token failure must be preserved as the source"
    );
}

#[tokio::test]
async fn transport_failure_is_transport_kind() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let tool =
        WebSearch::new(&format!("http://{addr}"), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a refused connection must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert!(std::error::Error::source(&err).is_some());
}

#[tokio::test]
async fn stalling_gateway_times_out_as_transport() {
    async fn web_search() -> Json<Value> {
        tokio::time::sleep(Duration::from_secs(30)).await;
        Json(serde_json::json!({ "results": [] }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::with_timeout(&mock.url(), "tok", Duration::from_millis(200))
        .expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a stalled gateway must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert!(
        std::error::Error::source(&err).is_some(),
        "the timeout must be preserved as the error's transport source"
    );
}

#[tokio::test]
async fn malformed_success_json_is_backend_error_with_source() {
    async fn web_search() -> Json<Value> {
        // Missing the required `results` array: valid JSON, wrong shape.
        Json(serde_json::json!({ "unexpected": true }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a wrong-shaped success body must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        std::error::Error::source(&err).is_some(),
        "a malformed response must preserve its parse source"
    );
}

#[tokio::test]
async fn success_body_with_empty_url_is_rejected() {
    async fn web_search() -> Json<Value> {
        Json(serde_json::json!({ "results": [{ "url": "" }] }))
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("an empty result url must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
}

#[tokio::test]
async fn oversized_success_body_is_rejected() {
    async fn web_search() -> String {
        "x".repeat(MAX_RESPONSE_BODY + 4096)
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("an oversized success body must be rejected, not truncated");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        err.to_string().contains("exceeded"),
        "the error must name the cap overflow: {err}"
    );
}

#[tokio::test]
async fn oversized_error_body_is_bounded_and_sanitized() {
    async fn web_search() -> (axum::http::StatusCode, String) {
        // Oversized and control-laden so both bounding and sanitization run.
        let mut body = "line-one\nline-two\ttab".to_owned();
        body.push_str(&"e".repeat(MAX_ERROR_BODY * 4));
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body)
    }
    let mock = MockServer::spawn(Router::new().route("/tools/web_search", post(web_search))).await;
    let tool = WebSearch::new(&mock.url(), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a 500 response must surface as an error");
    let message = err.to_string();
    assert!(
        message.contains("backend returned 500"),
        "error must name the status: {message}"
    );
    assert!(
        !message.contains('\n') && !message.contains('\t'),
        "control characters must be escaped, got: {message}"
    );
    assert!(
        message.len() < MAX_ERROR_BODY + 128,
        "the error-path body must be bounded, got {} bytes",
        message.len()
    );
}

/// A raw TCP mock that promises a large body via `Content-Length`, sends a
/// few bytes, then drops the connection so the error-body read fails partway.
#[tokio::test]
async fn error_body_read_failure_is_preserved_as_source() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header = "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 100000\r\n\r\n";
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(b"partial").await;
            let _ = socket.flush().await;
        }
    });
    let tool =
        WebSearch::new(&format!("http://{addr}"), "tok").expect("valid web search configuration");

    let err = tool
        .call(serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a truncated 500 body must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert!(
        std::error::Error::source(&err).is_some(),
        "the body-read failure must be preserved as the error's source, got: {err}"
    );
    handle.abort();
}
