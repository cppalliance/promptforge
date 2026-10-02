//! Tests for the HTTP failure classifier: one per rule, per overflow
//! phrase, per token-count form, the in-stream envelope, and a body that
//! matches nothing.

use super::{classify_http_failure, classify_stream_error};
use crate::model::CompletionErrorKind;

fn kind_of(status: u16, body: &str) -> CompletionErrorKind {
    classify_http_failure(status, body).kind()
}

#[test]
fn every_overflow_phrase_classifies_a_400_as_context_overflow() {
    let phrases = [
        "context length",
        "context window",
        "context size",
        "context_length_exceeded",
        "maximum context length",
        "prompt is too long",
        "too many tokens",
        "exceeds the available context size",
        "exceed_context_size",
        "input is too long",
        "exceeds the maximum number of tokens",
        "too large for model",
    ];
    assert_eq!(phrases.len(), 12, "the plan lists twelve phrases");
    for phrase in phrases {
        let body = format!("{{\"error\":\"the request hit a limit: {phrase} reached\"}}");
        assert_eq!(
            kind_of(400, &body),
            CompletionErrorKind::ContextOverflow,
            "400 with {phrase:?}"
        );
        assert_eq!(
            kind_of(413, &body),
            CompletionErrorKind::ContextOverflow,
            "413 with {phrase:?}"
        );
    }
}

#[test]
fn overflow_phrases_match_case_insensitively() {
    assert_eq!(
        kind_of(400, "CONTEXT WINDOW exceeded"),
        CompletionErrorKind::ContextOverflow
    );
    assert_eq!(
        kind_of(413, "Prompt Is Too Long"),
        CompletionErrorKind::ContextOverflow
    );
}

#[test]
fn provider_overflow_detection_matches_known_signatures() {
    let cases: &[(u16, &str, CompletionErrorKind)] = &[
        (
            400,
            "This model's maximum context length is 4096 tokens.",
            CompletionErrorKind::ContextOverflow,
        ),
        (
            400,
            "context_length_exceeded",
            CompletionErrorKind::ContextOverflow,
        ),
        (
            400,
            "the request exceeds the available context size",
            CompletionErrorKind::ContextOverflow,
        ),
        (
            413,
            "prompt is too long",
            CompletionErrorKind::ContextOverflow,
        ),
        (
            400,
            "CONTEXT WINDOW exceeded",
            CompletionErrorKind::ContextOverflow,
        ),
        (
            400,
            "too many tokens in prompt",
            CompletionErrorKind::ContextOverflow,
        ),
        // A server fault never classifies as an overflow, even with
        // overflow wording.
        (
            500,
            "maximum context length is 4096 tokens",
            CompletionErrorKind::ServerError,
        ),
        // A client rejection without overflow wording stays a plain
        // rejection.
        (
            400,
            "invalid request: unknown field `stream`",
            CompletionErrorKind::Rejected,
        ),
        // Credentials win over overflow wording outside 400 and 413.
        (401, "context length", CompletionErrorKind::Unavailable),
    ];
    for (status, body, expected) in cases {
        assert_eq!(
            kind_of(*status, body),
            *expected,
            "status {status} with body {body:?}"
        );
    }
}

#[test]
fn an_overflow_phrase_on_another_status_is_not_an_overflow() {
    assert_eq!(
        kind_of(422, "context length exceeded"),
        CompletionErrorKind::Rejected
    );
    assert_eq!(
        kind_of(429, "context length"),
        CompletionErrorKind::RateLimited
    );
}

#[test]
fn the_openai_token_counts_are_read_from_the_message() {
    let error = classify_http_failure(
        400,
        "This model's maximum context length is 4096 tokens. However, your messages \
         resulted in 5120 tokens. Please reduce the length of the messages.",
    );
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (Some(5120), Some(4096)));
}

#[test]
fn the_greater_than_maximum_token_counts_are_read_from_the_message() {
    let error = classify_http_failure(400, "prompt is too long: 205000 tokens > 200000 maximum");
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (Some(205_000), Some(200_000)));
}

#[test]
fn an_overflow_that_states_no_counts_leaves_them_unset() {
    let error = classify_http_failure(400, "context_length_exceeded");
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (None, None));
}

#[test]
fn a_count_too_large_for_u32_is_left_unset() {
    let error = classify_http_failure(
        400,
        "maximum context length is 99999999999 tokens, however you sent 5 tokens",
    );
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (Some(5), None));
}

#[test]
fn a_429_naming_quota_or_billing_is_quota_exhausted() {
    for word in ["quota", "billing", "insufficient_quota", "credit", "QUOTA"] {
        let body = format!("{{\"error\":\"your {word} has run out\"}}");
        assert_eq!(
            kind_of(429, &body),
            CompletionErrorKind::QuotaExhausted,
            "429 with {word:?}"
        );
    }
}

#[test]
fn any_other_429_is_rate_limited() {
    assert_eq!(
        kind_of(429, "slow down: too many requests"),
        CompletionErrorKind::RateLimited
    );
    assert_eq!(
        kind_of(429, "(empty body)"),
        CompletionErrorKind::RateLimited
    );
}

#[test]
fn a_503_or_529_is_overloaded() {
    assert_eq!(
        kind_of(503, "service unavailable"),
        CompletionErrorKind::Overloaded
    );
    assert_eq!(kind_of(529, "busy"), CompletionErrorKind::Overloaded);
}

#[test]
fn a_5xx_naming_overloaded_is_overloaded() {
    assert_eq!(
        kind_of(500, "{\"type\":\"overloaded_error\"}"),
        CompletionErrorKind::Overloaded
    );
    assert_eq!(
        kind_of(502, "The server is OVERLOADED"),
        CompletionErrorKind::Overloaded
    );
}

#[test]
fn every_other_5xx_is_a_server_error() {
    for status in [500, 501, 502, 504, 599] {
        assert_eq!(
            kind_of(status, "internal failure"),
            CompletionErrorKind::ServerError,
            "status {status}"
        );
    }
}

#[test]
fn a_401_or_403_is_unavailable_with_the_credentials_phrase() {
    for status in [401, 403] {
        let error = classify_http_failure(status, "bad key");
        assert_eq!(error.kind(), CompletionErrorKind::Unavailable);
        assert_eq!(
            error.to_string(),
            format!("the model backend did not accept the credentials (status {status})")
        );
    }
}

#[test]
fn a_400_naming_a_content_policy_term_is_refused() {
    for word in ["content_filter", "content policy", "safety", "refus"] {
        let body = format!("{{\"error\":\"blocked by {word} rules\"}}");
        assert_eq!(
            kind_of(400, &body),
            CompletionErrorKind::Refused,
            "400 with {word:?}"
        );
    }
}

#[test]
fn a_400_naming_both_a_context_limit_and_a_safety_term_is_an_overflow() {
    assert_eq!(
        kind_of(400, "context length exceeded; safety settings unchanged"),
        CompletionErrorKind::ContextOverflow
    );
}

#[test]
fn a_content_policy_term_on_another_status_is_rejected() {
    assert_eq!(
        kind_of(422, "content_filter"),
        CompletionErrorKind::Rejected
    );
}

#[test]
fn a_body_that_matches_no_rule_is_rejected_never_a_success() {
    for (status, body) in [
        (400, "invalid request: unknown field `stream`"),
        (404, "no such model"),
        (422, "unprocessable"),
        (418, "(empty body)"),
        (302, "moved"),
        (200, "ok"),
    ] {
        assert_eq!(
            kind_of(status, body),
            CompletionErrorKind::Rejected,
            "status {status} with body {body:?}"
        );
    }
}

#[test]
fn the_message_names_the_kind_and_the_status_and_keeps_the_body_as_detail() {
    let error = classify_http_failure(503, "upstream <busy>");
    assert_eq!(
        error.to_string(),
        "the model backend is overloaded (status 503)"
    );
    assert_eq!(
        error.message(),
        "the model backend is overloaded (status 503)"
    );
    assert_eq!(error.detail(), Some("upstream <busy>"));
    assert!(
        !error.to_string().contains("upstream"),
        "Display never carries the body"
    );
    assert_eq!(error.status(), Some(503));
    assert_eq!(error.backend_body(), Some("upstream <busy>"));
}

#[test]
fn each_http_kind_carries_its_fixed_phrase_and_the_status_suffix() {
    let cases: &[(u16, &str, &str)] = &[
        (
            400,
            "context length",
            "the request is larger than the model's context window (status 400)",
        ),
        (
            429,
            "slow down",
            "the model backend is limiting the request rate (status 429)",
        ),
        (
            429,
            "quota",
            "the model backend says the usage quota is spent (status 429)",
        ),
        (503, "busy", "the model backend is overloaded (status 503)"),
        (
            400,
            "content_filter",
            "the model backend refused the request on content policy grounds (status 400)",
        ),
        (
            500,
            "boom",
            "the model backend reported a fault of its own (status 500)",
        ),
        (
            404,
            "no model",
            "the model backend rejected the request (status 404)",
        ),
        (
            401,
            "no key",
            "the model backend did not accept the credentials (status 401)",
        ),
    ];
    for (status, body, message) in cases {
        let error = classify_http_failure(*status, body);
        assert_eq!(error.to_string(), *message, "status {status}, {body:?}");
        assert_eq!(error.message(), *message);
    }
}

#[test]
fn the_classifier_keeps_only_the_body_it_was_given() {
    let body = "x".repeat(2000);
    let error = classify_http_failure(400, &body);
    assert_eq!(error.detail().map(str::len), Some(2000));
}

#[test]
fn an_in_stream_envelope_naming_a_context_limit_is_an_overflow_without_a_status() {
    let error = classify_stream_error(
        "This model's maximum context length is 4096 tokens, however you sent 5000 tokens",
    );
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (Some(5000), Some(4096)));
    assert_eq!(
        error.to_string(),
        "the request is larger than the model's context window"
    );
    assert_eq!(error.status(), None);
}

#[test]
fn an_in_stream_envelope_applies_the_other_body_text_rules() {
    assert_eq!(
        classify_stream_error("overloaded_error: try again").kind(),
        CompletionErrorKind::Overloaded
    );
    assert_eq!(
        classify_stream_error("insufficient_quota").kind(),
        CompletionErrorKind::QuotaExhausted
    );
    assert_eq!(
        classify_stream_error("blocked by content_filter").kind(),
        CompletionErrorKind::Refused
    );
}

#[test]
fn an_in_stream_envelope_that_matches_no_rule_stays_a_transport_failure() {
    let error = classify_stream_error("upstream\\ndied");
    assert_eq!(error.kind(), CompletionErrorKind::Transport);
    assert_eq!(
        error.to_string(),
        "the connection to the model backend failed"
    );
    assert_eq!(error.detail(), Some("upstream\\ndied"));
    assert_eq!(error.status(), None);
    assert!(error.is_retryable());
}
