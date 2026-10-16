//! The tool request parsers: a `tools.call`, the loop's batched
//! `tool_calls`, and a local tool's `local_tool_done`.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use crate::protocol::{LocalToolOutcome, Request, ToolCallRecord};
use crate::tools::tool_name;
use crate::vm::scalar_return;

use super::{FieldFailure, call_error, shim_optional_string, shim_optional_u32};

/// The fields of one parsed `tool_call` table: a `tools.call` request's,
/// or one entry of the loop's `tool_calls` batch.
struct ToolCallFields {
    alias: String,
    args: serde_json::Value,
    call_id: Option<String>,
    turn: Option<u32>,
}

/// Parses a `tools.call` request from the fields [`read_tool_call`] reads.
pub(super) fn parse_tool_call(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<Request, FieldFailure> {
    let ToolCallFields {
        alias,
        args,
        call_id,
        turn,
    } = read_tool_call(lua, table)?;
    Ok(Request::ToolCall {
        alias,
        args,
        call_id,
        turn,
    })
}

/// Reads a `tool_call` table: the author-supplied `alias` (a string or a
/// Tool object, decoded through the one alias-or-Tool polymorphism) and
/// `args`, plus the shim-produced optional `call_id` and `turn`.
///
/// An absent or nil `args` parses as the empty object (the empty-argument
/// call every tool accepts). A non-table or JSON-unrepresentable `args` is
/// the call's error, framed as the other author-argument failures are,
/// so an author `pcall` catches it at the call site. `call_id` is set only
/// by the loop shim for a model-issued call, so a present non-string is a
/// malformed yield rather than a call error; `turn`, set by the same shim,
/// is likewise malformed unless it is an integer within `u32` range.
fn read_tool_call(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<ToolCallFields, FieldFailure> {
    let alias = match table.raw_get::<Value>("alias") {
        // Flatten to the call-error string so the answer frames exactly as
        // the other author-argument failures (`Error::Lua`, not a runtime
        // wrapper).
        Ok(value) => tool_name(&value).map_err(|error| call_error(error.to_string()))?,
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let args = match table.raw_get::<Value>("args") {
        Ok(Value::Nil) => serde_json::Value::Object(serde_json::Map::new()),
        Ok(value @ Value::Table(_)) => match lua.from_value(value) {
            Ok(args) => args,
            Err(error) => {
                return Err(call_error(format!(
                    "args must be a JSON-representable table: {error}"
                )));
            }
        },
        Ok(other) => {
            return Err(call_error(format!(
                "args must be a table, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let call_id = shim_optional_string(table, "call_id")?;
    let turn = shim_optional_u32(table, "turn")?;
    Ok(ToolCallFields {
        alias,
        args,
        call_id,
        turn,
    })
}

/// Parses a `tool_calls` request, which only the loop's step yields: the
/// required `turn` and the `calls` sequence, each entry read as a
/// `tool_call` request's `alias`, `args`, and required `call_id`. Any
/// other shape is a malformed yield.
pub(super) fn parse_tool_calls(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<Request, FieldFailure> {
    let Some(turn) = shim_optional_u32(table, "turn")? else {
        return Err(FieldFailure::Malformed);
    };
    let Ok(Value::Table(entries)) = table.raw_get::<Value>("calls") else {
        return Err(FieldFailure::Malformed);
    };
    let calls = (1..=entries.raw_len())
        .map(|index| {
            let Ok(Value::Table(entry)) = entries.raw_get::<Value>(index) else {
                return Err(FieldFailure::Malformed);
            };
            let ToolCallFields {
                alias,
                args,
                call_id: Some(id),
                ..
            } = read_tool_call(lua, &entry)?
            else {
                return Err(FieldFailure::Malformed);
            };
            Ok(ToolCallRecord {
                id,
                name: alias,
                arguments: args,
            })
        })
        .collect::<std::result::Result<_, _>>()?;
    Ok(Request::ToolCalls { calls, turn })
}

/// Parses a `local_tool_done` request: the shim-produced `ok` flag and,
/// when the handler returned, its first return value as `value`. A
/// returned value takes the scalar-return rule a block's return takes, so
/// nil reads as `""` and a table is a [`LocalToolOutcome::BadReturn`].
/// `None` for a missing or non-boolean `ok`.
pub(super) fn parse_local_tool_done(table: &mlua::Table) -> Option<Request> {
    let outcome = match table.raw_get::<Value>("ok").ok()? {
        Value::Boolean(true) => {
            let value = table.raw_get::<Value>("value").ok()?;
            match scalar_return(MultiValue::from_vec(vec![value])) {
                Ok(text) => LocalToolOutcome::Returned(text.unwrap_or_default()),
                Err(error) => LocalToolOutcome::BadReturn(error),
            }
        }
        Value::Boolean(false) => LocalToolOutcome::Raised,
        _ => return None,
    };
    Some(Request::LocalToolDone { outcome })
}
