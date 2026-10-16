//! The model's task built-ins: `task`, `task_cancel`, `task_status`, and
//! `await_tasks` (whose arm sits in the `await_tasks` module), answered
//! by the scheduler over its task arena.
//!
//! An author opts a section in with `tools.allow_tasks(targets?)`, which
//! records an allowlist on the section's tool runtime. While it is set,
//! every `chat` round the section yields advertises the four built-ins
//! beside its bound and local tools ([`advertise_task_builtins`]), and the
//! `tool_call` arm answers a model-issued call to one of them here, before
//! alias lookup, so no bound or local tool can shadow them. Every answer is
//! content the model reads: a started task's id, a cancel's confirmation,
//! a status line, a wait's drained notices, or a refusal naming what was
//! wrong - the Engine's own text, so it resumes trusted and its
//! `ToolResult` fires under the model's call id. A refusal is observed as
//! a failed tool call, a served answer as a succeeded one.
//!
//! The model sees only its own tasks: a `task_cancel` or `task_status`
//! naming a task the author started (or one the caller does not own) is
//! refused as unknown, so the model can neither end nor inspect the
//! author's work through its tool surface. The author, by contrast, may
//! adopt the model's tasks through `tasks.pending({ origin = "model" })`.
//!
//! The built-ins' fixed schemas and the function that advertises them
//! sit in the `schemas` sibling; this file holds the arms.

#[path = "builtins-schemas.rs"]
mod schemas;

use std::fmt::Write as _;

use promptforge_types::event::lifecycle;
use promptforge_types::ids::{TaskId, TaskOrigin};
use promptforge_types::tools::OutputTrust;
use serde_json::Value;

use crate::execute::protocol::{Answer, SpawnVia, TaskPhase, TaskStatus, ToolCallOutcome};
use crate::execute::section_context::TaskSeed;
use crate::lua::{SectionVm, TaskAllowlist};
use crate::{Error, Result};

use super::tool_call::ToolCallDispatch;
use super::{ChainIndex, Scheduler};

pub(super) use schemas::advertise_task_builtins;

/// One of the model's task built-ins. Their names are reserved: no
/// catalog or local tool can take one, and a script call to one is
/// unbound.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::execute) enum TaskBuiltin {
    /// `task`: start a task.
    Task,
    /// `task_cancel`: cancel a model task.
    TaskCancel,
    /// `task_status`: report a model task.
    TaskStatus,
    /// `await_tasks`: wait for a model task to end.
    AwaitTasks,
}

impl TaskBuiltin {
    /// Every built-in, in the order the model sees them advertised.
    pub(in crate::execute) const ALL: [TaskBuiltin; 4] = [
        TaskBuiltin::Task,
        TaskBuiltin::TaskCancel,
        TaskBuiltin::TaskStatus,
        TaskBuiltin::AwaitTasks,
    ];

    /// The built-in's tool name.
    pub(in crate::execute) fn name(self) -> &'static str {
        match self {
            TaskBuiltin::Task => "task",
            TaskBuiltin::TaskCancel => "task_cancel",
            TaskBuiltin::TaskStatus => "task_status",
            TaskBuiltin::AwaitTasks => "await_tasks",
        }
    }

    /// The built-in a tool name reserves, if it reserves one.
    pub(in crate::execute) fn from_name(name: &str) -> Option<TaskBuiltin> {
        TaskBuiltin::ALL
            .into_iter()
            .find(|builtin| builtin.name() == name)
    }
}

/// The section's task allowlist, read off its VM's tool runtime: `None`
/// until `tools.allow_tasks` has run in the section.
///
/// # Errors
/// Returns [`Error::Internal`] when the runtime's mutex is poisoned.
pub(super) fn task_allowlist(vm: &SectionVm) -> Result<Option<TaskAllowlist>> {
    let runtime = vm
        .tool_runtime()
        .lock()
        .map_err(|_| Error::internal("tool declaration runtime was poisoned"))?;
    Ok(runtime.allowed_tasks.clone())
}

/// One built-in's answer: the text the model reads, whether it served the
/// call or refused it, and the task chain a `task` started behind the
/// caller, which the dispatcher enqueues after the caller so the caller
/// runs first as it does after `tasks.spawn`. The text is the Engine's
/// own, so it always resumes trusted.
#[derive(Debug)]
pub(super) struct BuiltinAnswer {
    text: String,
    ok: bool,
    started: Option<ChainIndex>,
}

impl BuiltinAnswer {
    pub(super) fn served(text: String) -> Self {
        Self {
            text,
            ok: true,
            started: None,
        }
    }

    pub(super) fn refused(text: String) -> Self {
        Self {
            text,
            ok: false,
            started: None,
        }
    }
}

/// How one built-in call resolved: an answer for the caller now, or the
/// caller parked (`await_tasks` on live tasks), answered when it wakes.
#[derive(Debug)]
pub(super) enum BuiltinOutcome {
    Answered(BuiltinAnswer),
    Parked,
}

/// Reads the `id` argument of `task_cancel` or `task_status`, or the
/// refusal text for a missing or malformed one.
fn task_id_argument(builtin: TaskBuiltin, args: &Value) -> std::result::Result<TaskId, String> {
    let name = builtin.name();
    let Some(id) = args.get("id").and_then(Value::as_str) else {
        return Err(format!(
            "{name}: `id` must be a task id string, exactly as `task` returned it"
        ));
    };
    id.parse().map_err(|_| {
        format!(
            "{name}: `{id}` is not a task id: required a dot-separated path of decimal \
             components; use the id `task` returned"
        )
    })
}

/// Renders one status for the model: the id, the target heading, the
/// state (with the outcome for a finished task), then whatever the live
/// chain reports - where it is, what it waits on, its turns, its own live
/// tasks, and its latest note.
fn render_status(task: &TaskId, status: &TaskStatus) -> String {
    let mut text = format!(
        "Task id={task} (## {}): {}",
        status.target,
        status.state.tag()
    );
    if status.state == TaskPhase::Done {
        text.push_str(if status.ok == Some(true) {
            ", ok"
        } else {
            ", failed"
        });
    }
    if let Some(section) = &status.section {
        let _ = write!(text, ", in ## {section}");
    }
    if let Some(blocked) = status.blocked {
        let _ = write!(text, ", waiting on {}", blocked.tag());
    }
    let _ = write!(text, ", turns {}", status.turns);
    if !status.tasks.is_empty() {
        let tasks: Vec<String> = status.tasks.iter().map(ToString::to_string).collect();
        let _ = write!(text, ", tasks {}", tasks.join(", "));
    }
    if let Some(note) = &status.note {
        let _ = write!(text, ", note: {note}");
    }
    text
}

impl Scheduler {
    /// Answers a model-issued call to one of the task built-ins on the
    /// driver thread: the arm's answer, its succeeded/failed observation,
    /// and the trusted `ToolResult` report under the model's call id - or
    /// the chain parked, for an `await_tasks` whose answer comes when a
    /// task ends. Only the caller's own bookkeeping can fail here (a lost
    /// frame, a poisoned runtime); every model-facing fault is the
    /// answer's text. `turn` is the turn the call was dispatched under,
    /// which every report of its answer carries, including a parked
    /// answer's later one.
    ///
    /// # Errors
    /// Returns the internal fault the arm met.
    pub(super) fn answer_task_builtin(
        &mut self,
        id: ChainIndex,
        builtin: TaskBuiltin,
        args: &Value,
        call_id: &str,
        turn: u32,
    ) -> Result<ToolCallDispatch> {
        let outcome = match builtin {
            TaskBuiltin::Task => BuiltinOutcome::Answered(self.builtin_task(id, args)?),
            TaskBuiltin::TaskCancel => BuiltinOutcome::Answered(self.builtin_task_cancel(id, args)),
            TaskBuiltin::TaskStatus => BuiltinOutcome::Answered(self.builtin_task_status(id, args)),
            TaskBuiltin::AwaitTasks => self.builtin_await_tasks(id, args, call_id, turn),
        };
        let answer = match outcome {
            BuiltinOutcome::Answered(answer) => answer,
            BuiltinOutcome::Parked => return Ok(ToolCallDispatch::Parked),
        };
        let started = answer.started;
        let answer = self.report_builtin_answer(id, builtin, call_id, turn, answer);
        Ok(match started {
            Some(child) => ToolCallDispatch::Started(answer, child),
            None => ToolCallDispatch::Answered(answer),
        })
    }

    /// Reports one built-in's answer - the succeeded/failed observation and
    /// the `ToolResult` under the model's call id and the `turn` the call
    /// was dispatched under, trusted - and renders it as the tool call's
    /// answer. Shared by the immediate answers and the `await_tasks` wake.
    pub(super) fn report_builtin_answer(
        &self,
        id: ChainIndex,
        builtin: TaskBuiltin,
        call_id: &str,
        turn: u32,
        answer: BuiltinAnswer,
    ) -> Answer<Error> {
        let chain = &self.chains[id.index()];
        let emitter = chain.ctx.emitter();
        let section = chain.section_name();
        emitter.report(
            section,
            if answer.ok {
                lifecycle::TOOL_CALL_SUCCEEDED
            } else {
                lifecycle::TOOL_CALL_FAILED
            },
        );
        emitter.tool_result(
            section,
            turn,
            call_id,
            builtin.name(),
            None,
            &answer.text,
            OutputTrust::Trusted,
        );
        Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(answer.text)))
    }

    /// The `task` built-in: checks the arguments and the section's
    /// allowlist, then starts the task's chain through the same spawn path
    /// `tasks.spawn` takes, with the model as origin and the caller's
    /// current `var` as the seed. A spawn refusal (the depth cap, an
    /// unresolvable target, a list section) is the answer's text.
    fn builtin_task(&mut self, id: ChainIndex, args: &Value) -> Result<BuiltinAnswer> {
        let Some(target) = args.get("target").and_then(Value::as_str) else {
            return Ok(BuiltinAnswer::refused(
                "task: `target` must be a string naming a section heading, such as `## Research`"
                    .to_owned(),
            ));
        };
        let input = match args.get("input") {
            None | Some(Value::Null) => None,
            Some(Value::String(input)) => Some(input.as_str()),
            Some(_) => {
                return Ok(BuiltinAnswer::refused(
                    "task: `input` must be a string when given".to_owned(),
                ));
            }
        };
        let vm = self.chains[id.index()].frame()?.vm()?;
        match task_allowlist(vm)? {
            None => {
                return Ok(BuiltinAnswer::refused(
                    "task: model tasks are not enabled in this section".to_owned(),
                ));
            }
            Some(allowlist) if !allowlist.permits(target) => {
                let allowed = match allowlist {
                    TaskAllowlist::Only(headings) => headings.join(", "),
                    TaskAllowlist::Any => String::new(),
                };
                return Ok(BuiltinAnswer::refused(format!(
                    "task: target `{target}` is not allowed; allowed targets: {allowed}"
                )));
            }
            Some(_) => {}
        }
        let var = vm.var()?;
        let seed = TaskSeed {
            item: None,
            index: None,
        };
        match self.prepare_spawn(
            id,
            target,
            input,
            seed,
            var,
            TaskOrigin::Model,
            SpawnVia::Spawn,
        ) {
            Ok((task, child)) => Ok(BuiltinAnswer {
                text: format!("Task id={task} started"),
                ok: true,
                started: Some(child),
            }),
            Err(error) => Ok(BuiltinAnswer::refused(format!("task: {error}"))),
        }
    }

    /// The task named by `args` if the model may see it: a model-origin
    /// task the caller owns, or the refusal text.
    fn model_task(
        &self,
        caller: ChainIndex,
        builtin: TaskBuiltin,
        args: &Value,
    ) -> std::result::Result<TaskId, String> {
        let task = task_id_argument(builtin, args)?;
        match self.tasks.get(&task) {
            Some(slot)
                if slot.owner == caller
                    && slot.origin == TaskOrigin::Model
                    && !slot.is_internal() =>
            {
                Ok(task)
            }
            _ => Err(format!("{}: no model task with id {task}", builtin.name())),
        }
    }

    /// The `task_cancel` built-in over a model task the caller owns;
    /// idempotent as `tasks.cancel` is. The confirmation is the model's
    /// whole word on it: no `was canceled` notice follows, unlike an
    /// author's cancel of a model task.
    fn builtin_task_cancel(&mut self, id: ChainIndex, args: &Value) -> BuiltinAnswer {
        let task = match self.model_task(id, TaskBuiltin::TaskCancel, args) {
            Ok(task) => task,
            Err(text) => return BuiltinAnswer::refused(text),
        };
        match self.cancel_task(id, &task) {
            Ok(_) => BuiltinAnswer::served(format!("Task id={task} cancelled")),
            Err(error) => BuiltinAnswer::refused(format!("task_cancel: {error}")),
        }
    }

    /// The `task_status` built-in over a model task the caller owns: the
    /// status line, trusted.
    fn builtin_task_status(&self, id: ChainIndex, args: &Value) -> BuiltinAnswer {
        let task = match self.model_task(id, TaskBuiltin::TaskStatus, args) {
            Ok(task) => task,
            Err(text) => return BuiltinAnswer::refused(text),
        };
        match self.task_status(id, &task) {
            Ok(status) => BuiltinAnswer::served(render_status(&task, &status)),
            Err(error) => BuiltinAnswer::refused(format!("task_status: {error}")),
        }
    }
}

#[cfg(test)]
#[path = "builtins-tests.rs"]
mod tests;
