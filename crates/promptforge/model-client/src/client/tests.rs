use super::transport::{DEFAULT_REQUEST_TIMEOUT, escape_controls, from_env_with};
use super::*;
use std::num::NonZeroU64;

use crate::Error;
use crate::model::{CompletionErrorKind, CompletionOptions};
use serde_json::Value;

async fn client_for(app: axum::Router) -> GatewayClient {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
}

/// Renders `events` as SSE `data:` lines closed by the `[DONE]` sentinel.
fn sse_body(events: &[Value]) -> String {
    let mut body = String::new();
    for event in events {
        body.push_str("data: ");
        body.push_str(&event.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}

/// A client pointed at a mock gateway that answers every completion with
/// the given SSE body.
async fn sse_client(body: String) -> GatewayClient {
    use axum::Router;
    use axum::routing::post;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || {
            let body = body.clone();
            async move {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    body,
                )
            }
        }),
    );
    client_for(app).await
}

/// One streamed chunk carrying a content fragment.
fn content_chunk(text: &str) -> Value {
    serde_json::json!({
        "model": "qwen3-30b",
        "choices": [{ "index": 0, "delta": { "content": text }, "finish_reason": null }]
    })
}

fn lookup_from<'a>(
    pairs: &'a [(&'a str, &'a str)],
) -> impl Fn(&str) -> std::result::Result<Option<String>, Error> + 'a {
    let pairs: Vec<(String, String)> = pairs
        .iter()
        .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
        .collect();
    move |name| {
        Ok(pairs
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone()))
    }
}

#[test]
fn from_env_surfaces_non_unicode_value_instead_of_dropping_it() {
    let err = from_env_with(|name| {
        if name == "PROMPTFORGE_GATEWAY_URL" {
            Err(Error::InvalidEnv(name.to_owned()))
        } else {
            Ok(Some("tok".to_owned()))
        }
    })
    .expect_err("a non-Unicode variable must be surfaced, not treated as missing");
    assert!(
        matches!(err, Error::InvalidEnv(ref name) if name == "PROMPTFORGE_GATEWAY_URL"),
        "expected an explicit InvalidEnv error, got {err:?}"
    );
}

#[test]
fn from_env_missing_gateway_url() {
    let err = from_env_with(lookup_from(&[("PROMPTFORGE_GATEWAY_API_KEY", "tok")]))
        .expect_err("missing URL must fail");
    assert!(matches!(
        err,
        Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_URL"
    ));
}

#[test]
fn from_env_missing_gateway_key() {
    // A LAN gateway never trusts a keyless caller, so the key stays required
    // there; an empty value is the same as no value. Only the exact name
    // `localhost` is loopback: a name that merely contains it is not.
    for key_pairs in [
        vec![("PROMPTFORGE_GATEWAY_URL", "http://192.168.1.20:8081/v1")],
        vec![
            ("PROMPTFORGE_GATEWAY_URL", "http://192.168.1.20:8081/v1"),
            ("PROMPTFORGE_GATEWAY_API_KEY", ""),
        ],
        vec![("PROMPTFORGE_GATEWAY_URL", "https://gateway.example.com/v1")],
        vec![(
            "PROMPTFORGE_GATEWAY_URL",
            "http://localhost.evil.com:8081/v1",
        )],
        vec![("PROMPTFORGE_GATEWAY_URL", "http://notlocalhost:8081/v1")],
    ] {
        let err = from_env_with(lookup_from(&key_pairs))
            .expect_err("missing key against a non-loopback gateway must fail");
        assert!(
            matches!(err, Error::MissingEnv(ref name) if name == "PROMPTFORGE_GATEWAY_API_KEY"),
            "expected MissingEnv for {key_pairs:?}, got {err:?}"
        );
    }
}

#[test]
fn from_env_missing_gateway_key_is_fine_for_a_loopback_gateway() {
    // A loopback gateway trusts keyless same-machine callers by default, so
    // the key is optional for every loopback spelling; the built client is
    // the keyless one, which the Debug form cannot distinguish (no presence
    // signal leaks), so the header test below pins what it sends.
    for url in [
        "http://127.0.0.1:8081/v1",
        "http://127.0.0.2:8081/v1",
        "http://[::1]:8081/v1",
        "http://localhost:8081/v1",
        "http://LOCALHOST:8081/v1",
    ] {
        let client = from_env_with(lookup_from(&[("PROMPTFORGE_GATEWAY_URL", url)]))
            .unwrap_or_else(|err| panic!("a loopback URL needs no key, got {err:?} for {url}"));
        assert!(
            !client.has_key(),
            "the client built for {url} must carry no key"
        );
        let empty_key = from_env_with(lookup_from(&[
            ("PROMPTFORGE_GATEWAY_URL", url),
            ("PROMPTFORGE_GATEWAY_API_KEY", ""),
        ]))
        .unwrap_or_else(|err| panic!("an empty key on loopback is unset, got {err:?} for {url}"));
        assert!(!empty_key.has_key());
    }
    let keyed = from_env_with(lookup_from(&[
        ("PROMPTFORGE_GATEWAY_URL", "http://127.0.0.1:8081/v1"),
        ("PROMPTFORGE_GATEWAY_API_KEY", "tok"),
    ]))
    .expect("a loopback URL with a key builds");
    assert!(
        keyed.has_key(),
        "a key that is set is kept even on loopback"
    );
}

/// Spawns a gateway that records the `Authorization` header of each
/// completion request (as `Some(value)` or `None`) and answers a minimal
/// stop-finished stream, returning its `/v1` base and the capture slot.
async fn spawn_auth_capturing_gateway() -> (
    String,
    std::sync::Arc<std::sync::Mutex<Option<Option<String>>>>,
) {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::http::HeaderMap;
    use axum::routing::post;

    let captured: Arc<Mutex<Option<Option<String>>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap| {
            let slot = Arc::clone(&slot);
            async move {
                let auth = headers
                    .get(axum::http::header::AUTHORIZATION)
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                *slot.lock().expect("capture lock") = Some(auth);
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    sse_body(&[
                        content_chunk("ok"),
                        serde_json::json!({
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
                        }),
                    ]),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{addr}/v1"), captured)
}

#[tokio::test]
async fn keyless_client_sends_no_authorization_header() {
    // The gateway's loopback trust admits only a request with NO
    // Authorization header at all - a presented-but-wrong bearer is still
    // 401 - so a keyless client must omit the header, not send an empty one.
    let (base, captured) = spawn_auth_capturing_gateway().await;
    let client = GatewayClient::keyless(GatewayEndpoint::new(&base).expect("valid endpoint"));
    client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("the keyless completion succeeds");
    let seen = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("the gateway saw the request");
    assert_eq!(
        seen, None,
        "a keyless client must send no Authorization header, got {seen:?}"
    );
}

#[tokio::test]
async fn keyed_client_still_sends_the_bearer_header() {
    let (base, captured) = spawn_auth_capturing_gateway().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("the keyed completion succeeds");
    let seen = captured
        .lock()
        .expect("capture lock")
        .clone()
        .expect("the gateway saw the request");
    assert_eq!(seen.as_deref(), Some("Bearer tok"));
}

#[test]
fn keyless_client_debug_is_indistinguishable_from_a_keyed_one() {
    // No presence signal leaks through Debug either way.
    let keyless =
        GatewayClient::keyless(GatewayEndpoint::new("http://127.0.0.1:8081/v1").expect("valid"));
    let rendered = format!("{keyless:?}");
    assert!(rendered.contains("<redacted>"), "got: {rendered}");
    assert!(!rendered.contains("None"), "got: {rendered}");
}

#[test]
fn from_validated_parts_serializes_role_and_content_verbatim() {
    // The agent-executor seam: a `system` role and a content-parts array must
    // reach the wire exactly as validated, and the inherent constructors'
    // string form must stay byte-identical to the pre-seam shape.
    let parts = serde_json::json!([
        { "type": "text", "text": "look at this" },
        { "type": "image_url", "image_url": { "url": "data:image/png;base64,AAAA" } },
    ]);
    let multimodal = Message::from_validated_parts("user", parts.clone(), None, None);
    assert_eq!(
        serde_json::to_value(&multimodal).expect("a message must serialize"),
        serde_json::json!({ "role": "user", "content": parts }),
    );
    assert_eq!(
        multimodal.content(),
        "",
        "parts content has no text form; the accessor reports empty"
    );
    let system =
        Message::from_validated_parts("system", Value::String("be terse".to_owned()), None, None);
    assert_eq!(
        serde_json::to_value(&system).expect("a message must serialize"),
        serde_json::json!({ "role": "system", "content": "be terse" }),
    );
    assert_eq!(system.content(), "be terse");
    assert_eq!(
        serde_json::to_value(Message::user("hello")).expect("a message must serialize"),
        serde_json::json!({ "role": "user", "content": "hello" }),
        "the plain constructors keep their wire shape"
    );
}

#[test]
fn debug_redacts_the_bearer_key_and_never_leaks_it() {
    let client = GatewayClient::new(
        GatewayEndpoint::new("http://127.0.0.1:8081/v1").expect("valid test endpoint"),
        SecretString::new("super-secret-token").expect("non-empty test key"),
    );
    let rendered = format!("{client:?}");
    assert!(
        !rendered.contains("super-secret-token"),
        "the bearer key must never appear in Debug output, got: {rendered}"
    );
    assert!(
        rendered.contains("<redacted>"),
        "the key field must be redacted, got: {rendered}"
    );
    assert!(
        rendered.contains("http://127.0.0.1:8081/v1"),
        "the base URL is not a secret and should still appear, got: {rendered}"
    );
}

#[test]
fn secret_string_never_prints_its_contents() {
    let secret = SecretString::new("super-secret-token").expect("non-empty test key");
    assert_eq!(format!("{secret:?}"), "SecretString(<redacted>)");
    assert_eq!(format!("{secret}"), "<redacted>");
    assert_eq!(secret.expose(), "super-secret-token");
}

#[test]
fn tool_arguments_view_exposes_no_raw_value() {
    // F8: the public arguments view surfaces typed accessors, never a
    // serde_json::Value.
    let call = ToolCall {
        id: "call_1".to_owned(),
        name: "search".to_owned(),
        arguments: serde_json::json!({"query": "rust", "limit": 5}),
    };
    let args = call.arguments();
    assert!(!args.is_empty());
    assert!(args.contains("query"));
    assert!(!args.contains("absent"));
    let mut names: Vec<_> = args.names().collect();
    names.sort_unstable();
    assert_eq!(names, ["limit", "query"]);
    let json = args.to_json_string();
    assert!(json.contains("\"query\":\"rust\""), "got {json}");

    // A null payload reads as empty.
    let empty = ToolCall {
        id: "c2".to_owned(),
        name: "noop".to_owned(),
        arguments: Value::Null,
    };
    assert!(empty.arguments().is_empty());
    assert!(!empty.arguments().contains("anything"));
}

#[test]
fn tool_schema_new_validates_wire_name_and_object_schema() {
    // F7: a valid name and object schema are accepted.
    let schema =
        ToolSchema::new("web.search-1", "desc", json_object()).expect("a valid schema is accepted");
    assert_eq!(schema.name, "web.search-1");
    // An empty or malformed name is rejected.
    assert!(matches!(
        ToolSchema::new("", "d", json_object()),
        Err(ToolSchemaError::InvalidName { .. })
    ));
    assert!(matches!(
        ToolSchema::new("bad name", "d", json_object()),
        Err(ToolSchemaError::InvalidName { .. })
    ));
    // A non-object JSON Schema is rejected.
    assert!(matches!(
        ToolSchema::new("ok", "d", serde_json::json!([1, 2, 3])),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
    assert!(matches!(
        ToolSchema::new("ok", "d", serde_json::json!("scalar")),
        Err(ToolSchemaError::NonObjectSchema { .. })
    ));
}

fn json_object() -> Value {
    serde_json::json!({"type": "object", "properties": {}})
}

#[test]
fn escape_controls_neutralizes_control_bytes_and_bounds_length() {
    // F5: newlines and other control characters are escaped, not passed
    // through, so a body cannot forge log lines.
    let escaped = escape_controls("line1\nline2\r\u{7}end", 2000);
    assert!(!escaped.contains('\n'), "raw newline must be escaped");
    assert!(
        !escaped.contains('\r'),
        "raw carriage return must be escaped"
    );
    assert!(
        escaped.contains("\\n"),
        "escaped newline expected, got {escaped}"
    );
    assert_eq!(escape_controls("", 2000), "(empty body)");
    assert_eq!(escape_controls("abcdef", 3), "abc");
}

#[tokio::test]
async fn backend_error_display_is_body_free_and_body_is_opt_in_and_escaped() {
    use axum::Router;
    use axum::routing::post;

    // A non-success body carrying control characters and a would-be secret.
    async fn handler() -> (axum::http::StatusCode, String) {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            "forged\nlog: super-secret".to_owned(),
        )
    }
    let app = Router::new().route("/v1/chat/completions", post(handler));
    let client = client_for(app).await;
    let options = CompletionOptions::new("m");
    let err = client
        .complete(&[Message::user("hi")], None, &options, |_| {})
        .await
        .expect_err("a 502 must surface as a backend error");

    // F5: the public Display names only the status, never the raw body.
    let shown = err.to_string();
    assert!(shown.contains("502"), "status must appear, got {shown}");
    assert!(
        !shown.contains("super-secret") && !shown.contains('\n'),
        "the raw body must not ride in Display, got {shown}"
    );
    // The bounded, control-escaped body is available only via the opt-in.
    let body = err
        .backend_body()
        .expect("backend body is available opt-in");
    assert!(
        body.contains("\\n"),
        "control chars must be escaped, got {body}"
    );
    assert!(
        !body.contains('\n'),
        "no raw newline in the diagnostic body"
    );
}

#[test]
fn gateway_endpoint_rejects_non_http_schemes_and_missing_host() {
    for url in ["ftp://example.com/v1", "not-a-url", "http://", ""] {
        let error = GatewayEndpoint::new(url).expect_err("invalid endpoint must be rejected");
        assert_eq!(error.kind(), CompletionErrorKind::Config);
        assert!(!error.to_string().contains("missing environment variable"));
    }
}

#[test]
fn gateway_endpoint_rejects_credentials_query_and_fragment() {
    // F12: the strict URL parse rejects embedded credentials and the
    // query/fragment ambiguity a hand-rolled prefix scan let through.
    for url in [
        "http://user:pass@host/v1",
        "http://user@host/v1",
        "http://host/v1?token=leak",
        "http://host/v1#frag",
    ] {
        let error = GatewayEndpoint::new(url).expect_err("invalid endpoint must be rejected");
        assert_eq!(error.kind(), CompletionErrorKind::Config);
        assert!(!error.to_string().contains("missing environment variable"));
    }
    // A clean http(s) API root is still accepted and normalized.
    assert_eq!(
        GatewayEndpoint::new("http://host:8080/v1/")
            .expect("clean URL")
            .url(),
        "http://host:8080/v1"
    );
}

#[test]
fn secret_string_construction_rejects_an_empty_credential() {
    // F12: an empty bearer credential is unrepresentable.
    assert!(matches!(SecretString::new(""), Err(SecretError::Empty)));
    assert!(SecretString::new("tok").is_ok());
}

#[test]
fn gateway_endpoint_trims_trailing_slash_and_keeps_valid_urls() {
    let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")
        .expect("a well-formed https URL is accepted");
    assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
}

#[tokio::test]
async fn complete_sends_completion_options_and_stream_flags_on_the_wire() {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::Json;
    use axum::routing::post;
    use serde_json::Value;

    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let slot = Arc::clone(&slot);
            async move {
                *slot.lock().expect("capture lock") = Some(body);
                (
                    [(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                    sse_body(&[
                        content_chunk("ok"),
                        serde_json::json!({
                            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
                        }),
                    ]),
                )
            }
        }),
    );
    let client = client_for(app).await;
    let options = CompletionOptions {
        model: "analyst".into(),
        temperature: Some(crate::model::Temperature::new(0.0).expect("0.0 is valid")),
        max_tokens: Some(std::num::NonZeroU32::new(128).expect("128 is non-zero")),
        thinking: Some(false),
    };
    client
        .complete(&[Message::user("hi")], None, &options, |_| {})
        .await
        .unwrap();
    let body = captured.lock().expect("capture lock").clone().unwrap();
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
    // The one completion method always streams and always asks for the
    // final usage chunk.
    assert_eq!(body["stream"], true);
    assert_eq!(body["stream_options"]["include_usage"], true);
}

#[tokio::test]
async fn complete_hard_fails_on_empty_model_reply() {
    // A stream that carries only reasoning and a stop finish has no
    // product; the accumulated turn must fail exactly like the buffered
    // equivalent, with the finish_reason surviving.
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "ignored" } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }] }),
    ]))
    .await;
    let options = CompletionOptions {
        model: "m".into(),
        temperature: None,
        max_tokens: None,
        thinking: None,
    };
    let err = client
        .complete(&[Message::user("hi")], None, &options, |_| {})
        .await
        .expect_err("empty product must fail");
    assert_eq!(err.kind(), crate::model::CompletionErrorKind::EmptyReply);
    assert_eq!(
        err.finish_reason(),
        Some("stop"),
        "the finish_reason must survive the conversion into CompletionError"
    );
    assert!(matches!(Error::from(err), Error::EmptyModelReply { .. }));
}

fn openai_options() -> CompletionOptions {
    CompletionOptions::new("m")
}

/// Spawns a gateway that answers `/v1/chat/completions` with a fixed status
/// and raw body, returning its address.
async fn spawn_raw_gateway(status: axum::http::StatusCode, body: &'static str) -> String {
    use axum::Router;
    use axum::routing::post;
    use tokio::net::TcpListener;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(move || async move { (status, body) }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{addr}/v1")
}

#[tokio::test]
async fn complete_on_a_disabled_client_is_a_disabled_error() {
    // F14: a disabled client never touches the network.
    let client = GatewayClient::disabled();
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a disabled client cannot complete");
    assert_eq!(err.kind(), crate::model::CompletionErrorKind::Disabled);
}

#[tokio::test]
async fn complete_refuses_a_success_stream_over_the_size_cap() {
    // F14 (body-size, success path): a 200 stream larger than the cap is
    // refused as the bytes arrive, before any further parsing.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a long reply\"}}]}\n\n",
    )
    .await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
    .with_request_limits(
        DEFAULT_REQUEST_TIMEOUT,
        NonZeroU64::new(8).expect("non-zero cap"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an oversize stream must be refused");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
}

#[tokio::test]
async fn complete_refuses_a_backend_error_body_over_the_size_cap() {
    // F14 (body-size, error path): a non-success body larger than the cap is
    // also refused before it is buffered.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "this backend error body is definitely longer than eight bytes",
    )
    .await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    )
    .with_request_limits(
        DEFAULT_REQUEST_TIMEOUT,
        NonZeroU64::new(8).expect("non-zero cap"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an oversize error body must be refused");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
}

#[tokio::test]
async fn complete_refuses_a_malformed_stream_chunk() {
    // F14: a 200 whose stream carries an undecodable chunk is
    // MalformedResponse, and the decode failure is preserved as the
    // error-chain source.
    let base = spawn_raw_gateway(axum::http::StatusCode::OK, "data: { not json\n\n").await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("undecodable chunk must fail");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
    let source =
        std::error::Error::source(&err).expect("the decode error must be a preserved source");
    assert!(
        source.downcast_ref::<serde_json::Error>().is_some(),
        "the preserved source must be the JSON decode error, got {source}"
    );
}

#[tokio::test]
async fn complete_refuses_malformed_tool_call_fragments_at_the_boundary() {
    // F14: a well-formed HTTP 200 whose streamed tool-call fragment carries
    // non-string arguments is rejected at the client boundary, not passed on.
    let client = sse_client(sse_body(&[serde_json::json!({
        "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "id": "c1", "type": "function",
            "function": { "name": "t", "arguments": 123 }
        }] } }]
    })]))
    .await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("malformed tool arguments must be rejected");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
}

#[tokio::test]
async fn streamed_text_usage_timings_and_client_timing_accumulate() {
    // The llama.cpp streamed shape: content fragments, a finish chunk, and
    // the include_usage summary chunk carrying usage plus timings. The
    // accumulated completion must match the buffered equivalent while the
    // deltas reach the callback in order, and the client's own clock must
    // populate ClientTiming.
    let client = sse_client(sse_body(&[
        content_chunk("Hel"),
        content_chunk("lo!"),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
        serde_json::json!({
            "model": "qwen3-30b",
            "choices": [],
            "usage": { "completion_tokens": 3, "prompt_tokens": 7, "total_tokens": 10 },
            "timings": {
                "prompt_n": 7, "prompt_ms": 12.5, "prompt_per_second": 560.0,
                "predicted_n": 3, "predicted_ms": 30.5, "predicted_per_second": 98.5
            }
        }),
    ]))
    .await;
    let seen = std::sync::Mutex::new(Vec::new());
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |delta| {
            seen.lock().expect("delta log").push(delta);
        })
        .await
        .expect("a streamed text turn completes");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "Hello!"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(
        *seen.lock().expect("delta log"),
        vec![
            StreamDelta::Text("Hel".to_owned()),
            StreamDelta::Text("lo!".to_owned()),
        ],
        "each content fragment reaches the callback live, in order"
    );
    assert_eq!(completion.finish_reason(), Some("stop"));
    assert_eq!(completion.model(), "qwen3-30b");
    let usage = completion.usage().expect("usage from the final chunk");
    assert_eq!(usage.total_tokens, 10);
    let timings = completion
        .llama_timings()
        .expect("timings from the final chunk");
    assert_eq!(timings.predicted_n, 3);
    let timing = completion
        .client_timing()
        .expect("the streaming transport measures its own clock");
    assert!(
        timing.ttft_ms.is_some_and(|ttft| ttft >= 0.0),
        "TTFT is measured once the first delta arrives: {timing:?}"
    );
    assert!(
        timing.mean_itl_ms.is_some_and(|itl| itl >= 0.0),
        "mean ITL is measured with two delta chunks: {timing:?}"
    );
    assert!(timing.e2e_ms >= 0.0);
}

#[tokio::test]
async fn streamed_reasoning_stays_a_side_channel() {
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0,
            "delta": { "reasoning_content": "scratch" } }] }),
        content_chunk("answer"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "stop" }]
        }),
    ]))
    .await;
    let seen = std::sync::Mutex::new(Vec::new());
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |delta| {
            seen.lock().expect("delta log").push(delta);
        })
        .await
        .expect("reasoning plus text completes");
    match completion.result() {
        CompletionResult::Text(text) => {
            assert_eq!(
                text, "answer",
                "reasoning is never promoted into the answer"
            );
        }
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.reasoning_content(), Some("scratch"));
    assert_eq!(
        *seen.lock().expect("delta log"),
        vec![
            StreamDelta::Reasoning("scratch".to_owned()),
            StreamDelta::Text("answer".to_owned()),
        ],
        "reasoning and text deltas arrive separated"
    );
}

#[tokio::test]
async fn streamed_tool_call_fragments_reassemble_into_the_batch() {
    let client = sse_client(sse_body(&[
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "id": "call_1", "type": "function",
            "function": { "name": "web_search", "arguments": "{\"qu" }
        }] } }] }),
        serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
            "index": 0, "function": { "arguments": "ery\":\"rust\"}" }
        }] } }] }),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "tool_calls" }]
        }),
    ]))
    .await;
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("a streamed tool-call turn completes");
    match completion.result() {
        CompletionResult::ToolCalls(calls) => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id(), "call_1");
            assert_eq!(calls[0].name(), "web_search");
            assert_eq!(
                calls[0].arguments().to_json_string(),
                "{\"query\":\"rust\"}",
                "argument fragments buffer until the batch is whole"
            );
        }
        other => panic!("expected tool calls, got {other:?}"),
    }
}

#[tokio::test]
async fn truncated_tool_call_batch_fails_the_completion() {
    // A length or content_filter finish with tool calls means the batch may
    // hold partial JSON arguments; the whole batch fails rather than
    // executing a fragment.
    for reason in ["length", "content_filter"] {
        let client = sse_client(sse_body(&[
            serde_json::json!({ "choices": [{ "index": 0, "delta": { "tool_calls": [{
                "index": 0, "id": "c1", "type": "function",
                "function": { "name": "t", "arguments": "{\"whole\":true}" }
            }] } }] }),
            serde_json::json!({
                "choices": [{ "index": 0, "delta": {}, "finish_reason": reason }]
            }),
        ]))
        .await;
        let err = client
            .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
            .await
            .expect_err("a truncated tool-call batch must fail");
        assert_eq!(
            err.kind(),
            crate::model::CompletionErrorKind::MalformedResponse,
            "finish_reason {reason:?}"
        );
        assert!(
            err.to_string().contains("truncated"),
            "the error names the truncation: {err}"
        );
    }
}

#[tokio::test]
async fn truncated_text_still_returns_with_its_finish_reason() {
    // The truncation rule fails tool-call batches only: partial TEXT is
    // returned with finish_reason "length" so the caller can report it.
    let client = sse_client(sse_body(&[
        content_chunk("partial answ"),
        serde_json::json!({
            "choices": [{ "index": 0, "delta": {}, "finish_reason": "length" }]
        }),
    ]))
    .await;
    let completion = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect("truncated text is still a product");
    match completion.result() {
        CompletionResult::Text(text) => assert_eq!(text, "partial answ"),
        other => panic!("expected text, got {other:?}"),
    }
    assert_eq!(completion.finish_reason(), Some("length"));
}

#[tokio::test]
async fn stream_without_done_sentinel_is_malformed() {
    // A stream cut off before [DONE] may be missing its tail; it must never
    // pass for a complete turn.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"half\"}}]}\n\n",
    )
    .await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("a truncated stream must fail");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
    assert!(
        err.to_string().contains("[DONE]"),
        "the error names the missing sentinel: {err}"
    );
}

#[tokio::test]
async fn mid_stream_error_envelope_is_a_transport_failure() {
    // The gateway relays a mid-flight failure as a data: error envelope on
    // an already-open 200 stream; the completion classifies it as a
    // transport failure, never as model output.
    let client = sse_client(sse_body(&[
        content_chunk("par"),
        serde_json::json!({ "error": {
            "message": "upstream died", "type": "upstream", "code": "upstream_transport"
        } }),
    ]))
    .await;
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options(), |_| {})
        .await
        .expect_err("an error envelope must fail the completion");
    assert_eq!(err.kind(), crate::model::CompletionErrorKind::Transport);
    let source = std::error::Error::source(&err)
        .expect("the envelope message must ride as the cause")
        .to_string();
    assert!(source.contains("upstream died"), "cause: {source}");
}
