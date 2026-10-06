//! How tasks end: a task chain's own end, and the chain-end rules that
//! end every task an ending chain owns.
//!
//! The chain-end rules: a task ends with its owner. When a chain ends,
//! every live task it owns is abandoned - its slot moves to `Abandoned`,
//! its terminal observation names how the owner ended, and its backing
//! chain aborts with everything it owns in turn. An author-origin task
//! left live is the author's bug, so the owner's own outcome becomes
//! `tasks_live` naming the leaked ids; a model-origin task is abandoned
//! quietly (the model learns through a notice). Tasks survive a section's
//! fall-through and a `jump` - those move the walk within one chain - and
//! end with the chain itself: the root walk, a `call` child, or another
//! task (a fanout arm among them). The H1 pass and the walk after it are
//! one chain, so the hand-off reassigns the pass's tasks to the walk.

use std::sync::Arc;

use promptforge_types::event::Event;
use promptforge_types::ids::{AbandonReason, TaskId, TaskOrigin};
use promptforge_vfs::{Access, ExecId};

use crate::{Error, Result};

use super::notices::TaskEnd;
use super::tasks::{TaskBacking, TaskState};
use super::{ChainIndex, Scheduler};

impl Scheduler {
    /// Applies a task chain's end to its slot: the outcome lands in the
    /// slot, the slot moves to `Done`, the task's terminal observation
    /// fires under its target section, a model task's notice is queued on
    /// its owner, and an owner parked on a set containing the task is
    /// woken with it delivered (the notice is queued first, so a model
    /// parked in `await_tasks` reads it in the wake's answer). Otherwise
    /// the slot holds the outcome until a wait takes it.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the chain has no slot, which only a
    /// scheduler bug produces: every task chain registers its slot before
    /// it is enqueued.
    pub(super) fn complete_task(&mut self, id: ChainIndex, outcome: Result<String>) -> Result<()> {
        let chain = &self.chains[id.index()];
        let task = chain.task.clone();
        // The terminal is the task's own last word, stamped with its task.
        let emitter = Arc::clone(chain.ctx.emitter());
        let Some(slot) = self.tasks.get_mut(&task) else {
            return Err(Error::internal("a task chain's end implies its slot"));
        };
        let succeeded = outcome.is_ok();
        slot.state = TaskState::Done;
        slot.ok = Some(succeeded);
        let owner = slot.owner;
        let origin = slot.origin;
        let target = slot.target.clone();
        emitter.emit(&target, |execution, section, provenance| {
            let task = task.clone();
            if succeeded {
                Event::TaskSucceeded {
                    execution,
                    section,
                    provenance,
                    task,
                }
            } else {
                Event::TaskFailed {
                    execution,
                    section,
                    provenance,
                    task,
                }
            }
        });
        if origin == TaskOrigin::Model {
            let end = match &outcome {
                Ok(text) => TaskEnd::Completed(text),
                Err(error) => TaskEnd::Failed(error),
            };
            self.queue_task_notice(owner, &task, &target, end);
        }
        if let Some(slot) = self.tasks.get_mut(&task) {
            slot.outcome = Some(outcome);
        }
        self.wake_waiter(owner, &task);
        Ok(())
    }

    /// Applies the chain-end rules for tasks to `owner`'s `outcome`: every
    /// live task the chain owns is abandoned (the reason names how the
    /// owner ended: the section ended, the tool loop was exhausted, or the
    /// owner failed some other way), and an `Ok` outcome that leaked
    /// author-origin tasks becomes [`Error::TasksLive`] naming them in
    /// spawn order. A failing chain keeps its own error - the leak is the
    /// lesser fault - but its tasks end all the same.
    pub(super) fn settle_owned_tasks(
        &mut self,
        owner: ChainIndex,
        outcome: Result<String>,
    ) -> Result<String> {
        let reason = match &outcome {
            Ok(_) => AbandonReason::OwnerReturned,
            Err(Error::ToolLoopExhausted) => AbandonReason::ToolLoopExhausted,
            Err(_) => AbandonReason::OwnerFailed,
        };
        let leaked = self.abandon_owned_tasks(owner, reason);
        match outcome {
            Ok(_) if !leaked.is_empty() => Err(Error::TasksLive { tasks: leaked }),
            outcome => outcome,
        }
    }

    /// Joins `task` into `owner`: merges the task's final clock into the
    /// owner's access, so everything the task did happens before the
    /// owner's next step. A timer (no identity) and an owner already
    /// ended (no access) need no join.
    pub(super) fn join_task(&self, owner: ChainIndex, task: &TaskId) {
        let Some(exec) = self.tasks.get(task).and_then(|slot| slot.exec) else {
            return;
        };
        let Ok(access) = self.chains[owner.index()].access() else {
            return;
        };
        promptforge_vfs::detail::access_join(access, exec);
    }

    /// Joins every task `owner` owns into `access`: the chain-end join,
    /// so the chain's own final clock transitively covers everything its
    /// tasks did even when no wait delivered them.
    pub(super) fn join_owned_tasks(&self, owner: ChainIndex, access: &Access) {
        let execs: Vec<ExecId> = self
            .tasks
            .values()
            .filter(|slot| slot.owner == owner)
            .filter_map(|slot| slot.exec)
            .collect();
        for exec in execs {
            promptforge_vfs::detail::access_join(access, exec);
        }
    }

    /// Ends every live task `owner` owns because `owner` is ending: each
    /// slot moves to `Abandoned`, its backing chain aborts with everything
    /// it owns in turn (or its in-flight request is dropped), and its
    /// terminal observation fires under its target with `reason` - the
    /// observation is the reason's record, since no wait can reach an
    /// abandoned slot once its owner is gone; a model task's abandonment
    /// notice is queued and reported too, though the ending owner never
    /// reads it. An internal timer slot ends the same way but reports
    /// nothing and never counts as leaked: it is the wait's detail, not a
    /// task the author started. Returns the abandoned author-origin ids in
    /// spawn order, for the owner's `tasks_live` outcome; the caller
    /// discards them for an owner that is itself being aborted, whose
    /// outcome no one receives.
    pub(super) fn abandon_owned_tasks(
        &mut self,
        owner: ChainIndex,
        reason: AbandonReason,
    ) -> Vec<TaskId> {
        // The arena is a hash map; ids order as paths and one owner's tasks
        // are its direct children, so sorting recovers spawn order.
        let mut live: Vec<(TaskId, TaskOrigin, TaskBacking, String)> = self
            .tasks
            .iter()
            .filter(|(_, slot)| slot.owner == owner && slot.state.is_live())
            .map(|(task, slot)| (task.clone(), slot.origin, slot.backing, slot.target.clone()))
            .collect();
        live.sort_by(|left, right| left.0.cmp(&right.0));
        let mut leaked = Vec::new();
        for (task, origin, backing, target) in live {
            if let Some(slot) = self.tasks.get_mut(&task) {
                slot.state = TaskState::Abandoned;
                slot.ok = Some(false);
            }
            // The backing ends first, so anything it owned reports before
            // the task's own terminal event, which is the last word on it.
            let backing_chain = match backing {
                TaskBacking::Chain(backing_chain) => backing_chain,
                TaskBacking::Effect(effect) => {
                    self.abort_effect(effect);
                    continue;
                }
            };
            // The terminal is stamped with the abandoned task's own
            // provenance: its backing chain's emitter, taken before the
            // abort clears the chain's state. A task that was never
            // admitted (still queued for a slot) has no start event, so
            // it reports no terminal either.
            let admitted = self.chains[backing_chain.index()].admitted;
            let emitter = Arc::clone(self.chains[backing_chain.index()].ctx.emitter());
            self.abort_subtree(backing_chain);
            if admitted {
                emitter.emit(&target, |execution, section, provenance| {
                    Event::TaskAbandoned {
                        execution,
                        section,
                        provenance,
                        task: task.clone(),
                        reason,
                    }
                });
            }
            match origin {
                TaskOrigin::Model => {
                    self.queue_task_notice(owner, &task, &target, TaskEnd::Abandoned(reason));
                }
                // `Author`, or an origin `promptforge-types` adds behind
                // its `#[non_exhaustive]` `TaskOrigin`: treated as the
                // author's. `#[non_exhaustive]` denies this crate an
                // exhaustive match, so a new variant lands here silently.
                // Only one test pins which origins leak:
                // `an_ending_owner_leaks_its_author_task_and_never_its_model_task`.
                _ => leaked.push(task),
            }
        }
        // The abandoned tasks' slots are free: the drain admits the
        // queued chains that now fit at its next edge.
        leaked
    }

    /// Ends every live task in the arena because the run itself is ending:
    /// the whole-run counterpart of the per-owner chain-end rule, so a
    /// task stranded by a run cancel or a fatal answer still receives its
    /// one terminal before the run's end boundary. Each live slot's owner
    /// is passed to [`abandon_owned_tasks`](Self::abandon_owned_tasks)
    /// with `reason`, in ascending arena order. No slot reports twice:
    /// abandoning a task aborts its backing chain, and `abort_subtree`
    /// abandons that chain's own tasks (as `OwnerAborted`) on the way, so
    /// a nested slot is already terminal when its owner's turn comes and
    /// `is_live()` skips it. The leaked-author list is discarded: no one
    /// receives an outcome for a run that is ending.
    pub(super) fn settle_all_tasks(&mut self, reason: AbandonReason) {
        let mut owners: Vec<ChainIndex> = self
            .tasks
            .values()
            .filter(|slot| slot.state.is_live())
            .map(|slot| slot.owner)
            .collect();
        owners.sort_unstable_by_key(|owner| owner.index());
        owners.dedup();
        for owner in owners {
            self.abandon_owned_tasks(owner, reason);
        }
    }

    /// Moves every task `from` owns to `to`, with the notices not yet
    /// delivered: the H1 hand-off, where the pass and the walk are one
    /// chain (`0`) on either side, so a task the pass spawned is waited
    /// on, inspected, cancelled, or leaked by the walk exactly as if the
    /// walk had spawned it. The pass's admission accounting moves with
    /// the tasks: the walk inherits the slots the pass's tasks hold and
    /// the limit that gates them.
    pub(super) fn reassign_tasks(&mut self, from: ChainIndex, to: ChainIndex) {
        for slot in self.tasks.values_mut() {
            if slot.owner == from {
                slot.owner = to;
            }
        }
        self.chains[to.index()].slots_used += self.chains[from.index()].slots_used;
        self.chains[from.index()].slots_used = 0;
        // An effect-backed slot's effect is keyed under its owner in the
        // pending table; the pass has no parked effect of its own at the
        // hand-off, so every entry under it is such a slot's.
        for pending in self.pending.values_mut() {
            if pending.chain == from {
                pending.chain = to;
            }
        }
        for chain in &mut self.chains {
            if chain.owner == Some(from) {
                chain.owner = Some(to);
            }
        }
        let notices = std::mem::take(&mut self.chains[from.index()].task_notices);
        self.chains[to.index()].task_notices = notices;
    }
}
