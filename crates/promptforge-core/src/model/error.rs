//! The gateway transport error and its stable classifier.

use crate::Error;

/// A stable, matchable classification of a [`CompletionError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`.
///
/// # Examples
///
/// ```
/// use promptforge_core::model::CompletionErrorKind;
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
    /// The client could not be configured (missing environment, bad endpoint,
    /// or dialect selection).
    Config,
}

/// The error returned by the gateway transport ([`crate::client::GatewayClient`]
/// completion and catalog calls) and [`fetch_model_catalog`](super::fetch_model_catalog).
///
/// Carries a stable [`kind`](CompletionError::kind) classifier plus the
/// `is_retryable`/`is_timeout`/`status` predicates, and preserves the underlying
/// transport cause through [`std::error::Error::source`]. `#[non_exhaustive]`
/// and not constructible outside the crate.
///
/// # Examples
///
/// ```no_run
/// # async fn run() {
/// use promptforge_core::model::{fetch_model_catalog, CompletionErrorKind};
///
/// if let Err(error) = fetch_model_catalog("http://127.0.0.1:8081/v1", "tok").await {
///     if error.kind() == CompletionErrorKind::Backend {
///         eprintln!("gateway returned status {:?}", error.status());
///     }
///     if error.is_retryable() {
///         // A transient transport/backend failure: safe to retry.
///     }
/// }
/// # }
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
            Error::ParseFrontmatter { .. }
            | Error::ParseStructured { .. }
            | Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::Interrupted
            | Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::Bind { .. }
            | Error::BindSchema { .. }
            | Error::BindQuery { .. }
            | Error::Absent { .. }
            | Error::Duplicate { .. }
            | Error::Ambiguous { .. }
            | Error::DuplicateAlias { .. }
            | Error::DuplicateLiveToolId { .. }
            | Error::ToolIdSelectedTwice { .. }
            | Error::PickedToolNotLive { .. }
            | Error::ToolScopeAnalysis { .. }
            | Error::ToolScopeAnalysisSource { .. }
            | Error::NearDuplicateTools { .. }
            | Error::ModelBind { .. }
            | Error::ModelBindQuery { .. }
            | Error::ModelAbsent { .. }
            | Error::ModelDuplicate { .. }
            | Error::ModelAmbiguous { .. }
            | Error::DuplicateModelAlias { .. }
            | Error::Substitution(_)
            | Error::ToolLoopExhausted
            | Error::OutOfScopeToolCall { .. }
            | Error::UnknownScopedTool(_)
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::DialectNone
            | Error::DialectTie { .. }
            | Error::UnknownDialect(_)
            | Error::Tool { .. }
            | Error::FanoutArmJoin(_)
            | Error::Internal(_)
            | Error::InvalidToolWireName { .. }
            | Error::LuaQuota { .. }
            | Error::TimestampFormat(_) => CompletionErrorKind::Config,
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

    /// Returns the backend HTTP status, when the failure was a backend status.
    #[must_use]
    pub fn status(&self) -> Option<u16> {
        match &self.inner {
            Error::Backend { status, .. } | Error::BackendBodyRead { status, .. } => Some(*status),
            _ => None,
        }
    }

    /// Returns `true` when the transport failure was a timeout.
    #[must_use]
    pub fn is_timeout(&self) -> bool {
        match &self.inner {
            Error::Http(source) | Error::BackendBodyRead { source, .. } => source
                .downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_timeout),
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
