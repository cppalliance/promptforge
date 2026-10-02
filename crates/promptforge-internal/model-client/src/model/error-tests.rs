//! Tests for the failure vocabulary: retryability per kind, the fixed
//! phrases, the detail channel, and the conversion from the crate's error.

use super::*;
use crate::client::classify_http_failure;
use crate::detail::error_http;

const ALL_KINDS: [CompletionErrorKind; 12] = [
    CompletionErrorKind::ContextOverflow,
    CompletionErrorKind::RateLimited,
    CompletionErrorKind::QuotaExhausted,
    CompletionErrorKind::Overloaded,
    CompletionErrorKind::Refused,
    CompletionErrorKind::Timeout,
    CompletionErrorKind::Transport,
    CompletionErrorKind::ServerError,
    CompletionErrorKind::Rejected,
    CompletionErrorKind::MalformedResponse,
    CompletionErrorKind::EmptyReply,
    CompletionErrorKind::Unavailable,
];

#[test]
fn retryability_is_fixed_per_kind() {
    let retryable = [
        CompletionErrorKind::RateLimited,
        CompletionErrorKind::Overloaded,
        CompletionErrorKind::Timeout,
        CompletionErrorKind::Transport,
        CompletionErrorKind::ServerError,
        CompletionErrorKind::MalformedResponse,
    ];
    for kind in ALL_KINDS {
        let error = CompletionError::new(kind, "message");
        assert_eq!(
            error.is_retryable(),
            retryable.contains(&kind),
            "retryability of {kind:?}"
        );
    }
}

#[test]
fn a_429_is_retryable_now() {
    assert!(classify_http_failure(429, "slow down").is_retryable());
    assert!(!classify_http_failure(429, "insufficient_quota").is_retryable());
}

#[test]
fn each_kind_has_its_fixed_phrase() {
    let phrases = [
        (
            CompletionErrorKind::ContextOverflow,
            "the request is larger than the model's context window",
        ),
        (
            CompletionErrorKind::RateLimited,
            "the model backend is limiting the request rate",
        ),
        (
            CompletionErrorKind::QuotaExhausted,
            "the model backend says the usage quota is spent",
        ),
        (
            CompletionErrorKind::Overloaded,
            "the model backend is overloaded",
        ),
        (
            CompletionErrorKind::Refused,
            "the model backend refused the request on content policy grounds",
        ),
        (
            CompletionErrorKind::Timeout,
            "the model backend did not answer in time",
        ),
        (
            CompletionErrorKind::Transport,
            "the connection to the model backend failed",
        ),
        (
            CompletionErrorKind::ServerError,
            "the model backend reported a fault of its own",
        ),
        (
            CompletionErrorKind::Rejected,
            "the model backend rejected the request",
        ),
        (
            CompletionErrorKind::MalformedResponse,
            "the model backend sent a reply that could not be understood",
        ),
        (
            CompletionErrorKind::EmptyReply,
            "the model replied with no text and no tool calls",
        ),
        (
            CompletionErrorKind::Unavailable,
            "model access is turned off or not configured",
        ),
    ];
    assert_eq!(phrases.len(), ALL_KINDS.len());
    for (kind, phrase) in phrases {
        assert_eq!(kind.phrase(), phrase, "phrase of {kind:?}");
    }
}

#[test]
fn display_shows_the_message_and_never_the_detail() {
    let error = CompletionError::new(CompletionErrorKind::Rejected, "the message")
        .with_detail("provider says: <secret>");
    assert_eq!(error.to_string(), "the message");
    assert_eq!(error.message(), "the message");
    assert_eq!(error.detail(), Some("provider says: <secret>"));
    assert!(!format!("{error}").contains("secret"));
    assert!(
        !format!("{error:?}").is_empty(),
        "Debug stays available to operators"
    );
}

#[test]
fn a_new_error_has_no_extras() {
    let error = CompletionError::new(CompletionErrorKind::Transport, "down");
    assert_eq!(error.overflow(), (None, None));
    assert_eq!(error.finish_reason(), None);
    assert_eq!(error.detail(), None);
    assert!(std::error::Error::source(&error).is_none());
    assert_eq!(error.status(), None);
    assert_eq!(error.backend_body(), None);
}

#[test]
fn context_overflow_holds_the_counts() {
    let error = CompletionError::context_overflow(Some(10), None, "too big");
    assert_eq!(error.kind(), CompletionErrorKind::ContextOverflow);
    assert_eq!(error.overflow(), (Some(10), None));
    assert!(!error.is_retryable());
}

#[test]
fn with_source_keeps_the_cause_in_the_chain() {
    let error = CompletionError::new(CompletionErrorKind::Transport, "down")
        .with_source(std::io::Error::other("reset by peer"));
    let source = std::error::Error::source(&error).expect("the cause is kept");
    assert_eq!(source.to_string(), "reset by peer");
}

#[test]
fn with_finish_reason_is_read_back() {
    let error =
        CompletionError::new(CompletionErrorKind::EmptyReply, "empty").with_finish_reason("stop");
    assert_eq!(error.finish_reason(), Some("stop"));
}

#[test]
fn a_timeout_marked_transport_failure_is_a_timeout() {
    let timed_out = CompletionError::from(error_http(Timeout(Box::new(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "deadline",
    )))));
    assert_eq!(timed_out.kind(), CompletionErrorKind::Timeout);
    assert!(timed_out.is_timeout());
    assert!(timed_out.is_retryable());
    assert_eq!(
        timed_out.to_string(),
        "the model backend did not answer in time"
    );
    assert_eq!(timed_out.status(), None);
    assert!(std::error::Error::source(&timed_out).is_some());

    let plain = CompletionError::from(error_http(std::io::Error::other("reset")));
    assert_eq!(plain.kind(), CompletionErrorKind::Transport);
    assert!(!plain.is_timeout());
    assert_eq!(
        plain.to_string(),
        "the connection to the model backend failed"
    );

    let body_read = CompletionError::from(Error::BackendBodyRead {
        status: 500,
        source: Box::new(Timeout(Box::new(std::io::Error::other("slow")))),
    });
    assert_eq!(body_read.kind(), CompletionErrorKind::Timeout);
    assert!(body_read.is_timeout());
    assert_eq!(body_read.status(), Some(500));
    assert_eq!(body_read.backend_body(), None);
}

#[test]
fn a_body_read_failure_boxing_a_timeout_error_is_a_timeout() {
    let read_error = CompletionError::new(
        CompletionErrorKind::Timeout,
        "the model backend did not answer in time",
    );
    let error = CompletionError::from(Error::BackendBodyRead {
        status: 502,
        source: Box::new(read_error),
    });
    assert_eq!(error.kind(), CompletionErrorKind::Timeout);
    assert_eq!(error.status(), Some(502));

    let other = CompletionError::from(Error::BackendBodyRead {
        status: 502,
        source: Box::new(CompletionError::new(
            CompletionErrorKind::Transport,
            "reset",
        )),
    });
    assert_eq!(other.kind(), CompletionErrorKind::Transport);
}

#[test]
fn a_backend_status_goes_through_the_classifier() {
    let error = CompletionError::from(Error::Backend {
        status: 503,
        body: "busy".to_owned(),
    });
    assert_eq!(error.kind(), CompletionErrorKind::Overloaded);
    assert_eq!(
        error.to_string(),
        "the model backend is overloaded (status 503)"
    );
    assert_eq!(error.status(), Some(503));
    assert_eq!(error.backend_body(), Some("busy"));
}

#[test]
fn malformed_and_empty_variants_keep_the_fixed_phrase_and_their_text_as_detail() {
    let malformed = CompletionError::from(Error::MalformedResponse("no choices".to_owned()));
    assert_eq!(malformed.kind(), CompletionErrorKind::MalformedResponse);
    assert_eq!(
        malformed.to_string(),
        "the model backend sent a reply that could not be understood"
    );
    assert_eq!(malformed.detail(), Some("no choices"));

    let decoded = CompletionError::from(Error::MalformedResponseSource {
        message: "chunk was not valid JSON".to_owned(),
        source: Box::new(std::io::Error::other("eof")),
    });
    assert_eq!(decoded.kind(), CompletionErrorKind::MalformedResponse);
    assert_eq!(decoded.detail(), Some("chunk was not valid JSON"));
    assert_eq!(
        std::error::Error::source(&decoded)
            .expect("the decode cause is kept")
            .to_string(),
        "eof"
    );

    let empty = CompletionError::from(Error::EmptyModelReply {
        detail: "empty model reply",
        finish_reason: Some("stop".to_owned()),
    });
    assert_eq!(empty.kind(), CompletionErrorKind::EmptyReply);
    assert_eq!(
        empty.to_string(),
        "the model replied with no text and no tool calls"
    );
    assert_eq!(empty.finish_reason(), Some("stop"));
    assert_eq!(empty.detail(), Some("empty model reply"));
}

#[test]
fn every_environment_configuration_and_lock_variant_is_unavailable() {
    for error in [
        Error::MissingEnv("URL".to_owned()),
        Error::InvalidEnv("URL".to_owned()),
        Error::InvalidConfig("bad endpoint".to_owned()),
        Error::Config {
            message: "key is unusable".to_owned(),
            source: Box::new(std::io::Error::other("empty")),
        },
        Error::GatewayDisabled,
        Error::ModelSetLock("poisoned".to_owned()),
    ] {
        let label = format!("{error:?}");
        let converted = CompletionError::from(error);
        assert_eq!(
            converted.kind(),
            CompletionErrorKind::Unavailable,
            "{label}"
        );
        assert_eq!(
            converted.to_string(),
            "model access is turned off or not configured",
            "{label}"
        );
        assert!(!converted.is_retryable(), "{label}");
    }
}
