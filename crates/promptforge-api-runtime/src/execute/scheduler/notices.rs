//! Model-task notices: how the model learns that a task it started ended.
//!
//! An author waits on a task through the `tasks` namespace; the model has
//! no wait primitive of its own beyond `await_tasks`, so the engine tells
//! it. When a model-origin task reaches a terminal state, one sentence is
//! queued on the owner chain - `Task id=N (## Heading) completed: ...`,
//! `failed: ...`, `was canceled: the author cancelled it`, or
//! `was abandoned: <why>` - and reported as a `TaskNotice` under the
//! owner's section at that moment, so the log records the notice whether
//! or not a round ever reads it (an abandoned task's owner has ended, so
//! its notice is never read). A model-issued `task_cancel` queues nothing:
//! the model already read the built-in's confirmation.
//!
//! The queue drains in two places. The loop shim yields
//! `drain_task_notices` ahead of every `chat` round and appends each text
//! as a user record, so the model reads the notices in its next round;
//! the model's `await_tasks` drains the queue when it wakes and returns
//! the texts as its own answer. Either way each notice is read once.
//!
//! A completed task's final text is cross-chain model text reaching a
//! model without the author in between, so it is nonce-wrapped as
//! untrusted under the owner's run nonce; the rest of every sentence is
//! the engine's own and stays bare.

use std::sync::atomic::Ordering;

use promptforge_api_types::ids::{AbandonReason, TaskId};

use crate::Error;
use crate::execute::protocol::Answer;

use super::{ChainIndex, Scheduler};

/// How a model task ended, as the notice tells it.
#[derive(Clone, Copy)]
pub(super) enum TaskEnd<'a> {
    /// The chain returned its final text.
    Completed(&'a str),
    /// The chain failed with this error.
    Failed(&'a Error),
    /// The author cancelled the task through `tasks.cancel`.
    CancelledByAuthor,
    /// The owner ended while the task was live.
    Abandoned(AbandonReason),
}

impl Scheduler<'_> {
    /// Queues one notice on `owner` for its model task `task` (started at
    /// `target`) that ended as `end`, and reports it as a `TaskNotice`
    /// under the owner's section. The completed text is nonce-wrapped
    /// under the owner's run nonce before it is embedded.
    pub(super) fn queue_task_notice(
        &mut self,
        owner: ChainIndex,
        task: &TaskId,
        target: &str,
        end: TaskEnd<'_>,
    ) {
        let chain = &self.chains[owner.index()];
        let head = format!("Task id={task} (## {target})");
        let text = match end {
            TaskEnd::Completed(result) => {
                format!("{head} completed: {}", chain.ctx.nonce().wrap(result))
            }
            TaskEnd::Failed(error) => format!("{head} failed: {error}"),
            TaskEnd::CancelledByAuthor => {
                format!("{head} was canceled: the author cancelled it")
            }
            TaskEnd::Abandoned(reason) => format!("{head} was abandoned: {}", reason.why()),
        };
        chain.ctx.emitter().task_notice(
            chain.section_name(),
            chain.ctx.turns().load(Ordering::Relaxed),
            task,
            &text,
        );
        self.chains[owner.index()].task_notices.push(text);
    }

    /// Takes every notice queued on `id`, in arrival order.
    pub(super) fn drain_task_notices(&mut self, id: ChainIndex) -> Vec<String> {
        std::mem::take(&mut self.chains[id.index()].task_notices)
    }

    /// Dispatches the loop shim's `drain_task_notices` request: the queued
    /// notices resume the chain at once.
    pub(super) fn dispatch_drain_task_notices(&mut self, id: ChainIndex) {
        let notices = self.drain_task_notices(id);
        self.chains[id.index()].incoming = Some(Answer::DrainTaskNotices(Ok(notices)));
        self.ready.push_back(id);
    }
}
