//! The task-operation request parsers: the wait shims' internal `timer`
//! and its author-supplied seconds, the `join_any` set, the single-task
//! `ready`, `status`, and `cancel`, the `task_events` id and `last`
//! bound, the `pending` origin filter, the `note` text, and the
//! `concurrency` limit. The shims
//! resolve a `Task` handle to its bare id before yielding, so every task
//! field arrives as a path string; an id that does not parse is the
//! author's argument error, raised at the call site.

use std::time::Duration;

use mlua::Value;
use promptforge_types::ids::{TaskId, TaskOrigin};

use crate::Error;

use super::super::request::Request;
use super::{FieldFailure, call_string};

/// Parses one task id string; a malformed path is the call's error.
fn parse_task_id(text: &str) -> std::result::Result<TaskId, FieldFailure> {
    text.parse().map_err(|_| {
        FieldFailure::Call(Error::Lua(format!(
            "`{text}` is not a task id: required a dot-separated path such as `0.1`"
        )))
    })
}

/// Reads the author-supplied `task` id off the request table.
fn call_task(table: &mlua::Table) -> std::result::Result<TaskId, FieldFailure> {
    parse_task_id(&call_string(table, "task")?)
}

/// Parses a `timer` request: the author-supplied `seconds` (the wait's
/// `opts.timeout`). The value must be a number that `Duration` can hold -
/// non-negative, finite, and in range - so the scheduler's sleep never
/// meets a value it cannot represent; any other shape is the call's
/// error, raised at the wait's call site before a timer starts.
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
            return Err(FieldFailure::Call(Error::Lua(format!(
                "timeout must be a number, got {}",
                other.type_name()
            ))));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    if Duration::try_from_secs_f64(seconds).is_err() {
        return Err(FieldFailure::Call(Error::Lua(format!(
            "timeout must be a non-negative finite number of seconds, got {seconds}"
        ))));
    }
    Ok(Request::Timer { seconds })
}

/// Parses a `join_any` request: the shim-built `tasks` sequence of id
/// strings. The shim has already rejected an empty or non-table set, so a
/// missing or non-sequence field is a malformed yield; a member that is
/// not a valid id is the author's argument error.
pub(super) fn parse_join_any(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let Ok(Value::Table(set)) = table.raw_get::<Value>("tasks") else {
        return Err(FieldFailure::Malformed);
    };
    let mut tasks = Vec::new();
    for member in set.sequence_values::<Value>() {
        match member {
            Ok(Value::String(text)) => {
                let text = text.to_str().map_err(|_| FieldFailure::Malformed)?;
                tasks.push(parse_task_id(&text)?);
            }
            Ok(_) | Err(_) => return Err(FieldFailure::Malformed),
        }
    }
    if tasks.is_empty() {
        return Err(FieldFailure::Malformed);
    }
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

/// Parses a `task_events` request: the one task id and the optional
/// author-supplied `last` sequence number, which must be a non-negative
/// integer `u32` can hold when present; any other shape is the call's
/// error, raised at the call site.
pub(super) fn parse_task_events(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let task = call_task(table)?;
    let last = match table.raw_get::<Value>("last") {
        Ok(Value::Nil) => None,
        Ok(Value::Integer(last)) => Some(u32::try_from(last).map_err(|_| {
            FieldFailure::Call(Error::Lua(format!(
                "last must be a non-negative integer sequence number, got {last}"
            )))
        })?),
        Ok(Value::Number(last)) => {
            // A float with an integral value is the author writing `3.0`;
            // anything fractional, negative, or non-finite is no sequence
            // number. The range check happens in the integer conversion.
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the value is checked integral, finite, and in u32 range before the cast"
            )]
            let converted = (last.fract() == 0.0 && last >= 0.0 && last <= f64::from(u32::MAX))
                .then_some(last as u32);
            Some(converted.ok_or_else(|| {
                FieldFailure::Call(Error::Lua(format!(
                    "last must be a non-negative integer sequence number, got {last}"
                )))
            })?)
        }
        Ok(other) => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "last must be an integer, got {}",
                other.type_name()
            ))));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::TaskEvents { task, last })
}

/// Parses a `pending` request: the optional author-supplied `origin`
/// filter, which must name one of the two origins when present.
pub(super) fn parse_pending(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let origin = match table.raw_get::<Value>("origin") {
        Ok(Value::Nil) => None,
        Ok(Value::String(tag)) => {
            let tag = tag.to_str().map_err(|_| {
                FieldFailure::Call(Error::Lua(
                    "pending filter origin must be a valid UTF-8 string".to_owned(),
                ))
            })?;
            Some(TaskOrigin::from_tag(&tag).ok_or_else(|| {
                FieldFailure::Call(Error::Lua(format!(
                    "pending filter origin must be `author` or `model`, got `{}`",
                    &*tag
                )))
            })?)
        }
        Ok(other) => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "pending filter origin must be a string, got {}",
                other.type_name()
            ))));
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
        Ok(Value::Integer(limit)) => Some(
            u64::try_from(limit)
                .ok()
                .filter(|limit| *limit >= 1)
                .ok_or(FieldFailure::Malformed)?,
        ),
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
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "the value is checked integral, positive, and finite before the cast"
            )]
            // The ceiling is `u64::MAX` rounded up to 2^64, the nearest
            // f64 above it; the strict comparison keeps the cast below
            // the ceiling (the cast saturates at and past it).
            let converted = (limit < 18_446_744_073_709_551_616.0).then_some(limit as u64);
            Some(converted.ok_or_else(|| {
                FieldFailure::Call(Error::Lua(format!(
                    "concurrency limit must be a positive whole number in 64-bit range, got {limit}"
                )))
            })?)
        }
        Ok(_) | Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Concurrency { limit })
}
