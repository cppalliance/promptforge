//! The `timer` arm: the internal timeout behind a timed wait (an author's
//! `opts.timeout`, or the model's `await_tasks { timeout }`), as an
//! effect-backed task slot.
//!
//! A timer is the one task whose work is not a chain: its backing is an
//! in-flight leaf request that sleeps and posts back, and its slot lives
//! in the task arena beside the chain-backed ones so the wait machinery
//! needs no second primitive - the shim lists the timer's id in its
//! `when_any` set, and the timer's firing completes its slot and wakes the
//! waiter exactly as a task chain's end does. Cancel is the ordinary
//! cancel arm: the slot moves to `Cancelled` and the sleep is dropped
//! through the shared in-flight abort path.
//!
//! The timer is never author-visible. The shim keeps its id, `pending`
//! and a status table's `tasks` list omit effect-backed slots, and no task
//! observation fires for one: it is the wait's implementation detail, not
//! a task the author started. Its id still consumes the owner's next child
//! index so that every id the owner hands out stays a function of the
//! owner's own dispatch order.
//!
//! Until the effect loop lands the sleep is served by today's spawned leaf
//! path: a `tokio::time::sleep` on the answer channel, keyed in the pending
//! table under the owner so the stall check and the abort paths see it as
//! the in-flight request it is.

use std::time::Duration;

use promptforge_api_types::ids::{TaskId, TaskOrigin};

use crate::execute::protocol::Answer;
use crate::{Error, Result};

use super::tasks::{TaskBacking, TaskSlot, TaskState};
use super::{Arrival, ChainIndex, RequestId, Scheduler};

impl Scheduler<'_> {
    /// Dispatches a `timer` request: allocates the timer's id under the
    /// caller, registers its effect-backed slot, spawns the sleep onto the
    /// answer channel, and resumes the caller at once with the id. A
    /// dispatch failure is the call's answer, resumed into the caller so
    /// the wait shim raises it before any wait.
    pub(super) fn dispatch_timer(&mut self, id: ChainIndex, seconds: f64) {
        let answer = Answer::Timer(self.prepare_timer(id, seconds));
        self.chains[id.index()].incoming = Some(answer);
        self.ready.push_back(id);
    }

    /// The fallible half of timer dispatch, shared with the model's
    /// `await_tasks`: the duration check (the parse already bounds it, so
    /// a failure here is defensive), the id allocation, the slot, and the
    /// spawned sleep.
    pub(super) fn prepare_timer(&mut self, id: ChainIndex, seconds: f64) -> Result<TaskId> {
        let duration = Duration::try_from_secs_f64(seconds).map_err(|_| {
            Error::Lua(format!(
                "timeout must be a non-negative finite number of seconds, got {seconds}"
            ))
        })?;
        let task = TaskId::from(self.allocate_child_id(id)?);
        let request_id = RequestId(self.next_request);
        self.next_request += 1;
        let tx = self.answer_tx.clone();
        let handle = tokio::spawn(async move {
            tokio::time::sleep(duration).await;
            // A send fails only when the driver is gone (a cancelled run);
            // the firing is then moot.
            let _ = tx.send((request_id, Arrival::Timer));
        });
        self.io_tasks.insert(request_id, handle);
        self.pending.insert(request_id, id);
        self.tasks.insert(
            task.clone(),
            TaskSlot {
                backing: TaskBacking::Effect(request_id),
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

    /// Applies a timer's firing: the slot backed by `request` moves to
    /// `Done` with an empty outcome and its owner is woken if it is parked
    /// on a set containing the timer. Otherwise the slot holds until the
    /// owner's next wait delivers it - the shim's `when_all` rounds may
    /// be between waits when the timer fires.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when no live slot is backed by
    /// `request`, which only a scheduler bug produces: a cancelled or
    /// abandoned timer's request is aborted and its late firing discarded
    /// before it reaches here.
    pub(super) fn fire_timer(&mut self, request: RequestId) -> Result<()> {
        let Some((task, owner)) = self
            .tasks
            .iter()
            .find(|(_, slot)| slot.backing == TaskBacking::Effect(request) && slot.state.is_live())
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
