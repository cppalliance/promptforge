//! The fixed schemas of the model's task built-ins, and the one function
//! that appends them to a round's advertised scope. The five are the
//! engine's own: the descriptions name what each call does and what the
//! answer looks like, and the `task` description names the allowlisted
//! targets so the model copies a heading the arm will accept. The arms
//! that answer the calls are defined in the parent module.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::execute::scope::DispatchTarget;
use crate::lua::TaskAllowlist;
use crate::model::ToolSchema;
use crate::{Error, Result};
use promptforge_model_client::detail::tool_schema_new;

/// One built-in's fixed schema; the five are the engine's own, so a
/// refusal by the validated constructor is an internal fault.
fn builtin_schema(name: &str, description: String, parameters: Value) -> Result<ToolSchema> {
    tool_schema_new(name.to_owned(), description, parameters)
        .map_err(|_| Error::internal("a task built-in's fixed schema validates"))
}

/// Appends the five task built-ins to a round's advertised `schemas` and
/// `dispatch` map under `allowlist`, whose targets the `task` description
/// names so the model copies a heading the arm will accept.
///
/// # Errors
/// Returns [`Error::Internal`] when a fixed schema fails to validate.
pub(crate) fn advertise_task_builtins(
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
        builtin_schema(
            "await_tasks",
            "Wait until one of the tasks you started ends, then return every task result \
             that arrived. With `timeout` (seconds), return after that long at the latest, \
             naming the tasks still running; with no running task and no timeout, return \
             at once."
                .to_owned(),
            json!({
                "type": "object",
                "properties": {
                    "timeout": {
                        "type": "number",
                        "description": "Optional: the most seconds to wait."
                    }
                }
            }),
        )?,
        builtin_schema(
            "task_events",
            "Read what a task you started has reported so far: its sections, model \
             turns, tool calls, and their content, one JSON event per line in order. \
             With `last` (the `seq` of the last event you read), return only later \
             events."
                .to_owned(),
            json!({
                "type": "object",
                "properties": {
                    "id": {
                        "type": "string",
                        "description": "The task id, exactly as `task` returned it."
                    },
                    "last": {
                        "type": "integer",
                        "description": "Optional: the `provenance.seq` of the last event already read."
                    }
                },
                "required": ["id"]
            }),
        )?,
    ];
    for schema in built {
        dispatch.insert(schema.name.clone(), DispatchTarget::Builtin);
        schemas.push(schema);
    }
    Ok(())
}
