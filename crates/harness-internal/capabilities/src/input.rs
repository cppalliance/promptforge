//! The input broker: the part of the Host that carries a question to a
//! person; a capability waits on it for the operator's next message.
//!
//! When the Host has an operator, it supplies an [`InputBroker`] among
//! its services under [`INPUT_BROKER`](crate::INPUT_BROKER), and the run's
//! [`RunServices`](crate::RunServices) carry it; a Host without one (a
//! batch or eval Host) supplies none, and a capability reads that absence
//! as "there is nobody to ask". A Host binds each run's broker to whoever
//! launched the run, so a wait reaches the right operator without naming
//! the run or the section. A broker that is present stays present for the
//! whole run.

use std::fmt;

/// Waits for the operator's next message on a capability's behalf.
///
/// # Implementing
///
/// ```
/// use harness_capabilities::{InputBroker, InputError};
///
/// /// A broker whose operator always types the same text.
/// struct Scripted(&'static str);
///
/// #[async_trait::async_trait]
/// impl InputBroker for Scripted {
///     async fn wait(&self) -> Result<String, InputError> {
///         Ok(self.0.to_owned())
///     }
/// }
/// # let _ = Scripted("hello");
/// ```
///
/// # Invariants
///
/// - [`wait`](InputBroker::wait) returns the operator's text byte-exact:
///   no trimming, no re-encoding, no wrapping.
/// - A wait whose future is dropped (the run was cancelled) must not
///   leave the operator prompting against it, and must not panic.
/// - [`wait`](InputBroker::wait) must not block while polled: the Harness
///   polls it inside the run's own future, beside every other effect of
///   the run, so a broker hands any blocking work to the Host's own
///   runtime.
#[async_trait::async_trait]
pub trait InputBroker: Send + Sync {
    /// Waits for the operator's next message and returns it byte-exact.
    ///
    /// # Errors
    /// Returns an [`InputError`] when the wait ends without an answer, for
    /// example because the Host withdrew it.
    async fn wait(&self) -> Result<String, InputError>;
}

/// A broker's failure to produce the operator's message.
///
/// The `Display` message is broker-authored and safe to hand to a model; any
/// underlying cause hides behind [`std::error::Error::source`].
#[derive(Debug)]
#[non_exhaustive]
pub struct InputError {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl InputError {
    /// Builds a failure with only a message.
    ///
    /// # Examples
    /// ```
    /// use harness_capabilities::InputError;
    ///
    /// let error = InputError::message("the input wait was cancelled");
    /// assert_eq!(error.to_string(), "the input wait was cancelled");
    /// assert!(std::error::Error::source(&error).is_none());
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> InputError {
        InputError {
            message: text.into(),
            source: None,
        }
    }

    /// Builds a failure with `source` as the hidden cause.
    ///
    /// # Examples
    /// ```
    /// use harness_capabilities::InputError;
    ///
    /// let cause = std::io::Error::other("socket reset");
    /// let error = InputError::with_source("the operator's window closed", cause);
    /// assert_eq!(error.to_string(), "the operator's window closed");
    /// assert!(std::error::Error::source(&error).is_some());
    /// ```
    #[must_use]
    pub fn with_source(
        text: impl Into<String>,
        source: impl std::error::Error + Send + Sync + 'static,
    ) -> InputError {
        InputError {
            message: text.into(),
            source: Some(Box::new(source)),
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for InputError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_deref()
            .map(|source| source as &(dyn std::error::Error + 'static))
    }
}
