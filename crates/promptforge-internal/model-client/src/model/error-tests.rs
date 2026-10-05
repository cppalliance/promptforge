//! Tests for the failure vocabulary: retryability per kind, the fixed
//! phrases, the specifics that extend them, and the detail channel.

use super::*;

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
fn a_timeout_and_a_transport_failure_are_built_from_a_kind_and_a_source() {
    let timed_out = CompletionError::new(
        CompletionErrorKind::Timeout,
        "the model backend did not answer in time",
    )
    .with_source(std::io::Error::new(
        std::io::ErrorKind::TimedOut,
        "deadline",
    ));
    assert_eq!(timed_out.kind(), CompletionErrorKind::Timeout);
    assert!(timed_out.is_retryable());
    assert_eq!(
        timed_out.to_string(),
        "the model backend did not answer in time"
    );
    assert_eq!(
        std::error::Error::source(&timed_out)
            .expect("the cause is kept")
            .to_string(),
        "deadline"
    );

    let refused = CompletionError::new(
        CompletionErrorKind::Transport,
        "the connection to the model backend failed",
    )
    .with_source(std::io::Error::other("connection refused"));
    assert_eq!(refused.kind(), CompletionErrorKind::Transport);
    assert!(refused.is_retryable());
}

#[test]
fn a_phrased_failure_shows_only_the_fixed_phrase() {
    for kind in ALL_KINDS {
        let error = CompletionError::phrased(kind);
        assert_eq!(error.kind(), kind);
        assert_eq!(error.to_string(), kind.phrase());
        assert_eq!(error.detail(), None);
    }
}

#[test]
fn a_specific_extends_the_phrase_after_a_colon_and_never_adds_detail() {
    let malformed = CompletionError::malformed("no choices in response");
    assert_eq!(malformed.kind(), CompletionErrorKind::MalformedResponse);
    assert_eq!(
        malformed.to_string(),
        "the model backend sent a reply that could not be understood: no choices in response"
    );
    assert_eq!(malformed.message(), malformed.to_string());
    assert_eq!(malformed.detail(), None);
    assert!(malformed.is_retryable());

    let empty = CompletionError::specific(
        CompletionErrorKind::EmptyReply,
        "reasoning content was present but ignored",
    )
    .with_finish_reason("stop");
    assert_eq!(
        empty.to_string(),
        "the model replied with no text and no tool calls: \
         reasoning content was present but ignored"
    );
    assert_eq!(empty.finish_reason(), Some("stop"));
    assert!(!empty.is_retryable());
}
