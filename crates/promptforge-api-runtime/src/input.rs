//! The user-input vocabulary: what a `UserInput` effect is answered with.
//!
//! A section's direct `user_input()` call issues a `UserInput` effect and
//! resumes with `(text, available)`: `available` is `true` for real
//! operator text and `false` when the host had no input, in which case
//! `text` is the fixed [`INPUT_UNAVAILABLE_FALLBACK`] sentence. The flag
//! sits beside the text, so a human typing exactly the fallback sentence
//! can never spoof the unavailable state. No `user_input` tool is
//! advertised to the model: a `models.loop` scope includes exactly the
//! tools the prompt adds.
//!
//! The host policies sit behind the effect: a blocking host parks the
//! wait until the operator delivers (the section's VM and message history
//! stay intact), an [`InputOutcome::Unavailable`] answer is the
//! unavailable-fallback policy, and an [`InputError`] is the failure
//! policy, raising a typed [`RunErrorKind::Input`](crate::RunErrorKind::Input)
//! failure at the Lua call site. The policy trait a host implements
//! (`InputPerformer`) is the harness's, in `harness-runner`; the engine
//! knows only this answer vocabulary. Waits and responses are reported as
//! events - a wait-opened event and a byte-exact `UserInput` report -
//! without any replay machinery.

use std::fmt;

/// The fixed sentence a `user_input` call returns when the host has no
/// input to give.
///
/// The sentence is deliberately unremarkable: the availability flag, not
/// the text, distinguishes the fallback from operator input, so the
/// sentence never needs to be unguessable.
pub const INPUT_UNAVAILABLE_FALLBACK: &str =
    "User input is unavailable in this host; continue without it.";

/// What the host produced for one input request.
///
/// `#[non_exhaustive]`: a future policy (for example a deferred
/// continuation-capable wait) can add variants without breaking hosts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum InputOutcome {
    /// The operator supplied text, delivered byte-exact.
    Text(String),
    /// The host had no input to give: the call resolves to
    /// [`INPUT_UNAVAILABLE_FALLBACK`] with `available` false.
    Unavailable,
}

/// The host's failure to produce input.
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
    /// Builds a failure with only a message.
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
