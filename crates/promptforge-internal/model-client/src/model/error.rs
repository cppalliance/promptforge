//! The model failure vocabulary: the closed set of kinds a broker maps its
//! failures into, and the error that carries one.

use crate::classify::classify_http_failure;
use crate::error::BoxedSource;
use crate::{Error, Timeout};

/// What went wrong with a model round, as a closed set a caller can branch
/// on without reading status codes or response text.
///
/// Every broker maps its failures into these kinds, and retryability is
/// fixed per kind (see [`CompletionError::is_retryable`]).
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
///
/// # Examples
///
/// ```
/// use promptforge::model::CompletionErrorKind;
///
/// let kind = CompletionErrorKind::RateLimited;
/// let advice = match kind {
///     CompletionErrorKind::RateLimited | CompletionErrorKind::Overloaded => "back off, then retry",
///     CompletionErrorKind::ContextOverflow => "compact the conversation",
///     _ => "inspect",
/// };
/// assert_eq!(advice, "back off, then retry");
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CompletionErrorKind {
    /// The request exceeds the model's context window. Not retryable.
    ContextOverflow,
    /// The backend is limiting the request rate. Retryable.
    RateLimited,
    /// The billing or usage quota is spent. Not retryable.
    QuotaExhausted,
    /// The backend is temporarily at capacity. Retryable.
    Overloaded,
    /// The provider declined the content on policy grounds. Not retryable.
    Refused,
    /// No reply or next chunk arrived in time. Retryable.
    Timeout,
    /// The connection failed or the stream broke. Retryable.
    Transport,
    /// The backend reported a fault of its own. Retryable.
    ServerError,
    /// The backend refused the request for any other reason. Not retryable.
    Rejected,
    /// The reply could not be understood, or it exceeded the byte cap.
    /// Retryable.
    MalformedResponse,
    /// The model returned neither text nor tool calls. Not retryable.
    EmptyReply,
    /// Model access is turned off or not configured. Not retryable.
    Unavailable,
}

impl CompletionErrorKind {
    /// The fixed message for this kind: one lowercase phrase written for a
    /// model reader.
    pub(crate) fn phrase(self) -> &'static str {
        match self {
            CompletionErrorKind::ContextOverflow => {
                "the request is larger than the model's context window"
            }
            CompletionErrorKind::RateLimited => "the model backend is limiting the request rate",
            CompletionErrorKind::QuotaExhausted => {
                "the model backend says the usage quota is spent"
            }
            CompletionErrorKind::Overloaded => "the model backend is overloaded",
            CompletionErrorKind::Refused => {
                "the model backend refused the request on content policy grounds"
            }
            CompletionErrorKind::Timeout => "the model backend did not answer in time",
            CompletionErrorKind::Transport => "the connection to the model backend failed",
            CompletionErrorKind::ServerError => "the model backend reported a fault of its own",
            CompletionErrorKind::Rejected => "the model backend rejected the request",
            CompletionErrorKind::MalformedResponse => {
                "the model backend sent a reply that could not be understood"
            }
            CompletionErrorKind::EmptyReply => "the model replied with no text and no tool calls",
            CompletionErrorKind::Unavailable => "model access is turned off or not configured",
        }
    }

    fn is_retryable(self) -> bool {
        matches!(
            self,
            CompletionErrorKind::RateLimited
                | CompletionErrorKind::Overloaded
                | CompletionErrorKind::Timeout
                | CompletionErrorKind::Transport
                | CompletionErrorKind::ServerError
                | CompletionErrorKind::MalformedResponse
        )
    }
}

/// The error a model round or a catalog fetch fails with: what the broker
/// that performed it reports, and what comes back into the Engine in a
/// `Chat` effect's answer.
///
/// Holds a closed [`kind`](CompletionError::kind), a
/// [`message`](CompletionError::message) written for the operator and the
/// model, and optional extras: the token counts of a context overflow, the
/// choice's `finish_reason` for an empty reply, a provider
/// [`detail`](CompletionError::detail), and the underlying cause behind
/// [`std::error::Error::source`]. `Display` shows the message only. The
/// detail is provider text that the broker bounded and control-escaped; it
/// is an opt-in channel and never appears in `Display`.
///
/// A broker builds one with [`new`](CompletionError::new) or
/// [`context_overflow`](CompletionError::context_overflow) and adds the
/// extras with the `with_` methods. For an HTTP failure, build it with
/// [`classify_http_failure`](crate::client::classify_http_failure) instead.
/// `#[non_exhaustive]`.
///
/// # Examples
///
/// ```
/// use promptforge::model::{CompletionError, CompletionErrorKind};
///
/// let error = CompletionError::new(
///     CompletionErrorKind::Timeout,
///     "the model backend did not answer in time",
/// )
/// .with_detail("no chunk for 120 seconds");
/// assert_eq!(error.kind(), CompletionErrorKind::Timeout);
/// assert!(error.is_retryable());
/// assert_eq!(error.to_string(), "the model backend did not answer in time");
/// assert_eq!(error.detail(), Some("no chunk for 120 seconds"));
///
/// let overflow = CompletionError::context_overflow(
///     Some(5120),
///     Some(4096),
///     "the request is larger than the model's context window",
/// );
/// assert_eq!(overflow.overflow(), (Some(5120), Some(4096)));
/// assert!(!overflow.is_retryable());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct CompletionError {
    kind: CompletionErrorKind,
    message: String,
    prompt_tokens: Option<u32>,
    window: Option<u32>,
    finish_reason: Option<String>,
    detail: Option<String>,
    source: Option<BoxedSource>,
    /// The HTTP status behind the failure, kept for [`status`](Self::status).
    status: Option<u16>,
}

impl CompletionError {
    /// Builds a failure of `kind` with `message` as its display text.
    ///
    /// A broker uses the fixed phrase for the kind; the message must not
    /// carry provider text, which belongs in
    /// [`with_detail`](CompletionError::with_detail).
    #[must_use]
    pub fn new(kind: CompletionErrorKind, message: impl Into<String>) -> CompletionError {
        CompletionError {
            kind,
            message: message.into(),
            prompt_tokens: None,
            window: None,
            finish_reason: None,
            detail: None,
            source: None,
            status: None,
        }
    }

    /// Builds a [`ContextOverflow`](CompletionErrorKind::ContextOverflow)
    /// failure with the token counts the provider stated, `None` for a count
    /// it did not give.
    #[must_use]
    pub fn context_overflow(
        prompt_tokens: Option<u32>,
        window: Option<u32>,
        message: impl Into<String>,
    ) -> CompletionError {
        CompletionError {
            prompt_tokens,
            window,
            ..CompletionError::new(CompletionErrorKind::ContextOverflow, message)
        }
    }

    /// Keeps `source` as the underlying cause, reachable through
    /// [`std::error::Error::source`].
    #[must_use]
    pub fn with_source(
        mut self,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> CompletionError {
        self.source = Some(source.into());
        self
    }

    /// Records the choice's `finish_reason`, for an
    /// [`EmptyReply`](CompletionErrorKind::EmptyReply) failure.
    #[must_use]
    pub fn with_finish_reason(mut self, reason: impl Into<String>) -> CompletionError {
        self.finish_reason = Some(reason.into());
        self
    }

    /// Records provider text that explains the failure. The broker bounds
    /// and control-escapes it first; it never appears in `Display`.
    #[must_use]
    pub fn with_detail(mut self, text: impl Into<String>) -> CompletionError {
        self.detail = Some(text.into());
        self
    }

    /// Records the HTTP status behind the failure.
    pub(crate) fn with_status(mut self, status: u16) -> CompletionError {
        self.status = Some(status);
        self
    }

    /// Keeps an already boxed cause.
    fn with_boxed_source(mut self, source: BoxedSource) -> CompletionError {
        self.source = Some(source);
        self
    }

    /// Returns the closed classification of this failure.
    #[must_use]
    pub fn kind(&self) -> CompletionErrorKind {
        self.kind
    }

    /// Returns `true` when retrying may succeed. The answer is fixed per
    /// kind: `RateLimited`, `Overloaded`, `Timeout`, `Transport`,
    /// `ServerError`, and `MalformedResponse` are retryable; the rest are
    /// not.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Returns the token counts a context overflow stated, as
    /// `(prompt_tokens, window)`. A count the provider did not give, and
    /// every kind other than `ContextOverflow`, reads `None`.
    #[must_use]
    pub fn overflow(&self) -> (Option<u32>, Option<u32>) {
        (self.prompt_tokens, self.window)
    }

    /// Returns the choice's `finish_reason`, when the failure was an empty
    /// model reply and the backend supplied one.
    ///
    /// The tool loop gates on this: an empty turn with `Some("stop")` after
    /// successful tool calls is a clean exit, while a missing or `"length"`
    /// reason stays a hard failure.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Returns the message `Display` shows.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the provider text behind the failure, bounded and
    /// control-escaped by the broker.
    ///
    /// This is an explicit opt-in diagnostic channel: the text never
    /// appears in [`Display`](std::fmt::Display), so a hostile or sensitive
    /// payload cannot forge log lines or leak into an error message.
    #[must_use]
    pub fn detail(&self) -> Option<&str> {
        self.detail.as_deref()
    }

    /// Returns the bounded, control-escaped backend error body, when the
    /// failure was a non-success backend status.
    ///
    /// The same text as [`detail`](CompletionError::detail) for an HTTP
    /// status failure; `None` for every other failure.
    #[must_use]
    pub fn backend_body(&self) -> Option<&str> {
        self.status.and(self.detail())
    }

    /// Returns the backend HTTP status behind the failure, when there was
    /// one. A failure to read an error body keeps the status the backend had
    /// already returned.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Returns `true` when the failure was a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        self.kind == CompletionErrorKind::Timeout
    }
}

impl std::fmt::Display for CompletionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CompletionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}

/// Classifies the crate's error type into a kind. A transport failure is a
/// `Timeout` when the transport marked it with [`Timeout`] and a `Transport`
/// failure otherwise; a backend status goes through the HTTP classifier; the
/// environment, configuration, and lock variants are `Unavailable`. The
/// variant's own text is kept as the detail.
impl From<Error> for CompletionError {
    fn from(error: Error) -> Self {
        match error {
            Error::Backend { status, body } => classify_http_failure(status, &body),
            Error::BackendBodyRead { status, source } => transport(source).with_status(status),
            Error::Http(source) => transport(source),
            Error::MalformedResponse(message) => {
                CompletionError::phrased(CompletionErrorKind::MalformedResponse)
                    .with_detail(message)
            }
            Error::MalformedResponseSource { message, source } => {
                CompletionError::phrased(CompletionErrorKind::MalformedResponse)
                    .with_detail(message)
                    .with_boxed_source(source)
            }
            Error::EmptyModelReply {
                detail,
                finish_reason,
            } => {
                let error =
                    CompletionError::phrased(CompletionErrorKind::EmptyReply).with_detail(detail);
                match finish_reason {
                    Some(reason) => error.with_finish_reason(reason),
                    None => error,
                }
            }
            Error::Config { message, source } => {
                CompletionError::phrased(CompletionErrorKind::Unavailable)
                    .with_detail(message)
                    .with_boxed_source(source)
            }
            Error::GatewayDisabled => CompletionError::phrased(CompletionErrorKind::Unavailable),
            error @ (Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::ModelSetLock(_)) => CompletionError::phrased(CompletionErrorKind::Unavailable)
                .with_detail(error.to_string()),
        }
    }
}

impl CompletionError {
    /// Builds a failure whose message is the kind's fixed phrase.
    fn phrased(kind: CompletionErrorKind) -> CompletionError {
        CompletionError::new(kind, kind.phrase())
    }
}

/// A failed send or read: `Timeout` when the transport marked it (or the
/// source is itself a `Timeout` failure), else `Transport`.
fn transport(source: BoxedSource) -> CompletionError {
    let timed_out = source.downcast_ref::<Timeout>().is_some()
        || source
            .downcast_ref::<CompletionError>()
            .is_some_and(CompletionError::is_timeout);
    let kind = if timed_out {
        CompletionErrorKind::Timeout
    } else {
        CompletionErrorKind::Transport
    };
    CompletionError::phrased(kind).with_boxed_source(source)
}

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
