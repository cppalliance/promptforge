//! The wait, inspection, note, and cancel arms over the task arena.
//!
//! `when_any` is the one scheduler wait primitive: a chain names a set of
//! tasks it owns and is resumed with the first member that ends - at once
//! when one already has, otherwise when a member's chain end delivers it.
//! Delivery moves a `Done` slot to `Delivered` (its outcome is taken by
//! exactly one wait; a second wait raises `task_consumed`); a `Cancelled`
//! slot delivers `ok = false` with the `cancelled` error value and stays
//! as it is, since it holds no result to consume. An `Abandoned` slot is
//! never delivered: a task is abandoned because its owner ended, and only
//! the owner may wait on it, so no wait can reach the slot.
//!
//! Ownership is the rule for every arm: only the chain that spawned a
//! task may wait on, check, list, or cancel it, and an id naming no task
//! is refused the same way so a caller learns nothing about tasks it never
//! started. The model's `task_cancel` and `task_status` built-ins reuse
//! the cancel and status readers here, narrowed further to the caller's
//! model-origin tasks. `status` and `note` add the self exception: a chain may read
//! and annotate the task it runs inside (`sys.taskid`), which is how a
//! task reports progress. The main walk is task `0` with no slot, so its
//! own status is not reportable; `note` from the main walk records on the
//! walk itself, where nothing reads it.
//!
//! Cancel is idempotent: a live task's slot moves to `Cancelled`, its
//! backing chain aborts with everything it owns, and `TaskCancelled` fires
//! once under its target; a task already in a terminal state is left as it
//! is and reports nothing.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use promptforge_api_types::ids::{TaskId, TaskOrigin};

use crate::execute::protocol::{Answer, TaskDelivery, TaskStatus};
use crate::observe::Observation;
use crate::{Error, Result};

use super::tasks::{TaskBacking, TaskSlot, TaskState};
use super::{ChainIndex, Scheduler};

/// The `state` tag `tasks.status` reports: a delivered task reads `done`,
/// since delivery is the owner's bookkeeping, not a lifecycle change.
fn state_tag(state: TaskState) -> &'static str {
    match state {
        TaskState::Running => "running",
        TaskState::Done | TaskState::Delivered => "done",
        TaskState::Cancelled => "cancelled",
        TaskState::Abandoned => "abandoned",
    }
}

impl Scheduler<'_> {
    /// Resumes `id` at once with `answer`: the inline-answer path every
    /// non-waiting task arm takes.
    fn answer_inline(&mut self, id: ChainIndex, answer: Answer<Error>) {
        self.chains[id.index()].incoming = Some(answer);
        self.ready.push_back(id);
    }

    /// The slot of a task `caller` owns, or [`Error::TaskNotOwned`] - for
    /// an unknown id too.
    fn owned_slot(&self, caller: ChainIndex, task: &TaskId) -> Result<&TaskSlot> {
        match self.tasks.get(task) {
            Some(slot) if slot.owner == caller => Ok(slot),
            _ => Err(Error::TaskNotOwned { task: task.clone() }),
        }
    }

    /// The slot of a task `caller` owns or runs inside (its `sys.taskid`,
    /// which a `call` child of the task chain shares).
    fn visible_slot(&self, caller: ChainIndex, task: &TaskId) -> Result<&TaskSlot> {
        match self.tasks.get(task) {
            Some(slot) if slot.owner == caller || self.chains[caller.index()].task == *task => {
                Ok(slot)
            }
            _ => Err(Error::TaskNotOwned { task: task.clone() }),
        }
    }

    /// The live tasks `owner` owns in spawn order, narrowed to `origin`
    /// when given; an internal timer slot is never listed. The arena is a
    /// hash map; ids order as paths and one owner's tasks are its direct
    /// children, so sorting recovers spawn order.
    fn live_tasks_of(&self, owner: ChainIndex, origin: Option<TaskOrigin>) -> Vec<TaskId> {
        let mut live: Vec<TaskId> = self
            .tasks
            .iter()
            .filter(|(_, slot)| slot.owner == owner && slot.state.is_live() && !slot.is_internal())
            .filter(|(_, slot)| origin.is_none_or(|origin| slot.origin == origin))
            .map(|(task, _)| task.clone())
            .collect();
        live.sort();
        live
    }

    /// Dispatches a `when_any` request: every member must be a task the
    /// chain owns and none may be delivered already; the first terminal
    /// member in set order is delivered at once, otherwise the chain parks
    /// on the set until a member's chain end wakes it.
    pub(super) fn dispatch_when_any(&mut self, id: ChainIndex, tasks: Vec<TaskId>) {
        match self.first_terminal(id, &tasks) {
            Ok(Some(task)) => {
                let delivery = self.deliver(&task);
                self.answer_inline(id, Answer::WhenAny(Ok(delivery)));
            }
            Ok(None) => {
                self.chains[id.index()].waiting_on = tasks;
            }
            Err(error) => self.answer_inline(id, Answer::WhenAny(Err(error))),
        }
    }

    /// Validates a wait set and returns its first terminal member in set
    /// order, or `None` when every member is still running. An `Abandoned`
    /// member cannot pass the ownership check (its owner has ended), so
    /// the terminal arm only ever sees `Done` and `Cancelled`.
    fn first_terminal(&self, id: ChainIndex, tasks: &[TaskId]) -> Result<Option<TaskId>> {
        let mut first = None;
        for task in tasks {
            let slot = self.owned_slot(id, task)?;
            match slot.state {
                TaskState::Delivered => return Err(Error::TaskConsumed { task: task.clone() }),
                TaskState::Running => {}
                TaskState::Done | TaskState::Cancelled | TaskState::Abandoned => {
                    if first.is_none() {
                        first = Some(task.clone());
                    }
                }
            }
        }
        Ok(first)
    }

    /// Takes a terminal slot's outcome as a delivery: a `Done` slot's
    /// outcome moves out and the slot to `Delivered`; a cancelled slot
    /// yields the `cancelled` error value and stays, having no result to
    /// consume. An abandoned slot has no live owner to wait on it, so its
    /// delivery is a scheduler bug.
    fn deliver(&mut self, task: &TaskId) -> TaskDelivery<Error> {
        let outcome = match self.tasks.get_mut(task) {
            Some(slot) => match slot.state {
                TaskState::Done => {
                    slot.state = TaskState::Delivered;
                    slot.outcome
                        .take()
                        .unwrap_or_else(|| Err(Error::internal("a done slot holds its outcome")))
                }
                TaskState::Cancelled => Err(Error::TaskCancelled { task: task.clone() }),
                TaskState::Abandoned => Err(Error::internal(
                    "an abandoned task's owner ended, so no wait can deliver it",
                )),
                TaskState::Running | TaskState::Delivered => Err(Error::internal(
                    "only a terminal, undelivered slot is delivered",
                )),
            },
            None => Err(Error::internal("a delivered task has a slot")),
        };
        TaskDelivery {
            task: task.clone(),
            outcome,
        }
    }

    /// Wakes `task`'s owner if it is parked on a set containing `task`:
    /// the member is delivered as the wait's answer and the owner leaves
    /// its wait.
    pub(super) fn wake_waiter(&mut self, owner: ChainIndex, task: &TaskId) {
        if !self.chains[owner.index()].waiting_on.contains(task) {
            return;
        }
        let delivery = self.deliver(task);
        self.chains[owner.index()].waiting_on.clear();
        self.answer_inline(owner, Answer::WhenAny(Ok(delivery)));
    }

    /// Dispatches a `ready` request: whether a task the chain owns has
    /// ended, delivered or not.
    pub(super) fn dispatch_ready(&mut self, id: ChainIndex, task: &TaskId) {
        let answer = self.owned_slot(id, task).map(|slot| !slot.state.is_live());
        self.answer_inline(id, Answer::Ready(answer));
    }

    /// Dispatches a `status` request over a task the chain owns or runs
    /// inside.
    pub(super) fn dispatch_status(&mut self, id: ChainIndex, task: &TaskId) {
        let answer = self.task_status(id, task).map(Box::new);
        self.answer_inline(id, Answer::Status(answer));
    }

    /// Reads one task's status: the slot's facts, plus the backing chain's
    /// position while it is live (its section and what it is parked on)
    /// and the chain's counters and note, which the append-only arena
    /// keeps after the chain ends.
    pub(super) fn task_status(&self, id: ChainIndex, task: &TaskId) -> Result<TaskStatus> {
        let slot = self.visible_slot(id, task)?;
        let mut status = TaskStatus {
            target: slot.target.clone(),
            origin: slot.origin,
            state: state_tag(slot.state),
            ok: slot.ok,
            section: None,
            blocked: None,
            turns: 0,
            tasks: Vec::new(),
            depth: 0,
            note: None,
        };
        if let TaskBacking::Chain(backing) = slot.backing {
            let chain = &self.chains[backing.index()];
            status.turns = chain.ctx.turns().load(Ordering::Relaxed);
            status.depth = u32::try_from(chain.call_depth).unwrap_or(u32::MAX);
            status.note.clone_from(&chain.note);
            if slot.state.is_live() {
                status.section = chain
                    .frame
                    .is_some()
                    .then(|| chain.section_name().to_owned());
                status.blocked = chain.blocked;
                status.tasks = self.live_tasks_of(backing, None);
            }
        }
        Ok(status)
    }

    /// Dispatches a `pending` request: the chain's live tasks in spawn
    /// order, narrowed to `origin` when given.
    pub(super) fn dispatch_pending(&mut self, id: ChainIndex, origin: Option<TaskOrigin>) {
        let tasks = self.live_tasks_of(id, origin);
        self.answer_inline(id, Answer::Pending(Ok(tasks)));
    }

    /// Dispatches a `note` request: the text becomes the latest note of the
    /// task the chain runs inside - recorded on the task's backing chain,
    /// so a `call` child's note is the task's - or of the chain itself when
    /// it runs inside no slotted task (the main walk).
    pub(super) fn dispatch_note(&mut self, id: ChainIndex, text: String) {
        let task = self.chains[id.index()].task.clone();
        let target = match self.tasks.get(&task).map(|slot| slot.backing) {
            Some(TaskBacking::Chain(backing)) => backing,
            Some(TaskBacking::Effect(_)) | None => id,
        };
        self.chains[target.index()].note = Some(text);
        self.answer_inline(id, Answer::Note(Ok(())));
    }

    /// Dispatches a `cancel` request over a task the chain owns.
    pub(super) fn dispatch_cancel(&mut self, id: ChainIndex, task: &TaskId) {
        let answer = self.cancel_task(id, task);
        self.answer_inline(id, Answer::Cancel(answer));
    }

    /// Cancels a live task `caller` owns: the slot moves to `Cancelled`, the
    /// backing ends (a chain with everything it owns, a request dropped),
    /// and `TaskCancelled` fires once under the target - except for an
    /// internal timer, whose cancel is the wait shim's own bookkeeping and
    /// reports nothing. A task already in a terminal state is left as it
    /// is.
    pub(super) fn cancel_task(&mut self, caller: ChainIndex, task: &TaskId) -> Result<()> {
        let slot = self.owned_slot(caller, task)?;
        if !slot.state.is_live() {
            return Ok(());
        }
        let backing = slot.backing;
        let target = slot.target.clone();
        if let Some(slot) = self.tasks.get_mut(task) {
            slot.state = TaskState::Cancelled;
            slot.ok = Some(false);
        }
        // The backing ends first, so anything it owned reports before the
        // task's own terminal event, which is the last word on it.
        let backing_chain = match backing {
            TaskBacking::Chain(backing_chain) => backing_chain,
            TaskBacking::Effect(request) => {
                self.abort_request(request);
                return Ok(());
            }
        };
        self.abort_subtree(backing_chain);
        let chain = &self.chains[caller.index()];
        let observer = Arc::clone(chain.ctx.observer());
        observer.observe(
            chain.ctx.execution(),
            &target,
            Observation::TaskCancelled { task: task.clone() },
        );
        Ok(())
    }
}
