//! The failures this crate builds itself: each is a [`CompletionError`]
//! made from a kind and that kind's fixed phrase.
//!
//! A non-success HTTP status is not built here; `classify_http_failure`
//! reads its status and bounded body. These helpers cover the rest: a
//! send or read that went wrong, a disabled client, and a reply this
//! crate cannot read.

use std::fmt::Display;

use runvil::model::{CompletionError, CompletionErrorKind};

/// A send or read that failed: `Timeout` when `error` was a timeout and
/// `Transport` otherwise, with `error` kept as the source.
pub(crate) fn transport_failure(error: reqwest::Error) -> CompletionError {
    if error.is_timeout() {
        return timeout(error);
    }
    let kind = CompletionErrorKind::Transport;
    CompletionError::new(kind, kind.phrase()).with_source(error)
}

/// A receive deadline that elapsed: a `Timeout` with the elapsed error as
/// its source.
pub(crate) fn elapsed(error: tokio::time::error::Elapsed) -> CompletionError {
    timeout(error)
}

fn timeout(source: impl std::error::Error + Send + Sync + 'static) -> CompletionError {
    let kind = CompletionErrorKind::Timeout;
    CompletionError::new(kind, kind.phrase()).with_source(source)
}

/// The failure of a client built with `GatewayChat::disabled`: model
/// access is off, so no round is sent.
pub(crate) fn unavailable() -> CompletionError {
    let kind = CompletionErrorKind::Unavailable;
    CompletionError::new(kind, kind.phrase())
}

/// A `MalformedResponse` failure: the kind's fixed phrase extended with
/// `: ` and `specific`, which this crate's own code wrote. Provider text
/// never goes in `specific`.
pub(crate) fn malformed(specific: impl Display) -> CompletionError {
    let kind = CompletionErrorKind::MalformedResponse;
    CompletionError::new(kind, format!("{}: {specific}", kind.phrase()))
}

/// A validating constructor's refusal of parts this crate decoded from a
/// reply: an `InvalidParts` refusal becomes the `MalformedResponse` the
/// backend's reply earned, keeping its specific, since another reply may
/// hold whole parts. Any other failure passes through.
pub(crate) fn decoded(error: CompletionError) -> CompletionError {
    let kind = CompletionErrorKind::InvalidParts;
    if error.kind() != kind {
        return error;
    }
    let message = error.message();
    let specific = message
        .strip_prefix(kind.phrase())
        .and_then(|rest| rest.strip_prefix(": "))
        .unwrap_or(message);
    malformed(specific)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn each_helper_uses_the_fixed_phrase_for_its_kind() {
        let elapsed_error = tokio::time::timeout(
            std::time::Duration::from_millis(1),
            std::future::pending::<()>(),
        )
        .await
        .expect_err("a pending future elapses");
        let timed_out = elapsed(elapsed_error);
        assert_eq!(timed_out.kind(), CompletionErrorKind::Timeout);
        assert_eq!(
            timed_out.to_string(),
            "the model backend did not answer in time"
        );
        assert!(std::error::Error::source(&timed_out).is_some());

        let disabled = unavailable();
        assert_eq!(disabled.kind(), CompletionErrorKind::Unavailable);
        assert_eq!(
            disabled.to_string(),
            "model access is turned off or not configured"
        );

        let unreadable = malformed("model list body exceeds the 8-byte limit");
        assert_eq!(unreadable.kind(), CompletionErrorKind::MalformedResponse);
        assert_eq!(
            unreadable.to_string(),
            "the model backend sent a reply that could not be understood: \
             model list body exceeds the 8-byte limit"
        );
        assert_eq!(unreadable.detail(), None);
    }

    #[test]
    fn a_decoded_parts_refusal_is_a_retryable_malformed_reply_with_its_specific() {
        use runvil::model::ToolCall;
        let refusal = ToolCall::from_parts(" ", "fetch", serde_json::json!({}))
            .expect_err("a blank id is refused");
        assert_eq!(refusal.kind(), CompletionErrorKind::InvalidParts);
        let reply = decoded(refusal);
        assert_eq!(reply.kind(), CompletionErrorKind::MalformedResponse);
        assert!(reply.is_retryable());
        assert_eq!(
            reply.to_string(),
            "the model backend sent a reply that could not be understood: tool call id was blank"
        );

        let passed = decoded(unavailable());
        assert_eq!(passed.kind(), CompletionErrorKind::Unavailable);
    }
}
