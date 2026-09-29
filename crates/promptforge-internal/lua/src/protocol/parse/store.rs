//! The `store` request parser: the operation name and its author-supplied
//! arguments.

use mlua::Value;

use crate::Error;

use super::super::request::{Request, StoreOp};
use super::{FieldFailure, call_string};

/// Reads one author-supplied optional line bound: absent or nil is `None`,
/// an integer (or a float with an integral value, as Lua's own integer
/// conversion accepts) is `Some`, any other shape is the call's error.
fn call_optional_line(
    table: &mlua::Table,
    name: &str,
) -> std::result::Result<Option<i64>, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::Integer(line)) => Ok(Some(line)),
        // The bounds are exact powers of two (-2^63 and 2^63), so the
        // range check needs no lossy i64-to-f64 cast.
        Ok(Value::Number(line))
            if line.fract() == 0.0
                && (-9_223_372_036_854_775_808.0..9_223_372_036_854_775_808.0).contains(&line) =>
        {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the range check above bounds the value to i64"
            )]
            Ok(Some(line as i64))
        }
        Ok(other) => Err(FieldFailure::Call(Error::Lua(format!(
            "{name} must be an integer, got {}",
            other.type_name()
        )))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Parses a `store` request: the operation name and its author-supplied
/// arguments. Every wrong shape is the call's error, resumed as the answer
/// so the shim raises it at the call site - an author `pcall` catches it.
pub(super) fn parse_store(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let op = call_string(table, "store_op")?;
    let op = match op.as_str() {
        "write" => StoreOp::Write {
            path: call_string(table, "path")?,
            contents: call_string(table, "contents")?,
        },
        "append" => StoreOp::Append {
            path: call_string(table, "path")?,
            contents: call_string(table, "contents")?,
        },
        "read" => StoreOp::Read {
            path: call_string(table, "path")?,
            start: call_optional_line(table, "start")?,
            end: call_optional_line(table, "end")?,
        },
        "read_numbered" => StoreOp::ReadNumbered {
            path: call_string(table, "path")?,
            start: call_optional_line(table, "start")?,
            end: call_optional_line(table, "end")?,
        },
        "str_replace" => StoreOp::StrReplace {
            path: call_string(table, "path")?,
            old: call_string(table, "old")?,
            new: call_string(table, "new")?,
        },
        "delete" => StoreOp::Delete {
            path: call_string(table, "path")?,
        },
        "glob" => StoreOp::Glob {
            pattern: call_string(table, "pattern")?,
        },
        "exists" => StoreOp::Exists {
            path: call_string(table, "path")?,
        },
        other => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "unknown store operation {other:?}"
            ))));
        }
    };
    Ok(Request::Store { op })
}
