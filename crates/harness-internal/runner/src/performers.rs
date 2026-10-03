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
//! Each performer returns a boxed `'static` future the loop spawns as its
//! own task, so a performer must move what its future needs into it. A
//! `Vfs` effect has no performer: the VFS is synchronous by design, so the
//! loop answers it inline through the Engine's store operation.
//!
//! The runner supplies two performers itself - [`TokioTimer`] and
//! [`ActivatedTools`] - because each is machinery it already holds:
//! tokio's timer wheel and the tool table run preparation activated. The
//! Host supplies the [`InferenceBroker`].

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use promptforge::model::{
    Completion, CompletionError, CompletionOptions, Message, ModelBinding, ModelCatalog,
    StreamDelta, ToolSchema,
};
use promptforge::tools::{ToolError, ToolId, ToolOutput};
use serde_json::Value;

#[path = "performers-builtin.rs"]
mod builtin;
#[path = "performers-tools.rs"]
mod tools;

pub use builtin::TokioTimer;
pub use tools::ActivatedTools;

/// A boxed, sendable, owning future: what an asynchronous performer
/// returns and the loop spawns.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// Receives each live piece of a streaming model round as it arrives.
pub type OnDelta = Arc<dyn Fn(StreamDelta) + Send + Sync>;

/// The Host's inference: lists the models it serves and performs a `Chat`
/// effect as one model round over `messages` with `tools` advertised,
/// under `binding`'s frozen `options`.
pub trait InferenceBroker: Send + Sync {
    /// Lists the models the broker serves.
    fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>>;

    /// Runs the round. `on_delta` is present when the round's live deltas
    /// have a consumer (a section's `chat` round) and `None` when only the
    /// completed reply does (a nested `models.infer`).
    fn chat(
        &self,
        binding: ModelBinding,
        messages: Vec<Message>,
        tools: Vec<ToolSchema>,
        options: CompletionOptions,
        on_delta: Option<OnDelta>,
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

/// Performs a `Timer` effect: one sleep.
pub trait TimerPerformer: Send + Sync {
    /// Resolves once `seconds` have passed.
    fn sleep(&self, seconds: f64) -> BoxFuture<()>;
}

/// The Harness's performers: one for each chat, tool-call, and timer
/// effect, and the callback a streaming chat round's deltas go to. The
/// loop answers a `Vfs` effect inline and has no performer for it.
///
/// Shared handles, so the loop can move a performer into the task it
/// spawns for each effect while the bundle stays whole.
#[derive(Clone)]
pub struct Performers {
    /// Performs `Chat` effects.
    pub broker: Arc<dyn InferenceBroker>,
    /// Receives the live deltas of each `Chat` effect whose round has the
    /// `Chat` origin.
    pub on_delta: OnDelta,
    /// Performs `ToolCall` effects.
    pub tool: Arc<dyn ToolPerformer>,
    /// Performs `Timer` effects.
    pub timer: Arc<dyn TimerPerformer>,
}

impl std::fmt::Debug for Performers {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Performers").finish_non_exhaustive()
    }
}
