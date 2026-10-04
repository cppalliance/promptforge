//! The model failure vocabulary: the closed set of kinds a broker maps its
//! failures into, and the error that carries one.

/// A type-erased owned error cause.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// The kind of failure a model round ended with.
///
/// The kinds form a closed set, so a caller can branch on the kind without
/// reading status codes or response text. The caller that performs a round
/// maps every failure into one of these kinds. Each kind is either always
/// retryable or never retryable (see [`CompletionError::is_retryable`]).
/// The enum is `#[non_exhaustive]`, so a `match` on it needs a wildcard arm.
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
    /// The reply could not be understood, or it was larger than the size
    /// limit for a reply. Retryable.
    MalformedResponse,
    /// The model returned neither text nor tool calls. Not retryable.
    EmptyReply,
    /// Model access is turned off or not configured. Not retryable.
    Unavailable,
}

impl CompletionErrorKind {
    /// Returns the fixed phrase for this kind: one lowercase phrase written
    /// for a model to read.
    ///
    /// The caller builds a [`CompletionError`] message from this phrase. For
    /// a failure that came from an HTTP status, it appends ` (status N)`. A
    /// 401 or 403 is `Unavailable`, and its message uses
    /// `the model backend did not accept the credentials` in place of this
    /// phrase. Only `MalformedResponse`, `EmptyReply`, and `Unavailable` may
    /// extend the phrase with `: ` and specific text that the caller's own
    /// code wrote. Provider text never enters the message. It goes in the
    /// [`detail`](CompletionError::detail) instead.
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

/// The error a model round or a catalog fetch fails with.
///
/// The caller that performs the round or fetch reports this error. For a
/// model round, it comes back into the Engine in a `Chat` effect's answer.
///
/// It holds a [`kind`](CompletionError::kind) from a closed set, a
/// [`message`](CompletionError::message) written for the operator and the
/// model, and optional extras: the token counts of a context overflow, the
/// backend's `finish_reason` for an empty reply, a provider
/// [`detail`](CompletionError::detail), and the underlying cause behind
/// [`std::error::Error::source`]. `Display` shows only the message. The
/// detail is provider text that the caller bounded and control-escaped. It
/// is an opt-in channel and never appears in `Display`.
///
/// The caller builds one with [`new`](CompletionError::new) or
/// [`context_overflow`](CompletionError::context_overflow) and adds the
/// extras with the `with_` methods.
///
/// The message is the kind's fixed phrase. For a failure that came from an
/// HTTP status, the caller appends ` (status N)`. A 401 or 403 is
/// `Unavailable`, and its message uses
/// `the model backend did not accept the credentials` in place of the
/// phrase. `MalformedResponse`, `EmptyReply`, and `Unavailable` may extend
/// the message with `: ` and specific text that the caller's own code
/// wrote, such as the byte limit that was hit. Provider text never enters
/// the message. It goes in the [`detail`](CompletionError::detail) instead.
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
    /// The caller uses the kind's fixed
    /// [`phrase`](CompletionErrorKind::phrase) as the message. For a failure
    /// that came from an HTTP status, it appends ` (status N)`. A 401 or 403
    /// is `Unavailable`, and its message uses
    /// `the model backend did not accept the credentials` in place of the
    /// phrase. For `MalformedResponse`, `EmptyReply`, and `Unavailable`, the
    /// caller may extend the phrase with `: ` and specific text that its own
    /// code wrote. Provider text never enters the message. The caller passes
    /// it to [`with_detail`](CompletionError::with_detail) instead.
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
    /// failure with the token counts the provider stated. Pass `None` for a
    /// count the provider did not give.
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

    /// Records the `finish_reason` the backend gave, for an
    /// [`EmptyReply`](CompletionErrorKind::EmptyReply) failure.
    #[must_use]
    pub fn with_finish_reason(mut self, reason: impl Into<String>) -> CompletionError {
        self.finish_reason = Some(reason.into());
        self
    }

    /// Records provider text that explains the failure. The caller bounds
    /// and control-escapes the text before passing it. The text never
    /// appears in `Display`.
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
    fn specific(kind: CompletionErrorKind, specific: impl std::fmt::Display) -> CompletionError {
        CompletionError::new(kind, format!("{}: {specific}", kind.phrase()))
    }

    /// Builds a `MalformedResponse` failure naming what was wrong with the
    /// reply.
    pub(crate) fn malformed(specific: impl std::fmt::Display) -> CompletionError {
        CompletionError::specific(CompletionErrorKind::MalformedResponse, specific)
    }

    /// Returns the kind of this failure.
    #[must_use]
    pub fn kind(&self) -> CompletionErrorKind {
        self.kind
    }

    /// Returns `true` when retrying may succeed. The answer depends only on
    /// the kind. `RateLimited`, `Overloaded`, `Timeout`, `Transport`,
    /// `ServerError`, and `MalformedResponse` are retryable, and the rest
    /// are not.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.kind.is_retryable()
    }

    /// Returns the token counts that a context overflow reported, as
    /// `(prompt_tokens, window)`. A count is `None` when the provider did
    /// not give it, and both are `None` for every kind other than
    /// `ContextOverflow`.
    #[must_use]
    pub fn overflow(&self) -> (Option<u32>, Option<u32>) {
        (self.prompt_tokens, self.window)
    }

    /// Returns the backend's `finish_reason` when the failure was an empty
    /// model reply and the backend supplied one.
    ///
    /// The Engine's tool loop reads this value. An empty reply with
    /// `Some("stop")` after at least one answered tool call, including a
    /// call whose tool failed, is the model's clean exit. An empty reply
    /// with a missing or `"length"` reason stays a hard failure.
    #[must_use]
    pub fn finish_reason(&self) -> Option<&str> {
        self.finish_reason.as_deref()
    }

    /// Returns the message `Display` shows.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns the provider text behind the failure, which the caller
    /// bounded and control-escaped.
    ///
    /// The detail is an explicit opt-in diagnostic channel. The text never
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
