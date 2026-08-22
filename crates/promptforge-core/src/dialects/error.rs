//! The dialect resolution error and its stable classifier.

use crate::Error;

/// A stable, matchable classification of a [`DialectError`].
///
/// `#[non_exhaustive]` so new kinds do not break a caller's `match`. Obtain one
/// from [`DialectError::kind`] and match on it instead of parsing the message.
///
/// ```
/// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
///
/// // Empty evidence matches no dialect, so resolution fails with `NoMatch`.
/// let registry = ToolDialectRegistry::builtin();
/// let error = registry.resolve(&DialectEvidence::default()).unwrap_err();
/// assert_eq!(error.kind(), DialectErrorKind::NoMatch);
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DialectErrorKind {
    /// No registered dialect scored on the provided evidence.
    NoMatch,
    /// Two or more dialects tied for the highest detection score.
    Tie,
    /// A named dialect was not present in the registry.
    Unknown,
}

/// The error returned by
/// [`ToolDialectRegistry::resolve`](crate::dialects::ToolDialectRegistry::resolve).
///
/// Carries a stable [`kind`](DialectError::kind) classifier. `#[non_exhaustive]`
/// with a private representation; callers obtain one from a failed resolve and
/// inspect it through [`kind`](DialectError::kind) and
/// [`Display`](std::fmt::Display).
///
/// ```
/// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
///
/// let registry = ToolDialectRegistry::builtin();
/// let error = registry.resolve(&DialectEvidence::default()).unwrap_err();
/// assert_eq!(error.kind(), DialectErrorKind::NoMatch);
/// assert!(!error.to_string().is_empty());
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub struct DialectError {
    inner: Error,
}

impl DialectError {
    /// Returns the stable classification of this failure.
    ///
    /// ```
    /// use promptforge_core::dialects::{DialectEvidence, DialectErrorKind, ToolDialectRegistry};
    ///
    /// let registry = ToolDialectRegistry::builtin();
    /// let evidence = DialectEvidence::new(Some(true), None, None, None);
    /// // A single strong match resolves cleanly; a miss classifies as `NoMatch`.
    /// assert!(registry.resolve(&evidence).is_ok());
    /// let miss = registry.resolve(&DialectEvidence::default()).unwrap_err();
    /// assert_eq!(miss.kind(), DialectErrorKind::NoMatch);
    /// ```
    #[must_use]
    pub fn kind(&self) -> DialectErrorKind {
        match &self.inner {
            Error::DialectNone => DialectErrorKind::NoMatch,
            Error::DialectTie { .. } => DialectErrorKind::Tie,
            Error::UnknownDialect(_)
            | Error::ParseFrontmatter { .. }
            | Error::ParseStructured { .. }
            | Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::GatewayDisabled
            | Error::Http(_)
            | Error::Backend { .. }
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::BackendBodyRead { .. }
            | Error::EmptyModelReply { .. }
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
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::Tool { .. }
            | Error::FanoutArmJoin(_)
            | Error::Internal(_)
            | Error::LuaQuota { .. }
            | Error::TimestampFormat(_) => DialectErrorKind::Unknown,
        }
    }
}

impl std::fmt::Display for DialectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl std::error::Error for DialectError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        std::error::Error::source(&self.inner)
    }
}

impl From<Error> for DialectError {
    fn from(inner: Error) -> Self {
        DialectError { inner }
    }
}

impl From<DialectError> for Error {
    fn from(error: DialectError) -> Self {
        error.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unrelated_errors_classify_as_unknown() {
        let error = DialectError::from(Error::InvalidConfig("bad endpoint".to_owned()));
        assert_eq!(error.kind(), DialectErrorKind::Unknown);
    }
}
