//! Test-only hooks over the scheduler's private state: the seams the
//! `execute::tests` suites drive the driver's edge paths through
//! (overflow bounds, inline answers, task slots, the answer channel, and
//! the record of every issued effect). Compiled only under test; nothing
//! here exists in a shipped engine.

use std::sync::{Arc, Mutex};

use promptforge_api_types::ids::TaskId;
use tokio::sync::mpsc;

use crate::execute::run::{EffectAnswer, EffectId, EffectRecord};

use super::{Scheduler, TaskState};

impl Scheduler<'_> {
    /// Shrinks the chain-count bound so a test can drive the
    /// [`start_chain`](Self::start_chain) overflow path.
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.max_chains = limit;
    }

    /// The number of leaf effects the run has issued so far, so a test
    /// can prove a dispatch was answered inline with no spawned leaf work.
    pub(crate) fn leaf_requests_issued(&self) -> u64 {
        self.next_effect
    }

    /// The state of one task's slot, or `None` when no task with that id
    /// was ever started, so a test can prove a chain's end moved its slot.
    pub(crate) fn task_state_for_test(&self, task: &TaskId) -> Option<TaskState> {
        self.tasks.get(task).map(|slot| slot.state)
    }

    /// Posts an answer for an arbitrary effect id, so a test can drive
    /// the driver's unknown-answer paths directly.
    pub(crate) fn post_answer_for_test(&self, effect: u64, answer: EffectAnswer) {
        self.performers.post_for_test(EffectId(effect), answer);
    }

    /// A clone of the answer channel's send half, so a test double can
    /// post answers from inside a performer while the driver runs.
    pub(crate) fn answer_sender_for_test(&self) -> mpsc::UnboundedSender<(EffectId, EffectAnswer)> {
        self.performers.sender_for_test()
    }

    /// Records the record of every effect the run issues, in issue order,
    /// so a test can assert on the effects themselves.
    pub(crate) fn record_effects_for_test(&mut self) -> Arc<Mutex<Vec<EffectRecord>>> {
        self.performers.record_effects_for_test()
    }
}
