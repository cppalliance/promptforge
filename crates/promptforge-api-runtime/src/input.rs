//! The generic input broker: one host policy behind user input.
//!
//! The broker backs the script-side `user_input()` function only. A
//! section's direct `user_input()` call suspends on the broker and
//! resumes with `(text, available)`: `available` is `true` for real
//! operator text and `false` when the host had no input, in which case
//! `text` is the fixed [`INPUT_UNAVAILABLE_FALLBACK`] sentence. The flag
//! rides beside the text, so a human typing exactly the fallback sentence
//! can never spoof the unavailable state. No `user_input` tool is
//! advertised to the model: a `models.loop` scope carries exactly the
//! tools the prompt adds.
//!
//! The host policies are the broker's: a blocking broker parks the wait
//! until the host delivers (the section's VM and message history stay
//! intact), an unavailable answer (or no configured broker at all) is the
//! unavailable-fallback policy, and a broker error is the failure policy,
//! raising a typed [`RunErrorKind::Input`](crate::RunErrorKind::Input)
//! failure at the Lua call site. Waits and responses are recorded through
//! the run's [`Observer`](promptforge_api_types::observe::Observer) - a wait-opened observation and
//! a byte-exact `on_user_input` report - without any replay machinery.

use std::fmt;

/// The fixed sentence a `user_input` call carries when the host has no
/// input to give.
///
/// The sentence is deliberately unremarkable: the availability flag, not
/// the text, distinguishes the fallback from operator input, so the
/// sentence never needs to be unguessable.
pub const INPUT_UNAVAILABLE_FALLBACK: &str =
    "User input is unavailable in this host; continue without it.";

/// What the broker produced for one input request.
///
/// `#[non_exhaustive]`: a future policy (for example a deferred
/// continuation-capable wait) can add variants without breaking
/// implementors.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputOutcome {
    /// The operator supplied text, delivered byte-exact.
    Text(String),
    /// The host had no input to give: the call resolves to
    /// [`INPUT_UNAVAILABLE_FALLBACK`] with `available` false.
    Unavailable,
}

/// A broker's failure to produce input.
///
/// The message is host-authored and safe to surface at the Lua call site;
/// an underlying cause hides behind [`std::error::Error::source`].
#[derive(Debug)]
#[non_exhaustive]
pub struct InputError {
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl InputError {
    /// Builds a failure carrying only a message.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_runtime::input::InputError;
    ///
    /// let error = InputError::message("the input device is gone");
    /// assert_eq!(error.to_string(), "the input device is gone");
    /// ```
    #[must_use]
    pub fn message(text: impl Into<String>) -> InputError {
        InputError {
            message: text.into(),
            source: None,
        }
    }

    /// Builds a failure with `source` as the hidden `#[source]` cause.
    ///
    /// # Examples
    /// ```
    /// use promptforge_api_runtime::input::InputError;
    ///
    /// let cause = std::io::Error::other("socket reset");
    /// let error = InputError::with_source("the input device is gone", cause);
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

    /// Dissolves the error into its message and optional cause.
    pub(crate) fn into_parts(self) -> (String, Option<Box<dyn std::error::Error + Send + Sync>>) {
        (self.message, self.source)
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

/// The host policy behind user input: one asynchronous request per wait.
///
/// The executor calls [`user_input`](Self::user_input) when a section's
/// `user_input()` runs, and suspends the caller until the future
/// resolves. An implementation
/// that blocks until its host delivers input is the blocking policy;
/// answering [`InputOutcome::Unavailable`] is the unavailable-fallback
/// policy; an [`InputError`] is the failure policy. Implementations must
/// be `Send + Sync`, must not panic, and should return promptly when the
/// host tears the wait down.
#[async_trait::async_trait]
pub trait InputBroker: Send + Sync {
    /// Waits for the host's answer to one input request for `section` of
    /// `execution`.
    ///
    /// # Errors
    /// Returns an [`InputError`] when the host fails the wait rather than
    /// answering or declining it.
    async fn user_input(&self, execution: &str, section: &str) -> Result<InputOutcome, InputError>;
}
