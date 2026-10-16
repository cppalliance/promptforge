//! The model's `await_tasks { timeout? }` built-in: its one wait.
//!
//! In place of a `join_any`, the model gets a tool call that parks its
//! section until one of the tasks it started ends. The arm reuses the
//! author wait's machinery - the chain's `waiting_on` set and the wake a
//! member's end or a timer's firing performs - and diverts the wake: the
//! member is not delivered to a shim, since a model task's outcome
//! travels as a notice (queued on the owner before the wake), so the wake
//! drains the owner's notice queue and returns the texts as the tool
//! call's answer. A timeout is the same effect-backed timer an author's
//! `opts.timeout` starts, listed after the members so a finished member
//! wins over a fired timer, and cancelled (an internal cancel, reported
//! nowhere) when a member wins.
//!
//! The call parks only on an empty notice queue. A task that ended during
//! the chat round that issued the call (after its dispatch's drain, before
//! the answer arrived) has already queued its notice, and that notice is
//! the answer at once: the model asked for results that arrived, and one
//! has. Parking on it would hold the model for a second task's end or the
//! full timeout while its result sat unread.
//!
//! The answer shapes: the drained notices, one per line, when a task
//! ended; the notices then `timed out; tasks 3, 5 still running` when the
//! timer fired first; `nothing to wait for` when the model has no live
//! task, no timeout, and no notice pending; a plain sleep ending in
//! `slept N seconds` when only a timeout was given. Every shape is the
//! Engine's own text, so it resumes trusted.

use std::time::Duration;

use runvil_types::ids::{TaskId, TaskOrigin};
use serde_json::Value;

use crate::execute::protocol::Blocked;
use crate::{Error, Result};

use super::builtins::{BuiltinAnswer, BuiltinOutcome, TaskBuiltin};
use super::{ChainIndex, Scheduler};

/// The model's parked `await_tasks`, recorded on its chain until a member
/// of `waiting_on` ends or the timer fires.
#[derive(Debug)]
pub(super) struct AwaitTasks {
    /// The model's call id, for the `ToolResult` the wake reports under.
    call_id: String,
    /// The turn the call was dispatched under, which the wake's
    /// `ToolResult` reports.
    turn: u32,
    /// The timeout, when the call gave one.
    timeout: Option<AwaitTimeout>,
}

/// A parked `await_tasks` call's timeout.
#[derive(Debug)]
struct AwaitTimeout {
    /// The timer's slot id, listed last in the wait set.
    timer: TaskId,
    /// How long the call waits, for the plain-sleep rendering.
    duration: Duration,
}

/// The refusal for a `timeout` that is not a non-negative finite number
/// of seconds `Duration` can hold.
const TIMEOUT_REFUSAL: &str =
    "await_tasks: `timeout` must be a non-negative number of seconds when given";

/// Reads the optional `timeout` argument: absent or null is `None`; a
/// number of seconds `Duration` can hold is that duration; anything else
/// is the refusal.
fn timeout_argument(args: &Value) -> std::result::Result<Option<Duration>, String> {
    match args.get("timeout") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(seconds)) => seconds
            .as_f64()
            .and_then(|seconds| Duration::try_from_secs_f64(seconds).ok())
            .map(Some)
            .ok_or_else(|| TIMEOUT_REFUSAL.to_owned()),
        Some(_) => Err(TIMEOUT_REFUSAL.to_owned()),
    }
}

/// Renders a wake's answer: the drained notices, then the timeout line
/// when the timer fired - the still-running list, or the plain sleep's
/// duration when nothing was running.
fn render_wake(
    mut lines: Vec<String>,
    timed_out: Option<Duration>,
    still_running: &[TaskId],
) -> String {
    if let Some(timeout) = timed_out {
        if still_running.is_empty() {
            lines.push(format!("slept {} seconds", timeout.as_secs_f64()));
        } else {
            let ids: Vec<String> = still_running.iter().map(ToString::to_string).collect();
            lines.push(format!("timed out; tasks {} still running", ids.join(", ")));
        }
    }
    lines.join("\n")
}

impl Scheduler {
    /// The `await_tasks` built-in: answers at once with the pending
    /// notices when any are queued (a task that ended during the round
    /// that issued the call is a result that has already arrived, so no
    /// wait is owed), or with `nothing to wait for` when the queue is
    /// empty and the model has nothing live and no timeout; otherwise
    /// parks the chain on its live model tasks plus the timeout's timer,
    /// to be answered by [`Self::finish_await_tasks`]. Every fault, the
    /// timer's included, is the answer's text.
    pub(super) fn builtin_await_tasks(
        &mut self,
        id: ChainIndex,
        args: &Value,
        call_id: &str,
        turn: u32,
    ) -> BuiltinOutcome {
        let timeout = match timeout_argument(args) {
            Ok(timeout) => timeout,
            Err(text) => return BuiltinOutcome::Answered(BuiltinAnswer::refused(text)),
        };
        let notices = self.drain_task_notices(id);
        if !notices.is_empty() {
            return BuiltinOutcome::Answered(BuiltinAnswer::served(notices.join("\n")));
        }
        let live = self.live_tasks_of(id, Some(TaskOrigin::Model));
        if live.is_empty() && timeout.is_none() {
            return BuiltinOutcome::Answered(BuiltinAnswer::served(
                "nothing to wait for".to_owned(),
            ));
        }
        let mut set = live;
        let timeout = match timeout {
            Some(duration) => match self.prepare_timer(id, duration) {
                Ok(timer) => {
                    set.push(timer.clone());
                    Some(AwaitTimeout { timer, duration })
                }
                Err(error) => {
                    return BuiltinOutcome::Answered(BuiltinAnswer::refused(format!(
                        "await_tasks: {error}"
                    )));
                }
            },
            None => None,
        };
        let chain = &mut self.chains[id.index()];
        chain.waiting_on = set;
        chain.awaiting = Some(AwaitTasks {
            call_id: call_id.to_owned(),
            turn,
            timeout,
        });
        chain.blocked = Some(Blocked::Tasks);
        // Parking over live model tasks gives the chain's admission slots
        // back, exactly as an author's join does: the tasks it waits for
        // need them. A timeout-only wait keeps its slots.
        self.park_wait(id);
        BuiltinOutcome::Parked
    }

    /// Answers a parked `await_tasks` on `owner` woken by `woke` (a member
    /// that ended, or the timer): an unfired timer is cancelled, the
    /// owner's notices are drained, and the rendered text resumes the
    /// model's tool call. The caller has already cleared `waiting_on`
    /// and taken `awaiting`.
    ///
    /// # Errors
    /// Returns [`Error::Internal`] when the owner no longer owns its own
    /// timer, which only a scheduler bug produces.
    pub(super) fn finish_await_tasks(
        &mut self,
        owner: ChainIndex,
        awaiting: &AwaitTasks,
        woke: &TaskId,
    ) -> Result<()> {
        let fired = match &awaiting.timeout {
            Some(timeout) if timeout.timer == *woke => Some(timeout.duration),
            Some(timeout) => {
                // An internal slot: the cancel reports nothing.
                self.cancel_task(owner, &timeout.timer).map_err(|error| {
                    Error::internal_with_cause("an await_tasks owner owns its timer", error)
                })?;
                None
            }
            None => None,
        };
        let notices = self.drain_task_notices(owner);
        let still_running = self.live_tasks_of(owner, Some(TaskOrigin::Model));
        let text = render_wake(notices, fired, &still_running);
        let answer = self.report_builtin_answer(
            owner,
            TaskBuiltin::AwaitTasks,
            &awaiting.call_id,
            awaiting.turn,
            BuiltinAnswer::served(text),
        );
        self.wake_from_wait(owner, answer);
        Ok(())
    }
}
