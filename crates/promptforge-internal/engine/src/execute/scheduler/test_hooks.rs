//! Test-only hooks over the scheduler's private state: the seams the
//! `execute::tests` suites drive the scheduler's edge paths through
//! (overflow bounds, inline answers, and task slots). Compiled only under
//! test; nothing here exists in a shipped engine.

use promptforge_types::ids::TaskId;

pub(crate) use super::tasks::TaskState;
use super::{ChainIndex, Scheduler};

impl Scheduler {
    /// Shrinks the chain-count bound so a test can drive the
    /// [`start_chain`](Self::start_chain) overflow path.
    pub(crate) fn set_max_chains_for_test(&mut self, limit: usize) {
        self.max_chains = limit;
    }

    /// Drops the admission limit of the chain at arena index `chain` to
    /// zero, a limit `tasks.concurrency` refuses, so a test can leave a
    /// queued task that no slot will ever admit.
    pub(crate) fn wedge_admission_for_test(&mut self, chain: u32) {
        self.chains[ChainIndex(chain).index()].concurrency = 0;
    }

    /// The number of leaf effects the run has issued so far, so a test
    /// can prove a dispatch was answered inline with no effect issued.
    pub(crate) fn leaf_requests_issued(&self) -> u64 {
        self.next_effect
    }

    /// The state of one task's slot, or `None` when no task with that id
    /// was ever started, so a test can prove a chain's end moved its slot.
    pub(crate) fn task_state_for_test(&self, task: &TaskId) -> Option<TaskState> {
        self.tasks.get(task).map(|slot| slot.state)
    }
}
