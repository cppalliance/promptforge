//! The task history read: the author's `tasks.events(task, opts?)` and the
//! model's `task_events { id, last? }` built-in, one arm behind both.
//!
//! Every event the engine reports leaves through `step` and is the host's
//! to keep, so a read of a task's events is a leaf effect like any other:
//! the arm checks who may read what, issues a `TaskEvents` effect naming
//! the task and the reader's high-water mark, and the host answers from
//! its log with the events after that mark, in the task's sequence order.
//! The host commits a step's events before it performs the step's effects,
//! so a task reading its own history sees everything reported before the
//! read was issued.
//!
//! Who may read: the author's shim follows the `status` rule - a task the
//! caller owns, or the task the caller runs inside (`sys.taskid`), which
//! is how a task reads its own record; the main walk is task `0` and may
//! read itself the same way. The model's built-in follows the model rule -
//! only a model-origin task the caller owns, so the model never reads the
//! author's work through its tool surface.
//!
//! How the answer resumes: the shim receives the events as a sequence of
//! plain tables in each event's serialized shape. The model receives one
//! JSON event per line, nonce-wrapped as untrusted under the reader's run
//! nonce, because a task's history includes model, tool, and user text -
//! the one built-in answer that is not the engine's own words. A task
//! that has reported nothing new answers the model with a trusted sentence
//! saying so, since there is nothing to wrap.

use promptforge_api_types::event::Event;
use promptforge_api_types::ids::TaskId;
use serde_json::Value;

use crate::Error;
use crate::execute::protocol::Answer;
use crate::execute::run::Effect;

use super::builtins::{BuiltinAnswer, BuiltinOutcome};
use super::{ChainIndex, Continuation, Scheduler};

/// Who issued a history read, and so how its answer resumes the chain.
#[derive(Debug)]
pub(super) enum TaskEventsReader {
    /// The author's `tasks.events` shim: the events resume as a sequence.
    Shim,
    /// The model's `task_events` built-in: the events resume as untrusted
    /// text under the model's call id.
    Builtin {
        /// The model's call id, for the `ToolResult` the answer reports
        /// under.
        call_id: String,
    },
}

impl TaskEventsReader {
    /// The cancelled answer for a dropped read, in the shape the reader's
    /// shim expects.
    pub(super) fn dropped(&self) -> Answer<Error> {
        match self {
            TaskEventsReader::Shim => Answer::TaskEvents(Err(Error::Interrupted)),
            TaskEventsReader::Builtin { .. } => Answer::ToolCallResult(Err(Error::Interrupted)),
        }
    }
}

/// The refusal for a `last` that is not a non-negative integer `u32` holds.
const LAST_REFUSAL: &str =
    "task_events: `last` must be a non-negative integer sequence number when given";

/// Reads the optional `last` argument: absent or null is `None`; a
/// non-negative integer in range is `Some`; anything else is the refusal.
fn last_argument(args: &Value) -> std::result::Result<Option<u32>, String> {
    match args.get("last") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(last)) => last
            .as_u64()
            .and_then(|last| u32::try_from(last).ok())
            .map(Some)
            .ok_or_else(|| LAST_REFUSAL.to_owned()),
        Some(_) => Err(LAST_REFUSAL.to_owned()),
    }
}

/// Renders a history for the model: one JSON event per line, in order.
/// Every event serializes (its fields are strings, numbers, ids, and JSON
/// values), so the fallback line is unreachable in practice and stands
/// only so the rendering stays total.
fn render_events(events: &[Event]) -> String {
    events
        .iter()
        .map(|event| {
            serde_json::to_string(event)
                .unwrap_or_else(|_| "{\"kind\":\"unrenderable\"}".to_owned())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

impl Scheduler {
    /// Whether `caller` may read `task`'s history under the author rule: a
    /// task it owns (an internal timer slot is no task the author sees),
    /// or the task it runs inside. An id naming no task is refused the
    /// same way as one the caller does not own, so a caller learns nothing
    /// about tasks it never started.
    fn readable_task(&self, caller: ChainIndex, task: &TaskId) -> crate::Result<()> {
        if self.chains[caller.index()].task == *task {
            return Ok(());
        }
        match self.tasks.get(task) {
            Some(slot) if slot.owner == caller && !slot.is_internal() => Ok(()),
            _ => Err(Error::TaskNotOwned { task: task.clone() }),
        }
    }

    /// Dispatches the author's `task_events` request: a task the chain may
    /// read is issued as a `TaskEvents` effect and the chain parks on it;
    /// a refusal is the call's answer, resumed into the caller so an author
    /// `pcall` catches it.
    pub(super) fn dispatch_task_events(
        &mut self,
        id: ChainIndex,
        task: &TaskId,
        last: Option<u32>,
    ) {
        if let Err(error) = self.readable_task(id, task) {
            self.chains[id.index()].incoming = Some(Answer::TaskEvents(Err(error)));
            self.ready.push_back(id);
            return;
        }
        let effect = Effect::TaskEvents {
            task: task.clone(),
            last,
        };
        self.issue(id, effect, Continuation::TaskEvents(TaskEventsReader::Shim));
    }

    /// The model's `task_events` built-in: over a model task the caller
    /// owns, issues the read as a `TaskEvents` effect under the model's
    /// call id and parks the chain; every argument fault is the answer's
    /// text.
    pub(super) fn builtin_task_events(
        &mut self,
        id: ChainIndex,
        args: &Value,
        call_id: &str,
    ) -> BuiltinOutcome {
        let task = match self.model_task(id, "task_events", args) {
            Ok(task) => task,
            Err(text) => return BuiltinOutcome::Answered(BuiltinAnswer::refused(text)),
        };
        let last = match last_argument(args) {
            Ok(last) => last,
            Err(text) => return BuiltinOutcome::Answered(BuiltinAnswer::refused(text)),
        };
        let effect = Effect::TaskEvents { task, last };
        let reader = TaskEventsReader::Builtin {
            call_id: call_id.to_owned(),
        };
        self.issue(id, effect, Continuation::TaskEvents(reader));
        self.chains[id.index()].blocked = Some("tasks");
        BuiltinOutcome::Issued
    }

    /// Applies a history read's answer: the shim's sequence, or the
    /// model's text - the events nonce-wrapped as untrusted under the
    /// reader's run nonce, reported under the model's call id, or the
    /// trusted nothing-new sentence when the host returned no event.
    pub(super) fn accept_task_events(
        &self,
        chain: ChainIndex,
        reader: &TaskEventsReader,
        events: Vec<Event>,
    ) -> Answer<Error> {
        match reader {
            TaskEventsReader::Shim => Answer::TaskEvents(Ok(events)),
            TaskEventsReader::Builtin { call_id } => {
                let answer = if events.is_empty() {
                    BuiltinAnswer::served("no new events".to_owned())
                } else {
                    let nonce = self.chains[chain.index()].ctx.nonce();
                    BuiltinAnswer::served_untrusted(nonce.wrap(&render_events(&events)))
                };
                self.report_builtin_answer(chain, "task_events", call_id, answer)
            }
        }
    }
}
