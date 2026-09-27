//! What a performer task owes the loop: exactly one answer for its
//! effect, posted when the performer completes or, failing that, when the
//! task is torn down.

use std::sync::Arc;

use promptforge::effect::{EffectAnswer, EffectId};
use promptforge::vfs::Access;
use promptforge::vfs::{StoreOp, StoreOutcome, VfsError};
use tokio::sync::mpsc;

use crate::performers::StorePerformer;

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

/// Performs one store operation on the loop's behalf.
///
/// The access is this function's own parameter, so it drops when the
/// function returns - after the operation, before the caller's
/// [`Answering`] guard posts. The early drop is hygiene only: claims
/// follow happens-before within the run's scope, and the run ends that
/// scope at `Done` however long any access is held.
pub(super) fn perform_store(
    store: &dyn StorePerformer,
    access: Arc<Access>,
    op: StoreOp,
) -> Result<StoreOutcome, VfsError> {
    let result = store.perform(&access, op);
    drop(access);
    result
}
