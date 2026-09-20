//! The gateway transport error and its stable classifier.

use crate::Error;

/// A stable, matchable classification of a [`CompletionError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
///
/// # Examples
///
/// ```
/// use promptforge_model_client::model::CompletionErrorKind;
///
/// let kind = CompletionErrorKind::Backend;
/// let retry_hint = match kind {
///     CompletionErrorKind::Transport | CompletionErrorKind::MalformedResponse => "retry",
///     _ => "inspect",
/// };
/// assert_eq!(retry_hint, "inspect");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionErrorKind {
    /// The HTTP request failed at the transport layer (connection, timeout).
    Transport,
    /// The backend returned a non-success status.
    Backend,
    /// The backend response could not be decoded or was structurally invalid.
    MalformedResponse,
    /// The model returned neither non-empty tool calls nor non-empty text.
    EmptyReply,
    /// Gateway access was explicitly disabled by the host.
    Disabled,
    /// The client could not be configured (missing environment, bad endpoint).
    Config,
}

/// The error a model round or a catalog fetch fails with: what the
/// transport that performed it (the harness's gateway client) reports, and
/// what a `Chat` effect's answer carries back into the engine.
///
/// Carries a stable [`kind`](CompletionError::kind) classifier plus the
/// `is_retryable`/`is_timeout`/`status` predicates, and preserves the underlying
/// transport cause through [`std::error::Error::source`]. `#[non_exhaustive]`
/// and constructible outside the crate only from the hidden substrate.
///
/// # Examples
///
/// ```
/// use promptforge_model_client::model::{CompletionError, CompletionErrorKind};
///
/// fn report(error: &CompletionError) -> &'static str {
///     if error.kind() == CompletionErrorKind::Backend {
///         return "gateway returned a non-success status";
///     }
///     if error.is_retryable() {
///         return "transient; safe to retry";
///     }
///     "inspect"
/// }
/// # let _ = report;
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct CompletionError {
    inner: Error,
}

impl CompletionError {
    /// Returns the stable classification of this failure.
    #[must_use]
    pub fn kind(&self) -> CompletionErrorKind {
        match &self.inner {
            Error::Http(_) | Error::BackendBodyRead { .. } => CompletionErrorKind::Transport,
            Error::Backend { .. } => CompletionErrorKind::Backend,
            Error::MalformedResponse(_) | Error::MalformedResponseSource { .. } => {
                CompletionErrorKind::MalformedResponse
            }
            Error::EmptyModelReply { .. } => CompletionErrorKind::EmptyReply,
            Error::GatewayDisabled => CompletionErrorKind::Disabled,
            Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::ModelSetLock(_) => CompletionErrorKind::Config,
        }
    }

    /// Returns the choice's `finish_reason`, when the failure was an empty
    /// model reply and the backend supplied one.
    ///
    /// The tool loop gates on this: an empty turn with `Some("stop")` after
    /// successful tool calls is a clean exit, while a missing or `"length"`
    /// reason stays a hard failure.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        match &self.inner {
            Error::EmptyModelReply { finish_reason, .. } => finish_reason.as_deref(),
            _ => None,
        }
    }

    /// Returns the bounded, control-escaped backend error body, when the failure
    /// was a non-success backend status.
    ///
    /// This is an explicit opt-in diagnostic channel (F5): the raw body never
    /// rides in the public [`Display`](std::fmt::Display), so a hostile or
    /// sensitive payload cannot forge log lines or leak into an error message.
    /// The returned text is bounded and has its control characters escaped.
    #[must_use]
    pub fn backend_body(&self) -> Option<&str> {
        match &self.inner {
            Error::Backend { body, .. } => Some(body),
            _ => None,
        }
    }

    /// Returns the backend HTTP status carried by a status or body-read failure.
    ///
    /// `BackendBodyRead` failures are classified as
    /// [`CompletionErrorKind::Transport`] but still retain the response status.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match &self.inner {
            Error::Backend { status, .. } | Error::BackendBodyRead { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Returns `true` when the transport failure was a timeout.
    ///
    /// The transport marks a timeout by wrapping its own error in
    /// [`Timeout`](crate::Timeout); this crate names no HTTP client.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        match &self.inner {
            Error::Http(source) | Error::BackendBodyRead { source, .. } => {
                source.downcast_ref::<crate::Timeout>().is_some()
            }
            _ => false,
        }
    }

    /// Returns `true` when retrying may succeed (transient transport or 5xx).
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match &self.inner {
            Error::Http(_)
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::BackendBodyRead { .. } => true,
            Error::Backend { status, .. } => *status >= 500,
            _ => false,
        }
    }
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for CompletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for CompletionError {
    fn from(inner: Error) -> Self {
        CompletionError { inner }
    }
}

impl From<CompletionError> for Error {
    fn from(error: CompletionError) -> Self {
        error.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timeout_marked_transport_failure_reports_is_timeout() {
        let timed_out = CompletionError::from(Error::http(crate::Timeout(Box::new(
            std::io::Error::new(std::io::ErrorKind::TimedOut, "deadline"),
        ))));
        assert!(timed_out.is_timeout());
        assert_eq!(timed_out.kind(), CompletionErrorKind::Transport);
        let plain = CompletionError::from(Error::http(std::io::Error::other("reset")));
        assert!(!plain.is_timeout());
        let body_read = CompletionError::from(Error::BackendBodyRead {
            status: 500,
            source: Box::new(crate::Timeout(Box::new(std::io::Error::other("slow")))),
        });
        assert!(body_read.is_timeout());
        assert_eq!(body_read.status(), Some(500));
    }

    #[test]
    fn every_config_variant_classifies_as_config() {
        for error in [
            Error::MissingEnv("URL".to_owned()),
            Error::InvalidEnv("URL".to_owned()),
            Error::InvalidConfig("bad endpoint".to_owned()),
        ] {
            assert_eq!(
                CompletionError::from(error).kind(),
                CompletionErrorKind::Config
            );
        }
    }
}
