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
/// # Invariants
///
/// - [`wait`](InputBroker::wait) returns the operator's text byte-exact:
///   no trimming, no re-encoding, no wrapping.
/// - A wait whose future is dropped, as happens when the run is
///   cancelled, must not leave a prompt open for the operator. It must
///   not panic either.
/// - [`wait`](InputBroker::wait) must not block while it is polled. The
///   Harness polls it inside the run's own future, beside every other
///   effect of the run. A broker therefore hands any blocking work to the
///   Host's own runtime.
#[async_trait::async_trait]
pub trait InputBroker: Send + Sync {
    /// Waits for the operator's next message and returns it byte-exact.
    ///
    /// # Errors
    /// Returns an [`InputError`] when the wait ends without an answer, for
    /// example because the Host withdrew the wait.
    async fn wait(&self) -> Result<String, InputError>;
}

/// A broker's failure to produce the operator's message.
///
/// The broker writes the `Display` message. That message is safe to hand
/// to a model. Any underlying cause stays out of it and is returned by
/// [`std::error::Error::source`].
#[derive(Debug)]
#[non_exhaustive]
pub struct InputError {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl InputError {
    /// Builds a failure with only a message.
    #[must_use]
    pub fn message(text: impl Into<String>) -> InputError {
        InputError {
            message: text.into(),
            source: None,
        }
    }

    /// Builds a failure with a message and with `source` as its underlying
    /// cause, which the `Display` message does not show.
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
