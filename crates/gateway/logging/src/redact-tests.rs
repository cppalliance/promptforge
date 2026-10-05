//! Tests for field classification and the bounded text redaction pass.

use super::*;

#[test]
fn an_authorization_header_value_is_masked() {
    let redacted = redact_line("sending Authorization: Bearer abc123secret to upstream");
    assert!(
        !redacted.contains("abc123secret"),
        "the bearer token never reaches a record: {redacted}"
    );
    assert!(
        redacted.contains("Authorization: [redacted]"),
        "the header name survives so the log stays legible: {redacted}"
    );
}

#[test]
fn a_lowercase_authorization_header_is_masked() {
    let redacted = redact_line("header authorization: basic dXNlcg== rejected");
    assert!(
        !redacted.contains("dXNlcg=="),
        "HTTP/2's lowercase header names redact the same way: {redacted}"
    );
}

#[test]
fn cookie_and_set_cookie_values_are_masked() {
    let redacted = redact_line("request Cookie: session=xyz789; other=1\nnext line");
    assert!(
        !redacted.contains("xyz789"),
        "the cookie value never reaches a record: {redacted}"
    );
    assert!(
        redacted.contains("next line"),
        "redaction stops at the end of the header's line: {redacted}"
    );
    let redacted = redact_line("response Set-Cookie: token=abc; HttpOnly");
    assert!(
        !redacted.contains("token=abc"),
        "a set-cookie value never reaches a record: {redacted}"
    );
}

#[test]
fn a_bare_bearer_token_is_masked() {
    let redacted = redact_line("upstream rejected Bearer tok_live_51xyz with 401");
    assert!(
        !redacted.contains("tok_live_51xyz"),
        "a bearer token without its header name is still masked: {redacted}"
    );
    assert!(
        redacted.contains("Bearer [redacted]"),
        "the scheme survives: {redacted}"
    );
}

#[test]
fn api_key_assignments_are_masked_in_toml_and_json_shapes() {
    for (line, secret) in [
        ("api_key = \"toml-secret\"", "toml-secret"),
        ("api_key=\"compact-secret\"", "compact-secret"),
        ("{\"api_key\": \"json-secret\"}", "json-secret"),
        ("api_key: bare-secret, done", "bare-secret"),
    ] {
        let redacted = redact_line(line);
        assert!(
            !redacted.contains(secret),
            "the api_key value never reaches a record: {redacted}"
        );
        assert!(
            redacted.contains("api_key"),
            "the field name survives: {redacted}"
        );
    }
}

#[test]
fn adversarial_unstructured_values_are_masked() {
    for (line, secret) in [
        (
            "authorization: Basic YmFzaWMtdXNlcjpiYXNpYy1zZWNyZXQ=",
            "YmFzaWMtdXNlcjpiYXNpYy1zZWNyZXQ=",
        ),
        (
            "proxy rejected Basic YmFyZS11c2VyOmJhcmUtc2VjcmV0",
            "YmFyZS11c2VyOmJhcmUtc2VjcmV0",
        ),
        (
            "dependency rejected Bearer bearer-secret, retrying",
            "bearer-secret",
        ),
        ("cookie=session=cookie-secret; theme=dark", "cookie-secret"),
        ("set-cookie='set-cookie-secret'", "set-cookie-secret"),
        ("url=https://user:url-secret@example.test/v1", "url-secret"),
        (
            "GET https://user:embedded-url-secret@example.test/v1",
            "embedded-url-secret",
        ),
        ("prompt=\"first line\nprompt-secret\"", "prompt-secret"),
        (
            "prompt=\"escaped \\\" quote then escaped-prompt-secret\"",
            "escaped-prompt-secret",
        ),
        (
            "model_path=C:\\private\\path-secret\\model.gguf",
            "path-secret",
        ),
        (
            "payload={\"outer\":{\"token\":\"payload-secret\"}}",
            "payload-secret",
        ),
        (
            "outer error\ncaused by: request failed\ncaused by: api_key=nested-secret",
            "nested-secret",
        ),
        (
            "outer error\ncaused by: GET https://host/private-route?opaque-secret",
            "opaque-secret",
        ),
        (
            "outer error\ncaused by: model load failed at C:\\private\\model-secret.gguf",
            "model-secret",
        ),
        (
            "outer error\ncaused by: model load failed at /private/models/unix-secret.gguf",
            "unix-secret",
        ),
    ] {
        let redacted = redact_line(line);
        assert!(
            !redacted.contains(secret),
            "protected text survives redaction: {redacted}"
        );
        assert!(
            redacted.contains(REDACTED),
            "the mask marks the removed value: {redacted}"
        );
    }
}

#[test]
fn structured_field_classification_uses_whole_components() {
    for field in [
        "authorization",
        "authorization_header",
        "cookie_header",
        "gateway_api_key",
        "request.headers",
        "request_token_value",
        "upstream-url",
        "system_prompt",
        "config_path",
        "request_body",
        "secret",
    ] {
        assert!(is_sensitive_field(field), "{field} must be classified");
    }
    for field in [
        "message",
        "profile",
        "token_count",
        "body_count",
        "url_status",
        "secretary",
    ] {
        assert!(
            !is_sensitive_field(field),
            "{field} is an ordinary diagnostic field"
        );
    }
}

#[test]
fn structured_field_classification_preserves_namespaces_prefixes_and_alias_chains() {
    for field in [
        "request.authorization",
        "request:authorization",
        "r#authorization",
        "gateway-api_key",
        "request_token_raw_value",
        "response-cookie-header-values",
    ] {
        assert!(is_sensitive_field(field), "{field} must be classified");
    }
    for field in [
        "request.token_count",
        "request:body_size",
        "authorization_metadata",
        "cookie_jar",
        "secretary_value",
    ] {
        assert!(
            !is_sensitive_field(field),
            "{field} must remain an ordinary diagnostic field"
        );
    }
}

#[test]
fn mixed_patterns_cannot_leak_partial_secrets_at_any_capacity_boundary() {
    const FIRST_SECRET: &str = "a";
    const URL_SECRET: &str = "capacity-boundary-url-secret";
    let line = "api_key=a then https://user:capacity-boundary-url-secret@example.test/private";

    for capacity in (REDACTED.len() + 1)..line.len() {
        let output = redact_line_bounded(line, capacity)
            .finish("", false)
            .expect("ASCII input remains valid");
        assert!(
            !output.0.contains("api_key=a"),
            "the short first secret is masked at capacity {capacity}: {}",
            output.0
        );
        for fragment_len in 4..=URL_SECRET.len() {
            assert!(
                !output.0.contains(&URL_SECRET[..fragment_len]),
                "a URL secret prefix survived at capacity {capacity}: {}",
                output.0
            );
        }
        assert_ne!(
            output.0.as_ref(),
            FIRST_SECRET,
            "the first secret is never emitted by itself"
        );
    }
}

#[test]
fn unlabeled_urls_and_local_paths_are_replaced_whole_in_error_chains() {
    let redacted = redact_line(
        "dependency failed\ncaused by: https://host/private-route?opaque\ncaused by: C:\\private\\model.gguf\ncaused by: /opt/private/model.gguf",
    );
    for protected in [
        "https://host/private-route?opaque",
        "C:\\private\\model.gguf",
        "/opt/private/model.gguf",
    ] {
        assert!(
            !redacted.contains(protected),
            "an unlabeled URL or path survived: {redacted}"
        );
    }
    assert_eq!(
        redacted.matches(REDACTED).count(),
        3,
        "each complete protected location becomes one mask"
    );
}

#[test]
fn an_ordinary_line_passes_through_unchanged() {
    for line in [
        "loaded profile main with 2 models; bind 127.0.0.1:8081",
        "logging to C:\\Users\\operator\\.promptforge\\logs\\gateway.log",
    ] {
        assert_eq!(
            redact_line(line),
            line,
            "a line without a sensitive shape is byte-identical"
        );
    }
}

#[test]
fn a_field_name_mention_without_a_value_is_not_a_leak() {
    let line = "the api_key field is required";
    assert_eq!(
        redact_line(line),
        line,
        "naming the field redacts nothing: no assignment follows"
    );
}
