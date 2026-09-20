//! The model's task built-ins: `task`, `task_cancel`, `task_status`,
//! `await_tasks` (whose arm lives in the `await_tasks` module), and
//! `task_events` (whose arm lives in the `task_events` module), answered
//! by the scheduler over its task arena, and the round scope they join.
//!
//! An author opts a section in with `tools.allow_tasks(targets?)`, which
//! records an allowlist on the section's tool runtime. While it is set,
//! every `chat` round the section yields advertises the five built-ins
//! beside its bound and local tools ([`advertise_task_builtins`]), and the
//! `tool_call` arm answers a model-issued call to one of them here, before
//! alias lookup, so no bound or local tool can shadow them. Every answer is
//! content the model reads: a started task's id, a cancel's confirmation,
//! a status line, a wait's drained notices, or a refusal naming what was
//! wrong - the engine's own text, so it resumes trusted and its
//! `ToolResult` fires under the model's call id. The one exception is
//! `task_events`, whose answer is the task's reported history - model,
//! tool, and user text among it - and so resumes nonce-wrapped as
//! untrusted. A refusal is observed as a failed tool call, a served answer
//! as a succeeded one.
//!
//! The model sees only its own tasks: a `task_cancel`, `task_status`, or
//! `task_events` naming a task the author started (or one the caller does
//! not own) is refused as unknown, so the model can neither end nor
//! inspect the author's work through its tool surface. The author, by
//! contrast, may adopt the model's tasks through
//! `tasks.pending({ origin = "model" })`.
//!
//! The built-ins' fixed schemas and the function that advertises them
//! live in the `schemas` sibling; this file carries the arms.

#[path = "builtins-schemas.rs"]
mod schemas;

use std::fmt::Write as _;
use std::sync::atomic::Ordering;

use promptforge_api_types::ids::{TaskId, TaskOrigin};
use promptforge_api_types::tools::OutputTrust;
use serde_json::Value;

use crate::execute::protocol::{Answer, TaskStatus, ToolCallOutcome};
use crate::execute::section_context::TaskSeed;
use crate::lua::{SectionVm, TaskAllowlist, ToolBinding, ToolSet};
use crate::model::ToolSchema;
use crate::{Error, Result};
use promptforge_api_types::event::lifecycle;

use super::dispatch::unbound_tool_call;
use super::tool_call::ToolCallDispatch;
use super::{ChainIndex, Scheduler};

pub(super) use schemas::advertise_task_builtins;

/// The built-in names answered over the arena, in the order the model
/// sees them advertised.
const TASK_BUILTINS: [&str; 5] = [
    "task",
    "task_cancel",
    "task_status",
    "await_tasks",
    "task_events",
];

/// Whether `name` is one of the built-ins answered here.
pub(super) fn is_task_builtin(name: &str) -> bool {
    TASK_BUILTINS.contains(&name)
}

/// The section's task allowlist, read off its VM's tool runtime: `None`
/// until `tools.allow_tasks` has run in the section.
///
/// # Errors
/// Returns [`Error::Lua`] when the runtime's mutex is poisoned.
pub(super) fn task_allowlist(vm: &SectionVm) -> Result<Option<TaskAllowlist>> {
    let runtime = vm
        .tool_runtime
        .lock()
        .map_err(|_| Error::Lua("tool declaration runtime was poisoned".to_owned()))?;
    Ok(runtime.allowed_tasks.clone())
}

/// The bound and local halves of one round's tool scope, resolved from the
/// request's `tools` against the section: an absent list is the section's
/// current effective scope plus every local Lua tool; an explicit list
/// names its members, each a local tool, an effective binding (which
/// carries the section's description override), or a bound catalog slot.
///
/// # Errors
/// Returns [`Error::UnboundToolCall`] when an explicit alias names no
/// local tool and no bound slot.
pub(super) fn scope_halves(
    tools: Option<&[String]>,
    effective: Vec<ToolBinding>,
    local_schemas: Vec<ToolSchema>,
    tool_set: &ToolSet,
) -> Result<(Vec<ToolBinding>, Vec<ToolSchema>)> {
    let Some(aliases) = tools else {
        return Ok((effective, local_schemas));
    };
    let mut bound = Vec::with_capacity(aliases.len());
    let mut locals = Vec::new();
    for alias in aliases {
        if let Some(schema) = local_schemas.iter().find(|schema| &schema.name == alias) {
            locals.push(schema.clone());
            continue;
        }
        let binding = effective
            .iter()
            .find(|binding| binding.alias() == alias)
            .or_else(|| tool_set.binding(alias))
            .cloned();
        match binding {
            Some(binding) => bound.push(binding),
            None => return Err(unbound_tool_call(tool_set, alias)),
        }
    }
    Ok((bound, locals))
}

/// One built-in's answer: the text the model reads, whether it served the
/// call or refused it, and the task chain a `task` started behind the
/// caller, which the dispatcher enqueues after the caller so the caller
/// runs first as it does after `tasks.spawn`.
pub(super) struct BuiltinAnswer {
    pub(super) text: String,
    pub(super) ok: bool,
    /// Whether `text` is the engine's own (every answer but a history
    /// read's, whose events carry model, tool, and user text and arrive
    /// nonce-wrapped as [`OutputTrust::Untrusted`]).
    pub(super) trust: OutputTrust,
    pub(super) started: Option<ChainIndex>,
}

impl BuiltinAnswer {
    pub(super) fn served(text: String) -> Self {
        Self {
            text,
            ok: true,
            trust: OutputTrust::Trusted,
            started: None,
        }
    }

    /// A served answer whose text is not the engine's own: already
    /// nonce-wrapped by the caller, reported untrusted.
    pub(super) fn served_untrusted(text: String) -> Self {
        Self {
            text,
            ok: true,
            trust: OutputTrust::Untrusted,
            started: None,
        }
    }

    pub(super) fn refused(text: String) -> Self {
        Self {
            text,
            ok: false,
            trust: OutputTrust::Trusted,
            started: None,
        }
    }
}

/// How one built-in call resolved: an answer for the caller now, the
/// caller parked (`await_tasks` on live tasks), answered when it wakes, or
/// a leaf effect issued (`task_events`), answered when the host does.
pub(super) enum BuiltinOutcome {
    Answered(BuiltinAnswer),
    Parked,
    Issued,
}

/// Reads the `id` argument of `task_cancel`, `task_status`, or
/// `task_events`, or the refusal text for a missing or malformed one.
fn task_id_argument(name: &str, args: &Value) -> std::result::Result<TaskId, String> {
    let Some(id) = args.get("id").and_then(Value::as_str) else {
        return Err(format!(
            "{name}: `id` must be a task id string, exactly as `task` returned it"
        ));
    };
    id.parse()
        .map_err(|_| format!("{name}: `{id}` is not a task id; use the id `task` returned"))
}

/// Renders one status for the model: the id, the target heading, the
/// state (with the outcome for a finished task), then whatever the live
/// chain reports - where it is, what it waits on, its turns, its own live
/// tasks, and its latest note.
fn render_status(task: &TaskId, status: &TaskStatus) -> String {
    let mut text = format!("Task id={task} (## {}): {}", status.target, status.state);
    if status.state == "done" {
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
        let _ = write!(text, ", waiting on {blocked}");
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
    /// task ends, or a `TaskEvents` effect issued, for a `task_events`
    /// whose answer comes from the host's log. Only the caller's own
    /// bookkeeping can fail here (a lost frame, a poisoned runtime); every
    /// model-facing fault is the answer's text. The `tool_call` arm routes
    /// only [`is_task_builtin`] names here.
    ///
    /// # Errors
    /// Returns the internal fault the arm met, or [`Error::Internal`] for
    /// a name outside [`TASK_BUILTINS`].
    pub(super) fn answer_task_builtin(
        &mut self,
        id: ChainIndex,
        name: &str,
        args: &Value,
        call_id: &str,
    ) -> Result<ToolCallDispatch> {
        let outcome = match name {
            "task" => BuiltinOutcome::Answered(self.builtin_task(id, args)?),
            "task_cancel" => BuiltinOutcome::Answered(self.builtin_task_cancel(id, args)),
            "task_status" => BuiltinOutcome::Answered(self.builtin_task_status(id, args)),
            "await_tasks" => self.builtin_await_tasks(id, args, call_id),
            "task_events" => self.builtin_task_events(id, args, call_id),
            _ => {
                return Err(Error::internal(
                    "the tool_call arm routes only the answered task built-ins here",
                ));
            }
        };
        let answer = match outcome {
            BuiltinOutcome::Answered(answer) => answer,
            BuiltinOutcome::Parked => return Ok(ToolCallDispatch::Parked),
            BuiltinOutcome::Issued => return Ok(ToolCallDispatch::Issued),
        };
        let started = answer.started;
        let answer = self.report_builtin_answer(id, name, call_id, answer);
        Ok(match started {
            Some(child) => ToolCallDispatch::Started(answer, child),
            None => ToolCallDispatch::Answered(answer),
        })
    }

    /// Reports one built-in's answer - the succeeded/failed observation and
    /// the `ToolResult` under the model's call id, trusted unless the
    /// answer says otherwise - and renders it as the tool call's answer.
    /// Shared by the immediate answers, the `await_tasks` wake, and the
    /// `task_events` answer.
    pub(super) fn report_builtin_answer(
        &self,
        id: ChainIndex,
        name: &str,
        call_id: &str,
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
            chain.ctx.turns().load(Ordering::Relaxed),
            call_id,
            name,
            &answer.text,
            answer.trust,
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
        let chain = &self.chains[id.index()];
        let frame = chain
            .frame
            .as_ref()
            .ok_or(Error::internal("a live chain holds its frame"))?;
        let vm = frame.vm()?;
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
        match self.prepare_spawn(id, target, input, seed, &var, TaskOrigin::Model, false) {
            Ok((task, child)) => Ok(BuiltinAnswer {
                text: format!("Task id={task} started"),
                ok: true,
                trust: OutputTrust::Trusted,
                started: Some(child),
            }),
            Err(error) => Ok(BuiltinAnswer::refused(format!("task: {error}"))),
        }
    }

    /// The task named by `args` if the model may see it: a model-origin
    /// task the caller owns, or the refusal text.
    pub(super) fn model_task(
        &self,
        caller: ChainIndex,
        name: &str,
        args: &Value,
    ) -> std::result::Result<TaskId, String> {
        let task = task_id_argument(name, args)?;
        match self.tasks.get(&task) {
            Some(slot)
                if slot.owner == caller
                    && slot.origin == TaskOrigin::Model
                    && !slot.is_internal() =>
            {
                Ok(task)
            }
            _ => Err(format!("{name}: no model task with id {task}")),
        }
    }

    /// The `task_cancel` built-in over a model task the caller owns;
    /// idempotent as `tasks.cancel` is. The confirmation is the model's
    /// whole word on it: no `was canceled` notice follows, unlike an
    /// author's cancel of a model task.
    fn builtin_task_cancel(&mut self, id: ChainIndex, args: &Value) -> BuiltinAnswer {
        let task = match self.model_task(id, "task_cancel", args) {
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
        let task = match self.model_task(id, "task_status", args) {
            Ok(task) => task,
            Err(text) => return BuiltinAnswer::refused(text),
        };
        match self.task_status(id, &task) {
            Ok(status) => BuiltinAnswer::served(render_status(&task, &status)),
            Err(error) => BuiltinAnswer::refused(format!("task_status: {error}")),
        }
    }
}
