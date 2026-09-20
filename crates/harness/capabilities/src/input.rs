//! The [`InputBroker`] trait: one host policy behind user input.
//!
//! The engine issues a `UserInput` effect when a section's `user_input()`
//! runs and suspends the section until the harness answers it with an
//! [`InputOutcome`] or an [`InputError`]. The broker is the harness-side
//! policy that produces that answer: a blocking broker parks the wait until
//! the operator delivers (the section's VM and message history stay
//! intact), an [`InputOutcome::Unavailable`] answer is the
//! unavailable-fallback policy, and an [`InputError`] is the failure policy,
//! raising a typed input failure at the Lua call site. The outcome and error
//! vocabulary is the engine's, reached through
//! [`promptforge_api_runtime::input`].

use promptforge_api_runtime::input::{InputError, InputOutcome};

/// The host policy behind user input: one asynchronous request per wait.
///
/// The harness's input performer calls [`user_input`](Self::user_input)
/// for each `UserInput` effect and answers the effect with the result.
/// An implementation that blocks until its host delivers input is the
/// blocking policy; answering [`InputOutcome::Unavailable`] is the
/// unavailable-fallback policy; an [`InputError`] is the failure policy.
/// Implementations must be `Send + Sync`, must not panic, and should
/// return promptly when the host tears the wait down.
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
