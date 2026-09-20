//! Environment loading, the bearer on the wire, and the credential and
//! endpoint guards.

use harness_runner::spawn::spawn_tagged;
use promptforge_api_runtime::model::Message;

use super::*;
use crate::CompletionErrorKind;
use crate::config::SecretError;

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
                    ok_stream(),
                )
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    spawn_tagged("mock-auth-gateway", async move {
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
    let client = keyed_client(&base);
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
fn secret_string_construction_rejects_an_empty_credential() {
    // F12: an empty bearer credential is unrepresentable.
    assert!(matches!(SecretString::new(""), Err(SecretError::Empty)));
    assert!(SecretString::new("tok").is_ok());
}

#[test]
fn an_unusable_secret_classifies_as_config_and_keeps_its_cause() {
    // AUDIT-DISCARDED-SOURCE: the SecretError survives as the public
    // CompletionError's source, classified as Config.
    let secret_error = SecretString::new("").expect_err("blank key is rejected");
    let completion = crate::CompletionError::from(secret_error);
    assert_eq!(completion.kind(), CompletionErrorKind::Config);
    assert!(
        std::error::Error::source(&completion).is_some(),
        "the SecretError cause must survive"
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
fn gateway_endpoint_keeps_the_url_parse_cause() {
    // AUDIT-DISCARDED-SOURCE: the url::ParseError survives as the source.
    let url_error = GatewayEndpoint::new("not a url").expect_err("malformed URL is rejected");
    assert_eq!(url_error.kind(), CompletionErrorKind::Config);
    assert!(
        std::error::Error::source(&url_error).is_some(),
        "the url::ParseError cause must survive"
    );
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
fn gateway_endpoint_trims_trailing_slash_and_keeps_valid_urls() {
    let endpoint = GatewayEndpoint::new("https://gateway.example.com/v1/")
        .expect("a well-formed https URL is accepted");
    assert_eq!(endpoint.url(), "https://gateway.example.com/v1");
}
