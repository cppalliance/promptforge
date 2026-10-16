//! The classifier a Rust callback's failure runs through on its way to
//! author code, and the `normalize_failure` capture built on it.

use mlua::{Function, Lua, Value};

use super::{ErrorField, ErrorKind, ErrorValue, error_table};
use crate::error::Error;

/// A Rust callback's failure classified for the error table: the kind, the
/// message `tostring` rendered before (the root cause's display, without
/// the traceback mlua appends), and the kind's fields.
///
/// mlua raises a callback's `Err` into Lua as an opaque userdata whose
/// `tostring` is the error's display; author code cannot index it, so
/// `err.kind` fails at exactly the call sites that fail directly from
/// Rust. The classifier reads the typed [`Error`] back out of the mlua
/// wrapper when the callback raised one, and otherwise sorts mlua's own
/// variants: an authoring or argument failure (a runtime or syntax error,
/// a bad argument, a value conversion, an external error of another type,
/// an exhausted memory quota) is `lua`; anything else is mlua's own
/// machinery failing, which the author cannot act on, so it is `internal`.
struct Classified {
    kind: ErrorKind,
    message: String,
    fields: Vec<(String, ErrorField)>,
}

impl Classified {
    fn from_mlua(error: &mlua::Error) -> Classified {
        if let Some(typed) = error.downcast_ref::<Error>() {
            return Classified {
                kind: typed.kind(),
                message: typed.to_string(),
                fields: typed.fields(),
            };
        }
        let root = root_cause(error);
        let kind = match root {
            mlua::Error::RuntimeError(_)
            | mlua::Error::SyntaxError { .. }
            | mlua::Error::MemoryError(_)
            | mlua::Error::BadArgument { .. }
            | mlua::Error::FromLuaConversionError { .. }
            | mlua::Error::SerializeError(_)
            | mlua::Error::DeserializeError(_)
            | mlua::Error::ExternalError(_) => ErrorKind::Lua,
            _ => ErrorKind::Internal,
        };
        Classified {
            kind,
            message: root.to_string(),
            fields: Vec::new(),
        }
    }
}

/// Strips mlua's wrapping layers (the callback frame and any context) down
/// to the error a callback returned.
fn root_cause(error: &mlua::Error) -> &mlua::Error {
    match error {
        mlua::Error::CallbackError { cause, .. } | mlua::Error::WithContext { cause, .. } => {
            root_cause(cause)
        }
        other => other,
    }
}

impl std::fmt::Display for Classified {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl ErrorValue for Classified {
    fn kind(&self) -> ErrorKind {
        self.kind
    }

    fn fields(&self) -> Vec<(String, ErrorField)> {
        self.fields.clone()
    }
}

/// Builds the `normalize_failure(value)` chunk capture for the shim's
/// `pcall` and `xpcall` replacements: a Rust callback's failure (mlua's
/// wrapped error) becomes the error table its classification names; any
/// other caught value - a string, an author's own table, an error table
/// already built - passes through unchanged.
///
/// # Errors
/// Returns an `mlua` error if the function cannot be created.
pub(crate) fn install_normalize_failure(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(normalized)
}

/// Returns what `normalize_failure` returns for `value`: a Rust callback's
/// failure as the error table its classification names, anything else
/// unchanged, for the capture and the loop's compactor rule alike.
pub(crate) fn normalized(lua: &Lua, value: Value) -> mlua::Result<Value> {
    match value {
        Value::Error(error) => Ok(Value::Table(error_table(
            lua,
            &Classified::from_mlua(&error),
        )?)),
        other => Ok(other),
    }
}
