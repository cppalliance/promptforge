//! The rendering half of the protocol: an answer becomes the `(ok, result)`
//! resume envelope and a chat result becomes its plain result table.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::error_value::{ErrorValue, error_table};

use super::answer::{Answer, ChatResult, StoreOutcome, TaskDelivery, TaskStatus, ToolCallOutcome};

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
    tasks: &[promptforge_api_types::ids::TaskId],
) -> mlua::Result<mlua::Table> {
    let sequence = lua.create_table_with_capacity(tasks.len(), 0)?;
    for (position, task) in tasks.iter().enumerate() {
        sequence.raw_set(position + 1, task.to_string())?;
    }
    Ok(sequence)
}

/// Renders one [`ChatResult`] as the plain Lua result table.
///
/// `overflow` is always set as a boolean, so the loop shim branches on it
/// with a plain truth test; `overflow_reason` rides beside it as the
/// compactor's tag when the request was refused. Absent optional fields
/// are never set, so they resume as nil and `result.tool_calls` and
/// `result.reply` presence-branching works; mapping them through the
/// serde boundary would resume mlua's non-nil null sentinel instead. An
/// empty `reply` string is dropped here as well, so an empty reply resumes
/// as nil whether the producer left the field absent (its documented
/// shape) or handed over `Some("")`: the shim's exit rules read presence,
/// never length, and `empty_detail` supplies the message they raise. Each
/// call's `arguments` and the `metrics` sections cross the serde boundary
/// as tables (the metrics types skip absent sections in serialization, so
/// no null enters them).
fn chat_result_table(lua: &Lua, result: ChatResult) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    table.raw_set("overflow", result.overflow)?;
    if let Some(reason) = result.overflow_reason {
        table.raw_set("overflow_reason", reason.tag())?;
    }
    if let Some(reply) = result.reply.filter(|reply| !reply.is_empty()) {
        table.raw_set("reply", reply)?;
    }
    if let Some(detail) = result.empty_detail {
        table.raw_set("empty_detail", detail)?;
    }
    if let Some(calls) = result.tool_calls {
        let sequence = lua.create_table_with_capacity(calls.len(), 0)?;
        for (position, call) in calls.into_iter().enumerate() {
            let entry = lua.create_table()?;
            entry.raw_set("id", call.id)?;
            entry.raw_set("name", call.name)?;
            entry.raw_set("arguments", lua.to_value(&call.arguments)?)?;
            sequence.raw_set(position + 1, entry)?;
        }
        table.raw_set("tool_calls", sequence)?;
    }
    if let Some(finish_reason) = result.finish_reason {
        table.raw_set("finish_reason", finish_reason)?;
    }
    table.raw_set("model", result.model)?;
    if let Some(metrics) = result.metrics {
        table.raw_set("metrics", lua.to_value(&metrics)?)?;
    }
    Ok(table)
}

/// Renders a store op's return value: nil for the mutating ops, the text
/// for reads, a sequence table for glob, a boolean for exists - the legacy
/// closures' exact return shapes.
fn store_value(lua: &Lua, outcome: StoreOutcome) -> mlua::Result<Value> {
    Ok(match outcome {
        StoreOutcome::Unit => Value::Nil,
        StoreOutcome::Text(text) => Value::String(lua.create_string(&text)?),
        StoreOutcome::Paths(paths) => Value::Table(lua.create_sequence_from(paths)?),
        StoreOutcome::Bool(exists) => Value::Boolean(exists),
    })
}

/// Renders a `when_any` delivery's resume values after the `ok` flag: the
/// member's id as its path text (the shim wraps it in a `Task` handle),
/// then the member's own `(ok, result)` pair - its final text, or its
/// failure rendered as the error table the shim hands back unraised, so
/// `when_all` reports it without raising. The failure is also returned as
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
    /// with `error(result, 0)`, so a printing author sees exactly the host's
    /// message and a branching one reads `kind` - and the typed [`Error`]
    /// is returned alongside for the driver to retain. A successful
    /// `when_any` whose member failed retains the member's error the same
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
            // `{ task = id }` table around it, so no host handle crosses.
            Answer::Spawn(Ok(task)) | Answer::Timer(Ok(task)) => {
                vec![Value::String(lua.create_string(task.to_string())?)]
            }
            Answer::WhenAny(Ok(delivery)) => {
                let (values, member_error) = delivery_values(lua, delivery)?;
                retained = member_error;
                values
            }
            Answer::Ready(Ok(ready)) => vec![Value::Boolean(ready)],
            Answer::Status(Ok(status)) => vec![Value::Table(task_status_table(lua, *status)?)],
            Answer::Pending(Ok(tasks)) => vec![Value::Table(task_id_sequence(lua, &tasks)?)],
            Answer::Note(Ok(())) | Answer::Cancel(Ok(())) => vec![Value::Nil],
            // Always a sequence, empty included, so the shim's `#` and
            // `ipairs` need no nil check.
            Answer::DrainTaskNotices(Ok(notices)) => {
                vec![Value::Table(lua.create_sequence_from(notices)?)]
            }
            // The one serde-boundary conversion: the parsed JSON output
            // becomes the resumed Lua value, so the shim hands the script a
            // table with no codec in author reach.
            Answer::ToolCallResult(Ok(ToolCallOutcome::Structured(json))) => {
                vec![lua.to_value(&json)?]
            }
            Answer::Chat(Ok(result)) => vec![Value::Table(chat_result_table(lua, *result)?)],
            // The availability flag rides beside the text as a third resume
            // value, so the shim returns both and the broker's fixed
            // fallback sentence stays unspoofable by identical human text.
            Answer::UserInput(Ok(outcome)) => vec![
                Value::String(lua.create_string(&outcome.text)?),
                Value::Boolean(outcome.available),
            ],
            Answer::Store(Ok(outcome)) => vec![store_value(lua, outcome)?],
            Answer::Infer(Err(error))
            | Answer::Call(Err(error))
            | Answer::Spawn(Err(error))
            | Answer::Timer(Err(error))
            | Answer::WhenAny(Err(error))
            | Answer::Ready(Err(error))
            | Answer::Status(Err(error))
            | Answer::Pending(Err(error))
            | Answer::Note(Err(error))
            | Answer::Cancel(Err(error))
            | Answer::DrainTaskNotices(Err(error))
            | Answer::ToolCallResult(Err(error))
            | Answer::Chat(Err(error))
            | Answer::Store(Err(error))
            | Answer::UserInput(Err(error)) => {
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
