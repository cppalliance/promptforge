//! The task-operation request parsers: `spawn` and its seeds, the wait
//! shims' internal `timer` and its author-supplied seconds, the `join_any`
//! set, the single-task `ready`, `status`, and `cancel`, the `pending`
//! origin filter, the `note` text, and the `concurrency` limit. The shims
//! resolve a `Task` handle to its bare id before yielding, so every task
//! field arrives as a path string; an id that does not parse is the
//! author's argument error, raised at the call site.

use std::num::NonZeroU64;
use std::time::Duration;

use mlua::{Lua, LuaSerdeExt, Value};
use promptforge_types::ids::{TaskId, TaskOrigin};

use crate::protocol::{NonEmpty, Request, SpawnVia};

use super::{FieldFailure, call_error, call_optional_string, call_string, call_target, shim_var};

/// Parses one task id string; a malformed path is the call's error naming
/// why the parse refused it.
fn parse_task_id(text: &str) -> std::result::Result<TaskId, FieldFailure> {
    match text.parse() {
        Ok(task) => Ok(task),
        Err(error) => Err(call_error(format!(
            "`{text}` is not a task id such as `0.1`: {error}"
        ))),
    }
}

/// Reads the author-supplied `task` id off the request table.
fn call_task(table: &mlua::Table) -> std::result::Result<TaskId, FieldFailure> {
    parse_task_id(&call_string(table, "task")?)
}

/// Parses a `spawn` request: the author-supplied `target` (validated with
/// the `resolve_section_target` rule, as `call`'s is), the optional
/// author-supplied `input`, `item`, and `index` seeds, plus the
/// shim-produced `var` snapshot, `origin`, and `fanout` mark.
pub(super) fn parse_spawn(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<Request, FieldFailure> {
    let target = call_target(table)?;
    let input = call_optional_string(table, "input")?;
    let item = match table.raw_get::<Value>("item") {
        Ok(Value::Nil) => None,
        Ok(value @ (Value::Function(_) | Value::UserData(_) | Value::Thread(_))) => {
            return Err(call_error(format!(
                "item must be JSON data, got {}",
                value.type_name()
            )));
        }
        Ok(value) => match lua.from_value(value) {
            Ok(item) => Some(item),
            Err(error) => {
                return Err(call_error(format!(
                    "item must be a JSON-representable value: {error}"
                )));
            }
        },
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let index = match table.raw_get::<Value>("index") {
        Ok(Value::Nil) => None,
        Ok(Value::Integer(index)) => match u64::try_from(index) {
            Ok(index) => Some(index),
            Err(_) => {
                return Err(call_error(format!(
                    "index must be a non-negative integer, got {index}"
                )));
            }
        },
        Ok(other) => {
            return Err(call_error(format!(
                "index must be a non-negative integer, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let var = shim_var(lua, table)?;
    Ok(Request::Spawn {
        target,
        input,
        item,
        index,
        var,
        origin: shim_origin(table)?,
        via: shim_via(table)?,
    })
}

/// Reads a spawn's shim-produced `origin`: the author shim always says
/// `author`, so any other shape is a hand-built yield.
fn shim_origin(table: &mlua::Table) -> std::result::Result<TaskOrigin, FieldFailure> {
    let Ok(Value::String(tag)) = table.raw_get::<Value>("origin") else {
        return Err(FieldFailure::Malformed);
    };
    let Ok(tag) = tag.to_str() else {
        return Err(FieldFailure::Malformed);
    };
    tag.parse().map_err(|_| FieldFailure::Malformed)
}

/// Reads a spawn's shim-produced `fanout` mark: the `fanout` shim marks its
/// arms, `tasks.spawn` leaves the field absent, and any other shape is a
/// hand-built yield.
fn shim_via(table: &mlua::Table) -> std::result::Result<SpawnVia, FieldFailure> {
    match table.raw_get::<Value>("fanout") {
        Ok(Value::Nil | Value::Boolean(false)) => Ok(SpawnVia::Spawn),
        Ok(Value::Boolean(true)) => Ok(SpawnVia::Fanout),
        Ok(_) | Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Parses a `timer` request: the author-supplied `seconds` (the wait's
/// `opts.timeout`), carried on as the `Duration` it converts to. The value
/// must be a number that `Duration` can hold - non-negative, finite, and in
/// range; any other shape is the call's error, raised at the wait's call
/// site before a timer starts.
pub(super) fn parse_timer(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let seconds = match table.raw_get::<Value>("seconds") {
        Ok(Value::Number(seconds)) => seconds,
        // Every i64 converts to f64 exactly enough for a duration; a
        // magnitude past 2^53 loses low bits no sleep can observe.
        #[expect(
            clippy::cast_precision_loss,
            reason = "a duration in seconds needs no more than f64 precision"
        )]
        Ok(Value::Integer(seconds)) => seconds as f64,
        Ok(other) => {
            return Err(call_error(format!(
                "timeout must be a number, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    match Duration::try_from_secs_f64(seconds) {
        Ok(duration) => Ok(Request::Timer { duration }),
        Err(_) => Err(call_error(format!(
            "timeout must be a non-negative finite number of seconds, got {seconds}"
        ))),
    }
}

/// Parses a `join_any` request: the shim-built `tasks` sequence of id
/// strings. The shim has already rejected an empty or non-table set, so a
/// missing, empty, or non-sequence field is a malformed yield; a member
/// that is not a valid id is the author's argument error.
pub(super) fn parse_join_any(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let Ok(Value::Table(set)) = table.raw_get::<Value>("tasks") else {
        return Err(FieldFailure::Malformed);
    };
    let mut tasks = Vec::new();
    for member in set.sequence_values::<Value>() {
        let Ok(Value::String(text)) = member else {
            return Err(FieldFailure::Malformed);
        };
        let Ok(text) = text.to_str() else {
            return Err(FieldFailure::Malformed);
        };
        tasks.push(parse_task_id(&text)?);
    }
    let Some(tasks) = NonEmpty::from_vec(tasks) else {
        return Err(FieldFailure::Malformed);
    };
    Ok(Request::JoinAny { tasks })
}

/// Parses a `ready` request: the one task id.
pub(super) fn parse_ready(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    Ok(Request::Ready {
        task: call_task(table)?,
    })
}

/// Parses a `status` request: the one task id.
pub(super) fn parse_status(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    Ok(Request::Status {
        task: call_task(table)?,
    })
}

/// Parses a `cancel` request: the one task id.
pub(super) fn parse_cancel(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    Ok(Request::Cancel {
        task: call_task(table)?,
    })
}

/// Parses a `pending` request: the optional author-supplied `origin`
/// filter, which must name one of the two origins when present.
pub(super) fn parse_pending(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let origin = match table.raw_get::<Value>("origin") {
        Ok(Value::Nil) => None,
        Ok(Value::String(tag)) => {
            let Ok(tag) = tag.to_str() else {
                return Err(call_error(
                    "pending filter origin must be a valid UTF-8 string".to_owned(),
                ));
            };
            match tag.parse::<TaskOrigin>() {
                Ok(origin) => Some(origin),
                Err(_) => {
                    return Err(call_error(format!(
                        "pending filter origin must be `author` or `model`, got `{}`",
                        &*tag
                    )));
                }
            }
        }
        Ok(other) => {
            return Err(call_error(format!(
                "pending filter origin must be a string, got {}",
                other.type_name()
            )));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Pending { origin })
}

/// Parses a `note` request: the author-supplied `text`.
pub(super) fn parse_note(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    Ok(Request::Note {
        text: call_string(table, "text")?,
    })
}

/// Parses a `concurrency` request: the optional `limit`. The shim has
/// already validated an author-supplied argument as a positive whole
/// number and raised the `lua` error at the call site, so the field is
/// shim-produced: absent is the read-only form, and a whole number of at
/// least 1 is a limit. A whole-number float the limit type cannot hold
/// (an author's `1e20`) is the call's error, raised at the call site
/// like the shim's own refusal; any other shape is a malformed yield.
pub(super) fn parse_concurrency(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let limit = match table.raw_get::<Value>("limit") {
        Ok(Value::Nil) => None,
        Ok(Value::Integer(limit)) => match u64::try_from(limit).ok().and_then(NonZeroU64::new) {
            Some(limit) => Some(limit),
            None => return Err(FieldFailure::Malformed),
        },
        Ok(Value::Number(limit)) => {
            // A float with an integral value is the author writing `8.0`:
            // the shim's whole-number check let it through, so the parser
            // converts it. An integral magnitude past `u64` (an author's
            // `1e20`) is the call's error, raised at the call site like
            // the shim's own refusal; a fractional, negative, or
            // non-finite float is a hand-rolled yield and stays malformed.
            if limit.fract() != 0.0 || limit < 1.0 {
                return Err(FieldFailure::Malformed);
            }
            // The ceiling is `u64::MAX` rounded up to 2^64, the nearest
            // f64 above it; the strict comparison keeps the cast below
            // the ceiling (the cast saturates at and past it).
            if limit >= 18_446_744_073_709_551_616.0 {
                return Err(call_error(format!(
                    "concurrency limit must be a positive whole number in 64-bit range, got {limit}"
                )));
            }
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the value is checked integral, at least 1, and below 2^64 before the cast"
            )]
            let converted = limit as u64;
            NonZeroU64::new(converted)
        }
        Ok(_) | Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Concurrency { limit })
}
