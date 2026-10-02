//! What an effect owes the loop: exactly one answer. A performer task
//! posts its answer when the performer completes or, failing that, when
//! the task is torn down; a Vfs effect is answered inline by
//! [`answer_vfs`].

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use promptforge::effect::{EffectAnswer, EffectId};
use promptforge::ids::Provenance;
use promptforge::vfs::{Access, VfsOp, perform_vfs_op};
use tokio::sync::mpsc;

/// The send half every performer task posts its answer to.
pub(super) type AnswerSender = mpsc::UnboundedSender<(EffectId, EffectAnswer)>;

/// The answer a performer task owes its effect.
///
/// Posted through [`Answering::post`] when the performer completes. A
/// task that ends any other way - a panic tokio caught, an abort - never
/// reaches its `post`, so the guard's drop posts `Dropped` in its place:
/// the loop hears from every performer it started, and a lost performer
/// cannot leave its effect unanswered and the run waiting forever. An
/// aborted task's post is stale, since the loop answered its effect
/// before aborting it, and the loop discards it.
///
/// A send fails only when the driver is gone (a dropped driver whose
/// receiver closed); the answer is then moot.
pub(super) struct Answering {
    tx: AnswerSender,
    id: EffectId,
    posted: bool,
}

impl Answering {
    pub(super) fn new(tx: AnswerSender, id: EffectId) -> Self {
        Self {
            tx,
            id,
            posted: false,
        }
    }

    /// Posts the performer's answer and disarms the guard.
    pub(super) fn post(mut self, answer: EffectAnswer) {
        self.posted = true;
        let _ = self.tx.send((self.id, answer));
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        if !self.posted {
            let _ = self.tx.send((self.id, EffectAnswer::Dropped));
        }
    }
}

/// Answers one Vfs effect on the loop's own thread: performs `op`
/// through the store view the effect carries and returns the answer to
/// record and resume the run with.
///
/// The operation is synchronous by design, so no task, guard, or channel
/// is involved: the answer is the return value, and an [`Answering`] is
/// never made for a Vfs effect. A panic out of the backend is caught
/// here, logged against `id` and `provenance`, and answered `Dropped`, so
/// it ends the run as a cancelled one and never unwinds the loop.
///
/// The access is this function's own parameter, so it drops when the
/// function returns. The drop is hygiene only: claims follow
/// happens-before within the run's scope, and the run ends that scope at
/// `Done` however long any access is held.
pub(super) fn answer_vfs(
    id: EffectId,
    provenance: &Provenance,
    access: Arc<Access>,
    op: VfsOp,
) -> EffectAnswer {
    let result = catch_unwind(AssertUnwindSafe(|| perform_vfs_op(&access, op)));
    drop(access);
    match result {
        Ok(outcome) => EffectAnswer::Vfs(outcome),
        Err(_panic) => {
            tracing::error!(
                effect = %id,
                task = %provenance.task,
                "a Vfs operation panicked; its effect is dropped"
            );
            EffectAnswer::Dropped
        }
    }
}
