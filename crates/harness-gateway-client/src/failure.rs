//! The failures this crate builds itself: each is a [`CompletionError`]
//! made from a kind and that kind's fixed phrase.
//!
//! A non-success HTTP status is not built here; `classify_http_failure`
//! reads its status and bounded body. These helpers cover the rest: a
//! send or read that went wrong, a disabled client, and a reply this
//! crate cannot read.

use std::fmt::Display;

use promptforge::model::{CompletionError, CompletionErrorKind};

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

/// The failure of a client built with `GatewayClient::disabled`: model
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
}
