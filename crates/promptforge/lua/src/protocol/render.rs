//! The rendering half of the protocol: an answer becomes the `(ok, result)`
//! resume envelope, a chat result becomes its plain result table, and a
//! message record appends to an author's message list.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::{Error, Result, pack_sequence};

use super::answer::{Answer, ChatResult, StoreOutcome, ToolCallOutcome};
use super::request::{ContentPart, MessageContent, MessageRecord};

/// Appends one record to an author's message list - the table behind
/// `key`, stashed by the loop request's parse - rendered as the plain
/// record shape the protocol parse consumes: `role`, `content` (a string
/// or a content-parts array), `tool_calls` when the record carries calls,
/// and `tool_call_id` when it answers one. The append is raw, so a
/// `messages.new()` list's builder metatable never intercepts it.
///
/// The driver calls this as the loop's append sink: every assistant
/// message and correlated tool result lands in the author's own table, in
/// order, as its round completes.
///
/// # Errors
/// Returns [`Error::Lua`] if the registry read, a table creation, or a raw
/// set fails.
pub fn append_message_record(
    lua: &Lua,
    key: &mlua::RegistryKey,
    record: &MessageRecord,
) -> Result<()> {
    let list: mlua::Table = lua.registry_value(key).map_err(Error::lua)?;
    let entry = lua.create_table().map_err(Error::lua)?;
    entry
        .raw_set("role", record.role.as_str())
        .map_err(Error::lua)?;
    match &record.content {
        MessageContent::Text(text) => entry
            .raw_set("content", text.as_str())
            .map_err(Error::lua)?,
        MessageContent::Parts(parts) => {
            let sequence = lua
                .create_table_with_capacity(parts.len(), 0)
                .map_err(Error::lua)?;
            for (position, part) in parts.iter().enumerate() {
                let rendered = lua.create_table().map_err(Error::lua)?;
                match part {
                    ContentPart::Text(text) => {
                        rendered.raw_set("type", "text").map_err(Error::lua)?;
                        rendered
                            .raw_set("text", text.as_str())
                            .map_err(Error::lua)?;
                    }
                    ContentPart::ImageUrl(url) => {
                        rendered.raw_set("type", "image_url").map_err(Error::lua)?;
                        let image = lua.create_table().map_err(Error::lua)?;
                        image.raw_set("url", url.as_str()).map_err(Error::lua)?;
                        rendered.raw_set("image_url", image).map_err(Error::lua)?;
                    }
                }
                sequence
                    .raw_set(position + 1, rendered)
                    .map_err(Error::lua)?;
            }
            entry.raw_set("content", sequence).map_err(Error::lua)?;
        }
    }
    if !record.tool_calls.is_empty() {
        let sequence = lua
            .create_table_with_capacity(record.tool_calls.len(), 0)
            .map_err(Error::lua)?;
        for (position, call) in record.tool_calls.iter().enumerate() {
            let rendered = lua.create_table().map_err(Error::lua)?;
            rendered
                .raw_set("id", call.id.as_str())
                .map_err(Error::lua)?;
            rendered
                .raw_set("name", call.name.as_str())
                .map_err(Error::lua)?;
            rendered
                .raw_set(
                    "arguments",
                    lua.to_value(&call.arguments).map_err(Error::lua)?,
                )
                .map_err(Error::lua)?;
            sequence
                .raw_set(position + 1, rendered)
                .map_err(Error::lua)?;
        }
        entry.raw_set("tool_calls", sequence).map_err(Error::lua)?;
    }
    if let Some(id) = &record.tool_call_id {
        entry
            .raw_set("tool_call_id", id.as_str())
            .map_err(Error::lua)?;
    }
    let length = list.raw_len();
    list.raw_set(length + 1, entry).map_err(Error::lua)
}

/// Renders one [`ChatResult`] as the plain Lua result table.
///
/// Absent optional fields are never set, so they resume as nil and
/// `result.tool_calls` presence-branching works; mapping them through the
/// serde boundary would resume mlua's non-nil null sentinel instead. Each
/// call's `arguments` and the `metrics` sections cross the serde boundary
/// as tables (the metrics types skip absent sections in serialization, so
/// no null enters them).
fn chat_result_table(lua: &Lua, result: ChatResult) -> mlua::Result<mlua::Table> {
    let table = lua.create_table()?;
    if let Some(reply) = result.reply {
        table.raw_set("reply", reply)?;
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

impl<E: std::fmt::Display> Answer<E> {
    /// Renders the `(ok, result)` resume values for the shim.
    ///
    /// On success the envelope is `(true, text)` or, for a fanout, `(true,
    /// sequence)` with the packed 1-based result table built on the chain's
    /// VM. On failure it is `(false, message)`, where `message` is the
    /// error's display string - the shim raises it with `error(result, 0)`,
    /// so the author sees exactly the host's message - and the typed
    /// [`Error`] is returned alongside for the driver to retain.
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
            // The loop appended the history itself; success resumes as
            // `(true, nil)` so the shim returns nil.
            Answer::Loop(Ok(())) => Ok((
                MultiValue::from_vec(vec![Value::Boolean(true), Value::Nil]),
                None,
            )),
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
            | Answer::Fanout(Err(error))
            | Answer::ToolCallResult(Err(error))
            | Answer::Chat(Err(error))
            | Answer::Loop(Err(error))
            | Answer::Store(Err(error))
            | Answer::UserInput(Err(error)) => {
                let message = lua.create_string(error.to_string())?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(false), Value::String(message)]),
                    Some(error),
                ))
            }
        }
    }
}
