//! Tool output with mandatory trust, and the narrow model-safe tool error.

/// Whether a tool's output is trusted or must be treated as untrusted data.
///
/// Trust is mandatory and carried in [`ToolOutput`] so it cannot be forgotten:
/// an [`OutputTrust::Untrusted`] result is nonce-wrapped before it can reach
/// model input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum OutputTrust {
    /// The output was produced by trusted, first-party code.
    Trusted,
    /// The output contains attacker-influenceable external data.
    Untrusted,
}

/// The result of a successful [`Tool::call`](crate::tools::Tool::call),
/// carrying its text and trust.
///
/// Trust travels with the value so the executor never has to remember a
/// separate flag; construct with [`ToolOutput::trusted`] or
/// [`ToolOutput::untrusted`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ToolOutput {
    text: String,
    trust: OutputTrust,
}

impl ToolOutput {
    /// Builds a trusted output whose text is appended to the model verbatim.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// let out = ToolOutput::trusted("done");
    /// assert_eq!(out.trust(), OutputTrust::Trusted);
    /// assert_eq!(out.text(), "done");
    /// ```
    #[must_use]
    pub fn trusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Trusted,
        }
    }

    /// Builds an untrusted output that is nonce-wrapped before reaching a model.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// let out = ToolOutput::untrusted("<html>...");
    /// assert_eq!(out.trust(), OutputTrust::Untrusted);
    /// ```
    #[must_use]
    pub fn untrusted(text: impl Into<String>) -> ToolOutput {
        ToolOutput {
            text: text.into(),
            trust: OutputTrust::Untrusted,
        }
    }

    /// Borrows the output text.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::ToolOutput;
    ///
    /// assert_eq!(ToolOutput::trusted("hi").text(), "hi");
    /// ```
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Returns whether the output is trusted or untrusted.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{OutputTrust, ToolOutput};
    ///
    /// assert_eq!(ToolOutput::untrusted("x").trust(), OutputTrust::Untrusted);
    /// ```
    #[must_use]
    pub fn trust(&self) -> OutputTrust {
        self.trust
    }
}

/// A stable, matchable classification of a [`ToolError`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ToolErrorKind {
    /// The model supplied arguments the tool could not accept.
    InvalidArguments,
    /// The tool's backend refused or failed the request.
    Backend,
    /// The request failed at the transport layer (network, timeout).
    Transport,
    /// The run was cancelled before or during the call.
    Cancelled,
    /// Any other tool failure.
    Other,
}

/// A narrow, model-safe error from a [`Tool::call`](crate::tools::Tool::call).
///
/// The `Display` message is caller-facing and safe to hand back to the model;
/// any underlying cause is hidden behind [`std::error::Error::source`]. Match on
/// [`ToolError::kind`] rather than a private representation.
#[derive(Debug)]
#[non_exhaustive]
pub struct ToolError {
    kind: ToolErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl ToolError {
    /// Builds a model-safe error carrying only a message (kind `Other`).
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("could not read the page");
    /// assert_eq!(err.kind(), ToolErrorKind::Other);
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> ToolError {
        ToolError {
            kind: ToolErrorKind::Other,
            message: text.into(),
            source: None,
        }
    }

    /// Builds a model-safe error that keeps `src` as a hidden `#[source]` cause.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::ToolError;
    ///
    /// let io = std::io::Error::other("boom");
    /// let err = ToolError::with_source("backend failed", io);
    /// assert!(std::error::Error::source(&err).is_some());
    /// ```
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
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("bad args").with_kind(ToolErrorKind::InvalidArguments);
    /// assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    /// ```
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
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("stopped").with_kind(ToolErrorKind::Cancelled);
    /// assert!(err.is_cancelled());
    /// ```
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        matches!(self.kind, ToolErrorKind::Cancelled)
    }

    /// Returns whether retrying the same call could plausibly succeed.
    ///
    /// # Examples
    /// ```
    /// use promptforge_core::tools::{ToolError, ToolErrorKind};
    ///
    /// let err = ToolError::message("timeout").with_kind(ToolErrorKind::Transport);
    /// assert!(err.is_retryable());
    /// ```
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
