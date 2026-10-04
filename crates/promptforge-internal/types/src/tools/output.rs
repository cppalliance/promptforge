//! Tool output with mandatory trust, and the narrow model-safe tool error.

/// Whether a tool's output is trusted or must be treated as untrusted data.
///
/// Every [`ToolOutput`] carries a trust level, so it cannot be forgotten. An
/// [`OutputTrust::Untrusted`] result is nonce-wrapped before it can reach
/// model input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputTrust {
    /// The output was produced by trusted, first-party code.
    Trusted,
    /// The output contains external data that an attacker could influence.
    Untrusted,
}

/// The text a tool returns from a successful call, together with its trust
/// level.
///
/// Build one with [`ToolOutput::trusted`] or [`ToolOutput::untrusted`]. The
/// trust level travels with the text, so code that handles the output never
/// has to track it as a separate flag.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutput {
    text: String,
    trust: OutputTrust,
}

impl ToolOutput {
    /// Builds a trusted output whose text is added to model input verbatim.
    #[must_use]
    pub fn trusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Trusted,
        }
    }

    /// Builds an untrusted output that is nonce-wrapped before reaching a model.
    #[must_use]
    pub fn untrusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Untrusted,
        }
    }

    /// Borrows the output text.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether the output is trusted or untrusted.
    #[must_use]
    pub fn trust(&self) -> OutputTrust {
        self.trust
    }
}

/// A stable classification of a [`ToolError`] that code can match on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// The model supplied arguments the tool could not accept.
    InvalidArguments,
    /// The tool's backend refused or failed the request.
    Backend,
    /// The request failed at the transport layer, such as a network error or a
    /// timeout.
    Transport,
    /// The run was cancelled before or during the call.
    Cancelled,
    /// Any other tool failure.
    Other,
}

/// An error from a tool call, with a message that is safe to show the model.
///
/// The `Display` message is meant for the caller and is safe to hand back to
/// the model. Any underlying cause stays out of that message and is available
/// only through [`std::error::Error::source`]. Match on [`ToolError::kind`] to
/// tell failures apart.
#[derive(Debug)]
#[non_exhaustive]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ToolError {
    /// Builds an error from a message alone, with the kind `Other`.
    #[must_use]
    pub fn message(text: impl Into<String>) -> ToolError {
        ToolError {
            kind: ToolErrorKind::Other,
            message: text.into(),
            source: None,
        }
    }

    /// Builds a backend error that records `src` as its underlying cause.
    ///
    /// The cause is not part of the message. It is available only through
    /// `Error::source`. The kind starts as [`ToolErrorKind::Backend`]. Call
    /// [`ToolError::with_kind`] when the cause belongs to another kind.
    #[must_use]
    pub fn with_source(
        text: impl Into<String>,
        src: impl std::error::Error + Send + Sync + 'static,
    ) -> ToolError {
        ToolError {
            kind: ToolErrorKind::Backend,
            message: text.into(),
            source: Some(Box::new(src)),
        }
    }

    /// Sets the classification, returning the updated error.
    #[must_use]
    pub fn with_kind(mut self, kind: ToolErrorKind) -> ToolError {
        self.kind = kind;
        self
    }

    /// Returns the stable classification of this error.
    #[must_use]
    pub fn kind(&self) -> ToolErrorKind {
        self.kind
    }

    /// Returns whether the failure was a cancellation.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, ToolErrorKind::Cancelled)
    }

    /// Returns whether retrying the same call could plausibly succeed.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self.kind, ToolErrorKind::Transport)
    }
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ToolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|boxed| boxed.as_ref() as &(dyn std::error::Error + 'static))
    }
}
