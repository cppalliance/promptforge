//! The model failure vocabulary: the closed set of kinds a broker maps its
//! failures into, and the error that carries one.

/// A type-erased owned error cause.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

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
    /// Returns the fixed message for this kind: one lowercase phrase written
    /// for a model reader. A broker builds a [`CompletionError`] message
    /// from it. Only `MalformedResponse`, `EmptyReply`, and `Unavailable`
    /// may extend it with `: ` and a specific the broker's own code wrote
    /// (see [`CompletionError`]).
    ///
    /// # Examples
    ///
    /// ```
    /// use promptforge::model::{CompletionError, CompletionErrorKind};
    ///
    /// let kind = CompletionErrorKind::Timeout;
    /// assert_eq!(kind.phrase(), "the model backend did not answer in time");
    /// let error = CompletionError::new(kind, kind.phrase());
    /// assert_eq!(error.to_string(), kind.phrase());
    /// ```
    #[must_use]
    pub fn phrase(self) -> &'static str {
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
/// extras with the `with_` methods. For an HTTP failure, the
/// `harness-gateway-client` crate's `classify_http_failure` builds it.
/// `#[non_exhaustive]`.
///
/// The message is the kind's fixed phrase. `MalformedResponse`,
/// `EmptyReply`, and `Unavailable` may extend it with `: ` and a specific
/// that the broker's own code wrote (for example the byte limit that was
/// hit); provider text never goes in the message.
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
}

impl CompletionError {
    /// Builds a failure of `kind` with `message` as its display text.
    ///
    /// A broker uses the kind's fixed
    /// [`phrase`](CompletionErrorKind::phrase). For `MalformedResponse`,
    /// `EmptyReply`, and `Unavailable` it may extend the phrase with `: `
    /// and a specific its own code wrote. The message must not carry
    /// provider text, which belongs in
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

    /// Builds a failure whose message is the kind's fixed phrase.
    pub(crate) fn phrased(kind: CompletionErrorKind) -> CompletionError {
        CompletionError::new(kind, kind.phrase())
    }

    /// Builds a failure whose message is the kind's fixed phrase extended
    /// with `: ` and a specific this crate wrote.
    pub(crate) fn specific(
        kind: CompletionErrorKind,
        specific: impl std::fmt::Display,
    ) -> CompletionError {
        CompletionError::new(kind, format!("{}: {specific}", kind.phrase()))
    }

    /// Builds a `MalformedResponse` failure naming what was wrong with the
    /// reply.
    pub(crate) fn malformed(specific: impl std::fmt::Display) -> CompletionError {
        CompletionError::specific(CompletionErrorKind::MalformedResponse, specific)
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

#[cfg(test)]
#[path = "error-tests.rs"]
mod tests;
