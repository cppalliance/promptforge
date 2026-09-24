//! The host-supplied performers the tokio test driver performs a run's
//! `Chat`, `ToolCall`, and `UserInput` effects through.

use std::future::Future;
use std::pin::Pin;

use promptforge_types::tools::ToolError;

use crate::execute::{Effect, EffectAnswer};
use crate::input::InputOutcome;

/// A boxed, sendable future: what a [`Performer`] returns.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One effect kind's performer: a closure handed the whole [`Effect`]
/// that returns the future producing its [`EffectAnswer`]. The driver
/// spawns the future, so it must be `Send` and own what it needs.
pub type Performer = Box<dyn FnMut(Effect) -> BoxFuture<EffectAnswer> + Send>;

/// The host-supplied performers, one per effect kind a host performs.
///
/// A struct of boxed async closures, so a caller supplies behavior
/// without implementing anything from this module. The
/// engine-internal kinds (`Store`, `Timer`, `TaskEvents`) are the driver's
/// own.
///
/// [`Performers::refusing`] answers every kind with its refusal: a `Chat`
/// with a disabled-gateway completion error, a `ToolCall` with a
/// no-implementation tool error, and a `UserInput` with
/// [`InputOutcome::Unavailable`] - the unavailable-fallback policy. A
/// host starts from it and overrides the slots it supplies.
pub struct Performers {
    /// Performs a [`Effect::Chat`] and answers [`EffectAnswer::Chat`].
    pub chat: Performer,
    /// Performs a [`Effect::ToolCall`] and answers
    /// [`EffectAnswer::ToolCall`].
    pub tool_call: Performer,
    /// Performs a [`Effect::UserInput`] and answers
    /// [`EffectAnswer::UserInput`].
    pub user_input: Performer,
}

impl Performers {
    /// The refusing performers; see the type docs.
    #[must_use]
    pub fn refusing() -> Performers {
        Performers {
            chat: Box::new(|_| Box::pin(async { refuse_chat() })),
            tool_call: Box::new(|_| Box::pin(async { refuse_tool_call() })),
            user_input: Box::new(|_| Box::pin(async { refuse_user_input() })),
        }
    }
}

/// The `Chat` refusal: the disabled-gateway completion error.
pub(crate) fn refuse_chat() -> EffectAnswer {
    EffectAnswer::Chat(Err(promptforge_model_client::Error::GatewayDisabled.into()))
}

/// The `ToolCall` refusal: the id resolves to no implementation.
pub(crate) fn refuse_tool_call() -> EffectAnswer {
    EffectAnswer::ToolCall(Err(ToolError::message(
        "the tool the call names has no implementation in the host's table",
    )))
}

/// The `UserInput` refusal: the unavailable fallback.
pub(crate) fn refuse_user_input() -> EffectAnswer {
    EffectAnswer::UserInput(Ok(InputOutcome::Unavailable))
}

impl std::fmt::Debug for Performers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Performers").finish_non_exhaustive()
    }
}
