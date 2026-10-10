//! Model-task notices: how the model learns that a task it started ended.
//!
//! An author waits on a task through the `tasks` namespace; the model has
//! no wait primitive of its own beyond `await_tasks`, so the Engine tells
//! it. When a model-origin task reaches a terminal state, one sentence is
//! queued on the owner chain - `Task id=N (## Heading) completed: ...`,
//! `failed: ...`, `was canceled: the author cancelled it`, or
//! `was abandoned: <why>` - and reported as a `TaskNotice` under the
//! owner's section at that moment, so the log records the notice whether
//! or not a round ever reads it (an abandoned task's owner has ended, so
//! its notice is never read). A model-issued `task_cancel` queues nothing:
//! the model already read the built-in's confirmation.
//!
//! The queue drains in two places. The `chat` dispatch takes it ahead of
//! every `models.loop` round and pushes each text onto the author's list
//! as a user record, so the model reads the notices in that round; the
//! model's `await_tasks` drains the queue when it wakes and returns the
//! texts as its own answer. Either way each notice is read once.
//!
//! A completed task's final text is cross-chain model text reaching a
//! model without the author in between, so it is nonce-wrapped as
//! untrusted under the owner's run nonce; the rest of every sentence is
//! the Engine's own and stays bare.

use std::sync::atomic::Ordering;

use promptforge_types::ids::{AbandonReason, TaskId};

use crate::Error;

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

impl Scheduler {
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
        self.chains[owner.index()]
            .task_notices
            .push((task.clone(), text));
    }

    /// Takes every notice queued on `id`, in arrival order. Each notice
    /// is its task's delivery to the model, so taking it joins the task:
    /// everything the task did happens before the model's next step.
    pub(super) fn drain_task_notices(&mut self, id: ChainIndex) -> Vec<String> {
        let notices = std::mem::take(&mut self.chains[id.index()].task_notices);
        notices
            .into_iter()
            .map(|(task, text)| {
                self.join_task(id, &task);
                text
            })
            .collect()
    }
}
