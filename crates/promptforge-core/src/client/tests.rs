use super::transport::{DEFAULT_REQUEST_TIMEOUT, escape_controls, from_env_with};
use super::*;
use std::num::NonZeroU64;

use crate::Error;
use crate::model::CompletionOptions;
use serde_json::Value;

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
    let err = from_env_with(lookup_from(&[(
        "PROMPTFORGE_GATEWAY_URL",
        "http://127.0.0.1:8081/v1",
    )]))
    .expect_err("missing key must fail");
    assert!(matches!(
        err,
        Error::MissingEnv(name) if name == "PROMPTFORGE_GATEWAY_API_KEY"
    ));
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
    use tokio::net::TcpListener;

    // A non-success body carrying control characters and a would-be secret.
    async fn handler() -> (axum::http::StatusCode, String) {
        (
            axum::http::StatusCode::BAD_GATEWAY,
            "forged\nlog: super-secret".to_owned(),
        )
    }
    let app = Router::new().route("/v1/chat/completions", post(handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let options = CompletionOptions::new("m", crate::dialects::ToolDialectId::OpenAi);
    let err = client
        .complete(&[Message::user("hi")], None, &options)
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
    assert!(GatewayEndpoint::new("ftp://example.com/v1").is_err());
    assert!(GatewayEndpoint::new("not-a-url").is_err());
    assert!(GatewayEndpoint::new("http://").is_err());
    assert!(GatewayEndpoint::new("").is_err());
}

#[test]
fn gateway_endpoint_rejects_credentials_query_and_fragment() {
    // F12: the strict URL parse rejects embedded credentials and the
    // query/fragment ambiguity a hand-rolled prefix scan let through.
    assert!(GatewayEndpoint::new("http://user:pass@host/v1").is_err());
    assert!(GatewayEndpoint::new("http://user@host/v1").is_err());
    assert!(GatewayEndpoint::new("http://host/v1?token=leak").is_err());
    assert!(GatewayEndpoint::new("http://host/v1#frag").is_err());
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
async fn complete_sends_completion_options_model_on_the_wire() {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::Json;
    use axum::routing::post;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    let captured: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
    let slot = Arc::clone(&captured);
    let app = Router::new().route(
        "/v1/chat/completions",
        post(move |Json(body): Json<Value>| {
            let slot = Arc::clone(&slot);
            async move {
                *slot.lock().expect("capture lock") = Some(body);
                Json(json!({
                    "choices": [{
                        "message": { "role": "assistant", "content": "ok" },
                        "finish_reason": "stop"
                    }]
                }))
            }
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let options = CompletionOptions {
        model: "analyst".into(),
        temperature: Some(crate::model::Temperature::new(0.0).expect("0.0 is valid")),
        max_tokens: Some(std::num::NonZeroU32::new(128).expect("128 is non-zero")),
        thinking: Some(false),
        tool_dialect: crate::dialects::ToolDialectId::OpenAi,
    };
    client
        .complete(&[Message::user("hi")], None, &options)
        .await
        .unwrap();
    let body = captured.lock().expect("capture lock").clone().unwrap();
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["temperature"], 0.0);
    assert_eq!(body["max_tokens"], 128);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
}

#[tokio::test]
async fn complete_hard_fails_on_empty_model_reply() {
    use axum::Router;
    use axum::extract::Json;
    use axum::routing::post;
    use serde_json::{Value, json};
    use tokio::net::TcpListener;

    let app = Router::new().route(
        "/v1/chat/completions",
        post(|Json(_body): Json<Value>| async move {
            Json(json!({
                "choices": [{
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "reasoning_content": "ignored"
                    },
                    "finish_reason": "stop"
                }]
            }))
        }),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let options = CompletionOptions {
        model: "m".into(),
        temperature: None,
        max_tokens: None,
        thinking: None,
        tool_dialect: crate::dialects::ToolDialectId::OpenAi,
    };
    let err = client
        .complete(&[Message::user("hi")], None, &options)
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
    CompletionOptions::new("m", crate::dialects::ToolDialectId::OpenAi)
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
        .complete(&[Message::user("hi")], None, &openai_options())
        .await
        .expect_err("a disabled client cannot complete");
    assert_eq!(err.kind(), crate::model::CompletionErrorKind::Disabled);
}

#[tokio::test]
async fn complete_refuses_a_success_body_over_the_size_cap() {
    // F14 (body-size, success path): a 200 body larger than the cap is
    // refused before decoding.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":\"a long reply\"}}]}",
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
        .complete(&[Message::user("hi")], None, &openai_options())
        .await
        .expect_err("an oversize body must be refused");
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
        .complete(&[Message::user("hi")], None, &openai_options())
        .await
        .expect_err("an oversize error body must be refused");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
}

#[tokio::test]
async fn complete_refuses_malformed_successful_json() {
    // F14: a 200 whose body is not valid JSON is MalformedResponse, and the
    // decode failure is preserved as the error-chain source.
    let base = spawn_raw_gateway(axum::http::StatusCode::OK, "{ not json").await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options())
        .await
        .expect_err("undecodable body must fail");
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
async fn complete_refuses_malformed_tool_call_arguments_at_the_boundary() {
    // F14: a well-formed HTTP 200 whose tool-call arguments are not a JSON
    // object string is rejected at the client boundary, not passed on.
    let base = spawn_raw_gateway(
        axum::http::StatusCode::OK,
        "{\"choices\":[{\"message\":{\"role\":\"assistant\",\"content\":null,\
             \"tool_calls\":[{\"id\":\"c1\",\"type\":\"function\",\
             \"function\":{\"name\":\"t\",\"arguments\":123}}]},\
             \"finish_reason\":\"tool_calls\"}]}",
    )
    .await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&base).expect("valid endpoint"),
        SecretString::new("tok").expect("non-empty test key"),
    );
    let err = client
        .complete(&[Message::user("hi")], None, &openai_options())
        .await
        .expect_err("malformed tool arguments must be rejected");
    assert_eq!(
        err.kind(),
        crate::model::CompletionErrorKind::MalformedResponse
    );
}
