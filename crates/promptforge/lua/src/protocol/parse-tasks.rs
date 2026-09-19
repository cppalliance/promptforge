//! The task-operation request parsers: the `when_any` set, the single-task
//! `ready`, `status`, and `cancel`, the `pending` origin filter, and the
//! `note` text. The shims resolve a `Task` handle to its bare id before
//! yielding, so every task field arrives as a path string; an id that does
//! not parse is the author's argument error, raised at the call site.

use mlua::Value;
use promptforge_api_types::ids::{TaskId, TaskOrigin};

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

/// Parses a `when_any` request: the shim-built `tasks` sequence of id
/// strings. The shim has already rejected an empty or non-table set, so a
/// missing or non-sequence field is a malformed yield; a member that is
/// not a valid id is the author's argument error.
pub(super) fn parse_when_any(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
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
    Ok(Request::WhenAny { tasks })
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
