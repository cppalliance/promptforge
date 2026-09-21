//! The `timer` arm: the internal timeout behind a timed wait (an author's
//! `opts.timeout`, or the model's `await_tasks { timeout }`), as an
//! effect-backed task slot.
//!
//! A timer is the one task backed by an in-flight leaf request rather than
//! a chain: the request sleeps and posts back, and its slot sits in the
//! task arena beside the chain-backed ones so one wait primitive serves
//! both - the shim lists the timer's id in its `when_any` set, and the
//! timer's firing completes its slot and wakes the waiter exactly as a
//! task chain's end does. Cancel is the ordinary cancel arm: the slot
//! moves to `Cancelled` and the sleep is dropped through the shared
//! in-flight abort path.
//!
//! The timer is never author-visible. The shim keeps its id, `pending`
//! and a status table's `tasks` list omit effect-backed slots, and no task
//! observation fires for one: it is the wait's implementation detail, not
//! a task the author started. Its id still consumes the owner's next child
//! index so that every id the owner hands out stays a function of the
//! owner's own dispatch order.
//!
//! The sleep is a `Timer` effect issued under the owner: keyed in the
//! pending table under the owner so the stall check and the abort paths
//! see it as the in-flight effect it is, and performed by the host (a
//! tokio host sleeps on its timer wheel).

use std::time::Duration;

use promptforge_api_types::ids::{TaskId, TaskOrigin};

use crate::execute::protocol::Answer;
use crate::execute::run::{Effect, EffectId};
use crate::{Error, Result};

use super::tasks::{TaskBacking, TaskSlot, TaskState};
use super::{ChainIndex, Continuation, Scheduler};

impl Scheduler {
    /// Dispatches a `timer` request: allocates the timer's id under the
    /// caller, registers its effect-backed slot, issues the sleep as an
    /// effect, and resumes the caller at once with the id. A dispatch
    /// failure is the call's answer, resumed into the caller so the wait
    /// shim raises it before any wait.
    pub(super) fn dispatch_timer(&mut self, id: ChainIndex, seconds: f64) {
        let answer = Answer::Timer(self.prepare_timer(id, seconds));
        self.chains[id.index()].incoming = Some(answer);
        self.ready.push_back(id);
    }

    /// The fallible half of timer dispatch, shared with the model's
    /// `await_tasks`: the duration check (the parse already bounds it, so
    /// a failure here is defensive), the id allocation, the issued effect,
    /// and the slot.
    pub(super) fn prepare_timer(&mut self, id: ChainIndex, seconds: f64) -> Result<TaskId> {
        Duration::try_from_secs_f64(seconds).map_err(|_| {
            Error::Lua(format!(
                "timeout must be a non-negative finite number of seconds, got {seconds}"
            ))
        })?;
        let task = TaskId::from(self.allocate_child_id(id)?);
        let effect = self.issue(id, Effect::Timer { seconds }, Continuation::Timer);
        self.tasks.insert(
            task.clone(),
            TaskSlot {
                backing: TaskBacking::Effect(effect),
                owner: id,
                // The timer serves an author wait; the origin is reported
                // nowhere, since the slot is internal.
                origin: TaskOrigin::Author,
                target: "timer".to_owned(),
                state: TaskState::Running,
                ok: None,
                outcome: None,
            },
        );
        Ok(task)
    }

    /// Applies a timer's firing: the slot backed by `effect` moves to
    /// `Done` with an empty outcome and its owner is woken if it is parked
    /// on a set containing the timer. Otherwise the slot holds until the
    /// owner's next wait delivers it - the shim's `when_all` rounds may
    /// be between waits when the timer fires.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when no live slot is backed by
    /// `effect`, which only a scheduler bug produces: a cancelled or
    /// abandoned timer's effect is aborted and its late firing discarded
    /// before it reaches here.
    pub(super) fn fire_timer(&mut self, effect: EffectId) -> Result<()> {
        let Some((task, owner)) = self
            .tasks
            .iter()
            .find(|(_, slot)| slot.backing == TaskBacking::Effect(effect) && slot.state.is_live())
            .map(|(task, slot)| (task.clone(), slot.owner))
        else {
            return Err(Error::internal("a fired timer has a live slot"));
        };
        if let Some(slot) = self.tasks.get_mut(&task) {
            slot.state = TaskState::Done;
            slot.ok = Some(true);
            slot.outcome = Some(Ok(String::new()));
        }
        self.wake_waiter(owner, &task);
        Ok(())
    }
}
