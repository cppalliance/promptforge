//! One performer trait for each chat, tool-call, and timer effect, and the
//! bundle the effect loop performs a run's effects through.
//!
//! The Engine issues an [`Effect`](promptforge::effect::Effect) as a
//! value and waits for its
//! [`EffectAnswer`](promptforge::effect::EffectAnswer); a performer is
//! the Harness code that turns the one into the other. Each trait takes the
//! effect's fields and returns the answer's payload for its kind, so a
//! performer never sees the run, the recorder, or another kind's effects. The
//! effect loop owns the correlation: it hands each result back to the run
//! under the effect's id and writes the answer's record.
//!
//! Each performer returns a boxed `'static` future that the loop polls
//! inside the run's own future, beside every other effect in flight, so a
//! performer must move what its future needs into it. A performer must not
//! block while polled: one that blocks stalls every other effect of the
//! run, and the run's stop and cancel with them. A performer with blocking
//! or CPU-heavy work hands it to the Host's own runtime and awaits the
//! result. A `Vfs` effect has no performer: the VFS is synchronous by
//! design, so the loop answers it inline through the Engine's store
//! operation.
//!
//! The runner supplies one performer itself, [`ActivatedTools`], over the
//! tool table run preparation activated. The Host supplies the
//! [`InferenceBroker`] and the [`Timer`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge::effect::Round;
use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema,
};
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use serde_json::Value;

#[path = "performers-tools.rs"]
mod tools;

pub use tools::ActivatedTools;

/// A boxed, sendable, owning future: what an asynchronous performer
/// returns and the loop polls.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// The Host's inference: lists the models it serves and performs a `Chat`
/// effect as one model round over `messages` with `tools` advertised,
/// under `binding`'s frozen `options`.
///
/// The Harness polls each round inside the run's own future, so a broker
/// must not block while polled; blocking or CPU-heavy work goes to the
/// Host's own runtime.
pub trait InferenceBroker: Send + Sync {
    /// Lists the models the broker serves.
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>>;

    /// Runs the round. `round` is the round's run-wide id and the path
    /// that dispatched it. The Harness takes only the finished reply; a
    /// broker that shows the reply as it forms streams it on its own.
    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        round: Round,
    ) -> BoxFuture<Result<Box<Completion>, CompletionError>>;
}

/// Performs a `ToolCall` effect: resolves `tool` to an implementation and
/// calls it with `args`.
pub trait ToolPerformer: Send + Sync {
    /// Calls the tool. `alias` is the prompt-local name the call used,
    /// for the performer's own diagnostics; `tool` is the identity it
    /// resolves.
    fn call(
        &self,
        tool: ToolId,
        alias: String,
        args: Value,
    ) -> BoxFuture<Result<ToolOutput, ToolError>>;
}

/// The Host's clock: performs a `Timer` effect as one sleep.
///
/// The Harness polls each sleep inside the run's own future, so a timer
/// must not block while polled; it waits on the Host's own runtime.
pub trait Timer: Send + Sync {
    /// Resolves once `seconds` have passed.
    fn sleep(&self, seconds: f64) -> BoxFuture<()>;
}

/// The Harness's performers: one for each chat, tool-call, and timer
/// effect. The loop answers a `Vfs` effect inline and has no performer
/// for it.
///
/// Shared handles, so the loop can move a performer into the future it
/// starts for each effect while the bundle stays whole.
#[derive(Clone)]
pub struct Performers {
    /// Performs `Chat` effects.
    pub broker: Arc<dyn InferenceBroker>,
    /// Performs `ToolCall` effects.
    pub tool: Arc<dyn ToolPerformer>,
    /// Performs `Timer` effects.
    pub timer: Arc<dyn Timer>,
}

impl std::fmt::Debug for Performers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Performers").finish_non_exhaustive()
    }
}
