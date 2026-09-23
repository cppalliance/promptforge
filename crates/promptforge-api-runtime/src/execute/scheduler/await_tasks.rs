//! The model's `await_tasks { timeout? }` built-in: its one wait.
//!
//! In place of a `when_any`, the model gets a tool call that parks its
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
//! the chat round that issued the call (after the shim's drain, before
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
//! engine's own text, so it resumes trusted.

use promptforge_api_types::ids::{TaskId, TaskOrigin};
use serde_json::Value;

use super::builtins::{BuiltinAnswer, BuiltinOutcome};
use super::{ChainIndex, Scheduler};

/// The model's parked `await_tasks`, recorded on its chain until a member
/// of `waiting_on` ends or the timer fires.
#[derive(Debug)]
pub(super) struct AwaitTasks {
    /// The model's call id, for the `ToolResult` the wake reports under.
    call_id: String,
    /// The timeout timer's slot id, when the call gave a timeout; listed
    /// last in the wait set.
    timer: Option<TaskId>,
    /// The timeout in seconds, for the plain-sleep rendering.
    seconds: Option<f64>,
}

/// The refusal for a `timeout` that is not a non-negative finite number
/// of seconds `Duration` can hold.
const TIMEOUT_REFUSAL: &str =
    "await_tasks: `timeout` must be a non-negative number of seconds when given";

/// Reads the optional `timeout` argument: absent or null is `None`; a
/// number `Duration` can hold is `Some`; anything else is the refusal.
fn timeout_argument(args: &Value) -> std::result::Result<Option<f64>, String> {
    match args.get("timeout") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(seconds)) => seconds
            .as_f64()
            .filter(|seconds| std::time::Duration::try_from_secs_f64(*seconds).is_ok())
            .map(Some)
            .ok_or_else(|| TIMEOUT_REFUSAL.to_owned()),
        Some(_) => Err(TIMEOUT_REFUSAL.to_owned()),
    }
}

/// Renders a wake's answer: the drained notices, then the timeout line
/// when the timer fired - the still-running list, or the plain sleep's
/// duration when nothing was running.
fn render_wake(mut lines: Vec<String>, timed_out: Option<f64>, still_running: &[TaskId]) -> String {
    if let Some(seconds) = timed_out {
        if still_running.is_empty() {
            lines.push(format!("slept {seconds} seconds"));
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
    ) -> BuiltinOutcome {
        let seconds = match timeout_argument(args) {
            Ok(seconds) => seconds,
            Err(text) => return BuiltinOutcome::Answered(BuiltinAnswer::refused(text)),
        };
        let notices = self.drain_task_notices(id);
        if !notices.is_empty() {
            return BuiltinOutcome::Answered(BuiltinAnswer::served(notices.join("\n")));
        }
        let live = self.live_tasks_of(id, Some(TaskOrigin::Model));
        if live.is_empty() && seconds.is_none() {
            return BuiltinOutcome::Answered(BuiltinAnswer::served(
                "nothing to wait for".to_owned(),
            ));
        }
        let mut set = live;
        let timer = match seconds {
            Some(seconds) => match self.prepare_timer(id, seconds) {
                Ok(timer) => {
                    set.push(timer.clone());
                    Some(timer)
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
            timer,
            seconds,
        });
        chain.blocked = Some("tasks");
        BuiltinOutcome::Parked
    }

    /// Answers a parked `await_tasks` on `owner` woken by `woke` (a member
    /// that ended, or the timer): an unfired timer is cancelled, the
    /// owner's notices are drained, and the rendered text resumes the
    /// model's tool call. The caller has already cleared `waiting_on`
    /// and taken `awaiting`.
    pub(super) fn finish_await_tasks(
        &mut self,
        owner: ChainIndex,
        awaiting: &AwaitTasks,
        woke: &TaskId,
    ) {
        let timed_out = awaiting.timer.as_ref() == Some(woke);
        if !timed_out && let Some(timer) = &awaiting.timer {
            // An internal slot: the cancel reports nothing, and a fault
            // here (the owner no longer owning its own timer) cannot
            // happen outside a scheduler bug, so the result is not
            // inspected.
            let _ = self.cancel_task(owner, timer);
        }
        let notices = self.drain_task_notices(owner);
        let still_running = self.live_tasks_of(owner, Some(TaskOrigin::Model));
        let fired = if timed_out { awaiting.seconds } else { None };
        let text = render_wake(notices, fired, &still_running);
        let answer = self.report_builtin_answer(
            owner,
            "await_tasks",
            &awaiting.call_id,
            BuiltinAnswer::served(text),
        );
        self.answer_inline(owner, answer);
    }
}
