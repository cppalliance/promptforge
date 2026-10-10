//! The rendering half of the protocol: an answer becomes the `(ok, result)`
//! resume envelope.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::error_value::{ErrorValue, error_table};
use crate::tools::local_handler;

use super::answer::{Answer, TaskDelivery, TaskStatus, ToolCallOutcome, VfsOutcome};

/// Renders one [`TaskStatus`] as the plain Lua status table. Absent
/// optional fields are never set, so they resume as nil; `tasks` is always
/// a sequence of id strings, empty when the task owns nothing live.
fn task_status_table(lua: &Lua, status: TaskStatus) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.raw_set("target", status.target)?;
    table.raw_set("origin", status.origin.tag())?;
    table.raw_set("state", status.state)?;
    if let Some(ok) = status.ok {
        table.raw_set("ok", ok)?;
    }
    if let Some(section) = status.section {
        table.raw_set("section", section)?;
    }
    if let Some(blocked) = status.blocked {
        table.raw_set("blocked", blocked)?;
    }
    table.raw_set("turns", status.turns)?;
    table.raw_set("tasks", task_id_sequence(lua, &status.tasks)?)?;
    table.raw_set("depth", status.depth)?;
    if let Some(note) = status.note {
        table.raw_set("note", note)?;
    }
    Ok(table)
}

/// Renders task ids as a 1-based sequence of their path strings.
fn task_id_sequence(
    lua: &Lua,
    tasks: &[promptforge_types::ids::TaskId],
) -> mlua::Result<mlua::Table> {
    let sequence = lua.create_table_with_capacity(tasks.len(), 0)?;
    for (position, task) in tasks.iter().enumerate() {
        sequence.raw_set(position + 1, task.to_string())?;
    }
    Ok(sequence)
}

/// Renders a store op's return value: nil for the mutating ops, the text
/// for reads, a sequence table for glob, a boolean for exists.
fn store_value(lua: &Lua, outcome: VfsOutcome) -> mlua::Result<Value> {
    Ok(match outcome {
        VfsOutcome::Unit => Value::Nil,
        VfsOutcome::Text(text) => Value::String(lua.create_string(&text)?),
        VfsOutcome::Paths(paths) => Value::Table(lua.create_sequence_from(paths)?),
        VfsOutcome::Bool(exists) => Value::Boolean(exists),
    })
}

/// Renders a `join_any` delivery's resume values after the `ok` flag: the
/// member's id as its path text (the shim wraps it in a `Task` handle),
/// then the member's own `(ok, result)` pair - its final text, or its
/// failure rendered as the error table the shim hands back unraised, so
/// `join` reports it without raising. The failure is also returned as
/// the typed error to retain: a shim that re-raises the member's failure at
/// once (`fanout` on a fatal arm) surfaces the member's own typed error.
fn delivery_values<E: ErrorValue>(
    lua: &Lua,
    delivery: TaskDelivery<E>,
) -> mlua::Result<(Vec<Value>, Option<E>)> {
    let id = Value::String(lua.create_string(delivery.task.to_string())?);
    let (ok, result, retained) = match delivery.outcome {
        Ok(text) => (true, Value::String(lua.create_string(&text)?), None),
        Err(error) => (false, Value::Table(error_table(lua, &error)?), Some(error)),
    };
    Ok((vec![id, Value::Boolean(ok), result], retained))
}

impl<E: ErrorValue> Answer<E> {
    /// Renders the `(ok, result)` resume values for the shim.
    ///
    /// On success the envelope is `(true, value...)`. On failure it is
    /// `(false, table)`, where `table` is the error's structured value
    /// (`kind`, `message` as the error's display string, and the kind's
    /// fields, with `tostring` returning the message) - the shim raises it
    /// with `error(result, 0)`, so a printing author sees exactly the Engine's
    /// message and a branching one reads `kind` - and the typed error
    /// is returned alongside for the driver to retain. A successful
    /// `join_any` whose member failed retains the member's error the same
    /// way, since a shim may re-raise it at once.
    ///
    /// # Errors
    /// Returns an `mlua` error if a Lua string, userdata, or table cannot be
    /// created on `lua`.
    pub fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<E>)> {
        let mut retained = None;
        let values = match self {
            Answer::Infer(Ok(text))
            | Answer::Call(Ok(text))
            | Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(text))) => {
                vec![Value::String(lua.create_string(&text)?)]
            }
            // The task id resumes as its path text; the shim builds the
            // `{ task = id }` table around it, so no Engine handle crosses.
            Answer::Spawn(Ok(task)) | Answer::Timer(Ok(task)) => {
                vec![Value::String(lua.create_string(task.to_string())?)]
            }
            Answer::JoinAny(Ok(delivery)) => {
                let (values, member_error) = delivery_values(lua, delivery)?;
                retained = member_error;
                values
            }
            Answer::Ready(Ok(ready)) => vec![Value::Boolean(ready)],
            Answer::Status(Ok(status)) => vec![Value::Table(task_status_table(lua, *status)?)],
            Answer::Pending(Ok(tasks)) => vec![Value::Table(task_id_sequence(lua, &tasks)?)],
            // The effective limit resumes as a plain number, so the shim
            // returns it as the call's value.
            Answer::Concurrency(Ok(limit)) => {
                vec![Value::Integer(i64::try_from(limit).unwrap_or(i64::MAX))]
            }
            Answer::Note(Ok(())) | Answer::Cancel(Ok(())) => vec![Value::Nil],
            // The one serde-boundary conversion: the parsed JSON output
            // becomes the resumed Lua value, so the shim hands the script a
            // table with no codec in author reach.
            Answer::ToolCallResult(Ok(ToolCallOutcome::Structured(json))) => {
                vec![lua.to_value(&json)?]
            }
            // The result slot stays nil: the handler and its arguments ride
            // after it, so the shim tells a local answer from a bound
            // result by the handler's presence.
            Answer::ToolCallResult(Ok(ToolCallOutcome::Local { alias, args })) => vec![
                Value::Nil,
                Value::Function(local_handler(lua, &alias)?),
                lua.to_value(&args)?,
            ],
            // Opaque to Lua: only the loop's step reads it, by taking it
            // back out whole.
            Answer::Chat(Ok(result)) => vec![Value::UserData(lua.create_userdata(*result)?)],
            Answer::Store(Ok(outcome)) => vec![store_value(lua, outcome)?],
            Answer::Infer(Err(error))
            | Answer::Call(Err(error))
            | Answer::Spawn(Err(error))
            | Answer::Timer(Err(error))
            | Answer::JoinAny(Err(error))
            | Answer::Ready(Err(error))
            | Answer::Status(Err(error))
            | Answer::Pending(Err(error))
            | Answer::Concurrency(Err(error))
            | Answer::Note(Err(error))
            | Answer::Cancel(Err(error))
            | Answer::ToolCallResult(Err(error))
            | Answer::Chat(Err(error))
            | Answer::Store(Err(error)) => {
                let table = error_table(lua, &error)?;
                return Ok((
                    MultiValue::from_vec(vec![Value::Boolean(false), Value::Table(table)]),
                    Some(error),
                ));
            }
        };
        let mut envelope = Vec::with_capacity(values.len() + 1);
        envelope.push(Value::Boolean(true));
        envelope.extend(values);
        Ok((MultiValue::from_vec(envelope), retained))
    }
}
