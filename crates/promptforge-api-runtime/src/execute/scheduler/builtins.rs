//! The model's task built-ins: `task`, `task_cancel`, and `task_status`,
//! answered by the scheduler over its task arena, and the round scope they
//! join.
//!
//! An author opts a section in with `tools.allow_tasks(targets?)`, which
//! records an allowlist on the section's tool runtime. While it is set,
//! every `chat` round the section yields advertises the three built-ins
//! beside its bound and local tools ([`advertise_task_builtins`]), and the
//! `tool_call` arm answers a model-issued call to one of them here, before
//! alias lookup, so no bound or local tool can shadow them. Every answer is
//! content the model reads: a started task's id, a cancel's confirmation,
//! a status line, or a refusal naming what was wrong - the engine's own
//! text, so it resumes trusted and its `ToolResult` fires under the
//! model's call id. A refusal is observed as a failed tool call, a served
//! answer as a succeeded one.
//!
//! The model sees only its own tasks: a `task_cancel` or `task_status`
//! naming a task the author started (or one the caller does not own) is
//! refused as unknown, so the model can neither end nor inspect the
//! author's work through its tool surface. The author, by contrast, may
//! adopt the model's tasks through `tasks.pending({ origin = "model" })`.
//! `task_events` and `await_tasks` are reserved names still; they answer
//! as unbound until their arms land.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use promptforge_api_types::ids::{TaskId, TaskOrigin};
use serde_json::{Value, json};

use crate::client::ToolSchema;
use crate::execute::protocol::{Answer, TaskStatus, ToolCallOutcome};
use crate::execute::scope::DispatchTarget;
use crate::execute::section_context::TaskSeed;
use crate::lua::{SectionVm, TaskAllowlist, ToolBinding, ToolSet};
use crate::observe::detail;
use crate::{Error, Result};

use super::dispatch::unbound_tool_call;
use super::{ChainIndex, Scheduler};

/// The built-in names this module answers, in the order the model sees
/// them advertised.
const TASK_BUILTINS: [&str; 3] = ["task", "task_cancel", "task_status"];

/// Whether `name` is one of the built-ins answered here (not merely
/// reserved).
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

/// One built-in's fixed schema; the three are the engine's own, so a
/// refusal by the validated constructor is an internal fault.
fn builtin_schema(name: &str, description: String, parameters: Value) -> Result<ToolSchema> {
    ToolSchema::new(name.to_owned(), description, parameters)
        .map_err(|_| Error::internal("a task built-in's fixed schema validates"))
}

/// Appends the three task built-ins to a round's advertised `schemas` and
/// `dispatch` map under `allowlist`, whose targets the `task` description
/// names so the model copies a heading the arm will accept.
///
/// # Errors
/// Returns [`Error::Internal`] when a fixed schema fails to validate.
pub(super) fn advertise_task_builtins(
    schemas: &mut Vec<ToolSchema>,
    dispatch: &mut BTreeMap<String, DispatchTarget>,
    allowlist: &TaskAllowlist,
) -> Result<()> {
    let targets = match allowlist {
        TaskAllowlist::Any => {
            "any section of this prompt, named by its heading (for example `## Research`)"
                .to_owned()
        }
        TaskAllowlist::Only(headings) => format!("one of: {}", headings.join(", ")),
    };
    let id_parameters = json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The task id, exactly as `task` returned it."
            }
        },
        "required": ["id"]
    });
    let built = [
        builtin_schema(
            "task",
            format!(
                "Start a background task running one section of this prompt and return at \
                 once with `Task id=N started`. The task runs beside you; check on it with \
                 `task_status`, and its result arrives as a notice when it ends. `target` \
                 must be {targets}. `input` optionally replaces the task's arguments."
            ),
            json!({
                "type": "object",
                "properties": {
                    "target": {
                        "type": "string",
                        "description": "The heading of the section to run, such as `## Research`."
                    },
                    "input": {
                        "type": "string",
                        "description": "Optional replacement for the task's arguments."
                    }
                },
                "required": ["target"]
            }),
        )?,
        builtin_schema(
            "task_cancel",
            "Cancel a task you started, by id. Cancelling a task that already ended does \
             nothing."
                .to_owned(),
            id_parameters.clone(),
        )?,
        builtin_schema(
            "task_status",
            "Report a task you started: running, done, cancelled, or abandoned, with what \
             it is waiting on and its latest progress note."
                .to_owned(),
            id_parameters,
        )?,
    ];
    for schema in built {
        dispatch.insert(schema.name.clone(), DispatchTarget::Builtin);
        schemas.push(schema);
    }
    Ok(())
}

/// One built-in's answer: the text the model reads, whether it served the
/// call or refused it, and the task chain a `task` started behind the
/// caller, which the dispatcher enqueues after the caller so the caller
/// runs first as it does after `tasks.spawn`.
pub(super) struct BuiltinAnswer {
    pub(super) text: String,
    pub(super) ok: bool,
    pub(super) started: Option<ChainIndex>,
}

impl BuiltinAnswer {
    fn served(text: String) -> Self {
        Self {
            text,
            ok: true,
            started: None,
        }
    }

    fn refused(text: String) -> Self {
        Self {
            text,
            ok: false,
            started: None,
        }
    }
}

/// Reads the `id` argument of `task_cancel` or `task_status`, or the
/// refusal text for a missing or malformed one.
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

impl Scheduler<'_> {
    /// Answers a model-issued call to one of the task built-ins on the
    /// driver thread: the arm's answer, its succeeded/failed observation,
    /// and the trusted `ToolResult` report under the model's call id. Only
    /// the caller's own bookkeeping can fail here (a lost frame, a
    /// poisoned runtime); every model-facing fault is the answer's text.
    /// The `tool_call` arm routes only [`is_task_builtin`] names here; a
    /// reserved name without an arm never reaches this method.
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
    ) -> Result<(Answer<Error>, Option<ChainIndex>)> {
        let answer = match name {
            "task" => self.builtin_task(id, args)?,
            "task_cancel" => self.builtin_task_cancel(id, args),
            "task_status" => self.builtin_task_status(id, args),
            _ => {
                return Err(Error::internal(
                    "the tool_call arm routes only the answered task built-ins here",
                ));
            }
        };
        let chain = &self.chains[id.index()];
        let observer = Arc::clone(chain.ctx.observer());
        let execution = chain.ctx.execution();
        let section = chain.section_name();
        observer.observe(
            execution,
            section,
            if answer.ok {
                detail::TOOL_CALL_SUCCEEDED
            } else {
                detail::TOOL_CALL_FAILED
            },
        );
        observer.on_tool_result(
            execution,
            section,
            id.0,
            // The call depth is capped far inside u32; the saturation is a
            // defensive no-op.
            u32::try_from(chain.call_depth).unwrap_or(u32::MAX),
            chain.ctx.turns().load(Ordering::Relaxed),
            call_id,
            name,
            &answer.text,
            true,
        );
        Ok((
            Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(answer.text))),
            answer.started,
        ))
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
    /// idempotent as `tasks.cancel` is.
    fn builtin_task_cancel(&mut self, id: ChainIndex, args: &Value) -> BuiltinAnswer {
        let task = match self.model_task(id, "task_cancel", args) {
            Ok(task) => task,
            Err(text) => return BuiltinAnswer::refused(text),
        };
        match self.cancel_task(id, &task) {
            Ok(()) => BuiltinAnswer::served(format!("Task id={task} cancelled")),
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
