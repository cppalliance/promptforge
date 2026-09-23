//! What a performer task owes the loop: exactly one answer for its
//! effect, posted when the performer completes or, failing that, when the
//! task is torn down.

use std::sync::Arc;

use promptforge_api_runtime::{EffectAnswer, EffectId};
use promptforge_api_runtime::{StoreError, StoreOp, StoreOutcome};
use shared_vfs::Access;
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

/// Performs one store operation and releases its access before returning.
///
/// Claims-release ordering: the access clone drops after the operation
/// and before the answer posts, so the claims it holds release before a
/// resumed chain can acquire overlapping claims. The access is this
/// function's own parameter so the order holds on a panic too: the
/// unwind drops it here, before the caller's [`Answering`] guard posts.
pub(super) fn perform_store(
    store: &dyn StorePerformer,
    access: Arc<Access>,
    op: StoreOp,
) -> Result<StoreOutcome, StoreError> {
    let result = store.perform(&access, op);
    drop(access);
    result
}
