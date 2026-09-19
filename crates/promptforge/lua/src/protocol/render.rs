//! The rendering half of the protocol: an answer becomes the `(ok, result)`
//! resume envelope and a chat result becomes its plain result table.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::error_value::{ErrorValue, error_table};
use crate::pack_sequence;

use super::answer::{Answer, ChatResult, StoreOutcome, ToolCallOutcome};

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

impl<E: ErrorValue> Answer<E> {
    /// Renders the `(ok, result)` resume values for the shim.
    ///
    /// On success the envelope is `(true, text)` or, for a fanout, `(true,
    /// sequence)` with the packed 1-based result table built on the chain's
    /// VM. On failure it is `(false, table)`, where `table` is the error's
    /// structured value (`kind`, `message` as the error's display string,
    /// and the kind's fields, with `tostring` returning the message) - the
    /// shim raises it with `error(result, 0)`, so a printing author sees
    /// exactly the host's message and a branching one reads `kind` - and
    /// the typed [`Error`] is returned alongside for the driver to retain.
    ///
    /// # Errors
    /// Returns an `mlua` error if a Lua string, userdata, or table cannot be
    /// created on `lua`.
    pub fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<E>)> {
        match self {
            Answer::Infer(Ok(text))
            | Answer::Call(Ok(text))
            | Answer::ToolCallResult(Ok(ToolCallOutcome::Plain(text))) => {
                let text = lua.create_string(&text)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::String(text)]),
                    None,
                ))
            }
            // The task id resumes as its path text; the shim builds the
            // `{ task = id }` table around it, so no host handle crosses.
            Answer::Spawn(Ok(task)) => {
                let text = lua.create_string(task.to_string())?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::String(text)]),
                    None,
                ))
            }
            Answer::ToolCallResult(Ok(ToolCallOutcome::Structured(json))) => {
                // The one serde-boundary conversion: the parsed JSON output
                // becomes the resumed Lua value, so the shim hands the
                // script a table with no codec in author reach.
                let value = lua.to_value(&json)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), value]),
                    None,
                ))
            }
            Answer::Fanout(Ok(results)) => {
                let mut handles = Vec::with_capacity(results.len());
                for result in results {
                    handles.push(lua.create_userdata(result)?);
                }
                let sequence = pack_sequence(lua, handles)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::Table(sequence)]),
                    None,
                ))
            }
            Answer::Chat(Ok(result)) => {
                let table = chat_result_table(lua, *result)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::Table(table)]),
                    None,
                ))
            }
            // The availability flag rides beside the text as a third resume
            // value, so the shim returns both and the broker's fixed
            // fallback sentence stays unspoofable by identical human text.
            Answer::UserInput(Ok(outcome)) => {
                let text = lua.create_string(&outcome.text)?;
                Ok((
                    MultiValue::from_vec(vec![
                        Value::Boolean(true),
                        Value::String(text),
                        Value::Boolean(outcome.available),
                    ]),
                    None,
                ))
            }
            // The store op's return value: nil for the mutating ops, the
            // text for reads, a sequence table for glob, a boolean for
            // exists - the legacy closures' exact return shapes.
            Answer::Store(Ok(outcome)) => {
                let value = match outcome {
                    StoreOutcome::Unit => Value::Nil,
                    StoreOutcome::Text(text) => Value::String(lua.create_string(&text)?),
                    StoreOutcome::Paths(paths) => Value::Table(lua.create_sequence_from(paths)?),
                    StoreOutcome::Bool(exists) => Value::Boolean(exists),
                };
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), value]),
                    None,
                ))
            }
            Answer::Infer(Err(error))
            | Answer::Call(Err(error))
            | Answer::Spawn(Err(error))
            | Answer::Fanout(Err(error))
            | Answer::ToolCallResult(Err(error))
            | Answer::Chat(Err(error))
            | Answer::Store(Err(error))
            | Answer::UserInput(Err(error)) => {
                let table = error_table(lua, &error)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(false), Value::Table(table)]),
                    Some(error),
                ))
            }
        }
    }
}
