//! One performer trait per effect kind, and the bundle the effect loop
//! performs a run's effects through.
//!
//! The engine issues an [`Effect`](promptforge_api_runtime::Effect) as a
//! value and waits for its
//! [`EffectAnswer`](promptforge_api_runtime::EffectAnswer); a performer is
//! the host code that turns the one into the other. Each trait takes the
//! effect's fields and returns the answer's payload for its kind, so a
//! performer never sees the run, the log, or another kind's effects. The
//! effect loop owns the correlation: it hands each result back to the run
//! under the effect's id and writes the answer's record.
//!
//! The asynchronous performers return a boxed `'static` future the loop
//! spawns as its own task, so a performer must move what its future needs
//! into it. The store performer is synchronous: the VFS is synchronous by
//! design, and the loop runs the call on tokio's blocking pool.
//!
//! The runner supplies four performers itself - [`TokioTimer`],
//! [`VfsStore`], [`LogTaskEvents`], and [`ActivatedTools`] - because each
//! is machinery it already holds: tokio's timer wheel, the engine's store
//! operation, the run log, and the tool table run preparation activated.
//! The chat and input performers live with what they reach: the gateway
//! client and the session's input wait.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge_api_runtime::input::{InputError, InputOutcome};
use promptforge_api_runtime::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ToolSchema,
};
use promptforge_api_runtime::{StoreError, StoreOp, StoreOutcome};
use promptforge_api_types::event::Event;
use promptforge_api_types::ids::TaskId;
use promptforge_api_types::tools::{ToolError, ToolId, ToolOutput};
use serde_json::Value;
use shared_vfs::Access;

#[path = "performers-host.rs"]
mod host;
#[path = "performers-tools.rs"]
mod tools;

pub use host::{LogTaskEvents, TokioTimer, VfsStore};
pub use tools::ActivatedTools;

/// A boxed, sendable, owning future: what an asynchronous performer
/// returns and the loop spawns.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Performs a `Chat` effect: one model round over `messages` with `tools`
/// advertised, under `binding`'s frozen `options`.
pub trait ChatPerformer: Send + Sync {
    /// Runs the round. `stream` says whether the round's live deltas have
    /// a consumer (a section's `chat` round) or only the completed reply
    /// does (a nested `models.infer`).
    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        stream: bool,
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

/// Performs a `UserInput` effect: one wait for operator input.
pub trait InputPerformer: Send + Sync {
    /// Waits for the operator's text for `section` of `execution`, or
    /// reports that none is available.
    fn wait(
        &self,
        execution: String,
        section: String,
    ) -> BoxFuture<Result<InputOutcome, InputError>>;
}

/// Performs a `Store` effect: one store operation under the chain's
/// access capability.
///
/// Synchronous: the loop runs it on the blocking pool and drops the
/// access after it returns, so the claims the operation held release
/// before the answer reaches the run.
pub trait StorePerformer: Send + Sync {
    /// Performs `op` through `access`. The performer uses the capability
    /// as given and never derives, widens, or retains store scope from it.
    ///
    /// # Errors
    /// Returns the store's own failure, which the engine raises at the
    /// author's call site as a store error.
    fn perform(&self, access: &Access, op: StoreOp) -> Result<StoreOutcome, StoreError>;
}

/// Performs a `Timer` effect: one sleep.
pub trait TimerPerformer: Send + Sync {
    /// Resolves once `seconds` have passed.
    fn sleep(&self, seconds: f64) -> BoxFuture<()>;
}

/// Performs a `TaskEvents` effect: one read of a task's reported history.
pub trait TaskEventsPerformer: Send + Sync {
    /// Every event of `task` with a sequence number after `last` (all of
    /// them when `last` is `None`), in sequence order, as the host's log
    /// holds them.
    fn events(&self, task: TaskId, last: Option<u32>) -> BoxFuture<Vec<Event>>;
}

/// The host's performers, one per effect kind.
///
/// Shared handles, so the loop can move a performer into the task it
/// spawns for each effect while the bundle stays whole.
#[derive(Clone)]
pub struct Performers {
    /// Performs `Chat` effects.
    pub chat: Arc<dyn ChatPerformer>,
    /// Performs `ToolCall` effects.
    pub tool: Arc<dyn ToolPerformer>,
    /// Performs `UserInput` effects.
    pub input: Arc<dyn InputPerformer>,
    /// Performs `Store` effects.
    pub store: Arc<dyn StorePerformer>,
    /// Performs `Timer` effects.
    pub timer: Arc<dyn TimerPerformer>,
    /// Performs `TaskEvents` effects.
    pub task_events: Arc<dyn TaskEventsPerformer>,
}

impl std::fmt::Debug for Performers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Performers").finish_non_exhaustive()
    }
}
