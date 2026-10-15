//! Test-only hooks over the scheduler's private state: the seams the
//! `execute::tests` suites drive the scheduler's edge paths through
//! (overflow bounds, inline answers, and task slots). Compiled only under
//! test; nothing here exists in a shipped Engine.

use promptforge_types::ids::{ChainId, TaskId};
use promptforge_types::tools::ToolId;

use crate::Result;
use crate::execute::scope::DispatchTarget;
use crate::execute::walk_target::SlicePath;

pub(crate) use super::tasks::TaskState;
use super::{ChainIndex, Counters, Scheduler};

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

    /// Adds `name`, bound to `tool`, to the scope the chain at arena index
    /// `chain` advertised for its last round. Nothing changes the run's
    /// offering, so no round advertises a name it cannot bind; this lets a
    /// test answer the round with a call that binds to nothing.
    pub(crate) fn advertise_for_test(&mut self, chain: u32, name: &str, tool: ToolId) {
        self.chains[ChainIndex(chain).index()]
            .advertised
            .as_mut()
            .expect("the chain advertised a round")
            .insert(name.to_owned(), DispatchTarget::Bound(tool));
    }

    /// The state of one task's slot, or `None` when no task with that id
    /// was ever started, so a test can prove a chain's end moved its slot.
    pub(crate) fn task_state_for_test(&self, task: &TaskId) -> Option<TaskState> {
        self.tasks.get(task).map(|slot| slot.state)
    }

    /// Starts a chain over the top-level slice at `index`, as a jump out
    /// of the H1 pass starts the walk, and returns the section name the
    /// chain reports before its first entry.
    ///
    /// # Errors
    /// Returns [`Error::Internal`](crate::Error::Internal) when the run's
    /// chain count exceeds its bound.
    pub(crate) fn name_before_first_entry_for_test(&mut self, index: usize) -> Result<String> {
        let ctx = self.ctx.clone();
        let id = self.start_chain(
            ChainId::root(),
            Counters::default(),
            ctx,
            SlicePath::root(),
            index,
            None,
            serde_json::json!({}),
            0,
            1,
        )?;
        Ok(self.chains[id.index()].section_name().to_owned())
    }
}
