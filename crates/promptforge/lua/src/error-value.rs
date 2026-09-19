//! The structured error value every failure takes when it reaches Lua.
//!
//! A failure that crosses into author code - a shim's own argument error,
//! a Rust-raised error answered through the `(ok, result)` envelope, a
//! host callback's own failure (`tools.add`, `models.get`, a `sys` or `var`
//! guard) caught by `pcall`, or a shim raise such as a future
//! `tool_loop_exhausted` - is one Lua table `{ kind, message, ... }` under
//! a shared metatable whose `__tostring` returns `message`. A `pcall`
//! caller that prints the error sees exactly the text it saw before; a
//! caller that branches reads `kind` and the kind's own fields (`reason`
//! for `context_exhausted`, `finish_reason` for `empty_model_reply`).
//!
//! [`ErrorKind`] names the closed vocabulary of kinds, [`ErrorValue`] is
//! what a Rust error implements to render itself into the shape,
//! [`install_normalize_failure`] is the capture the shim's `pcall` and
//! `xpcall` replacements run a caught value through (so a Rust callback's
//! failure, which mlua raises as an opaque userdata, takes the same shape),
//! and [`Raised`] is the table read back into Rust when a shim raise
//! surfaces as a block coroutine's failure, so the kind survives the
//! boundary in both directions.

use std::collections::BTreeMap;

use mlua::{Function, Lua, Table, Value};

use crate::compactors::OverflowReason;
use crate::error::Error;

/// The registry key of the shared error metatable, created on first use per
/// VM so every error table on that VM - Lua-built or Rust-built - carries
/// the same identity and the read-back can recognize it.
const METATABLE_REGISTRY: &str = "promptforge.error_value.metatable";

/// The closed vocabulary of failure kinds an author can branch on.
///
/// The tag is the `kind` string the Lua table carries; the set is fixed by
/// the protocol and a new failure classifies into one of these rather than
/// inventing a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[doc(hidden)]
pub enum ErrorKind {
    /// The model tool loop ran its iteration cap without a final reply.
    ToolLoopExhausted,
    /// A request overflowed the model's context window and the selected
    /// compactor does not compact; carries `reason`.
    ContextExhausted,
    /// The model returned a turn with no product; carries `finish_reason`
    /// when the backend supplied one.
    EmptyModelReply,
    /// The model named a tool outside the section's advertised scope.
    OutOfScopeTool,
    /// A script `tools.call` named an alias with no binding in the run.
    UnboundTool,
    /// A dispatched tool's own failure.
    Tool,
    /// A task operation named a task the caller does not own.
    TaskNotOwned,
    /// A task's result was already delivered once.
    TaskConsumed,
    /// A section ended while author-origin tasks it owns were still live.
    TasksLive,
    /// The host cancelled the run.
    Cancelled,
    /// A Lua authoring or runtime failure: a compile error, a runtime error
    /// in author code, a shim's argument error, or an exhausted host quota.
    Lua,
    /// An internal invariant was violated, or a host-side failure the
    /// author cannot act on (transport, backend, store, configuration).
    Internal,
}

impl ErrorKind {
    /// The `kind` string the Lua table carries.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            ErrorKind::ToolLoopExhausted => "tool_loop_exhausted",
            ErrorKind::ContextExhausted => "context_exhausted",
            ErrorKind::EmptyModelReply => "empty_model_reply",
            ErrorKind::OutOfScopeTool => "out_of_scope_tool",
            ErrorKind::UnboundTool => "unbound_tool",
            ErrorKind::Tool => "tool",
            ErrorKind::TaskNotOwned => "task_not_owned",
            ErrorKind::TaskConsumed => "task_consumed",
            ErrorKind::TasksLive => "tasks_live",
            ErrorKind::Cancelled => "cancelled",
            ErrorKind::Lua => "lua",
            ErrorKind::Internal => "internal",
        }
    }

    /// Parses a `kind` string; `None` for a tag outside the vocabulary.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<ErrorKind> {
        match tag {
            "tool_loop_exhausted" => Some(ErrorKind::ToolLoopExhausted),
            "context_exhausted" => Some(ErrorKind::ContextExhausted),
            "empty_model_reply" => Some(ErrorKind::EmptyModelReply),
            "out_of_scope_tool" => Some(ErrorKind::OutOfScopeTool),
            "unbound_tool" => Some(ErrorKind::UnboundTool),
            "tool" => Some(ErrorKind::Tool),
            "task_not_owned" => Some(ErrorKind::TaskNotOwned),
            "task_consumed" => Some(ErrorKind::TaskConsumed),
            "tasks_live" => Some(ErrorKind::TasksLive),
            "cancelled" => Some(ErrorKind::Cancelled),
            "lua" => Some(ErrorKind::Lua),
            "internal" => Some(ErrorKind::Internal),
            _ => None,
        }
    }
}

impl std::fmt::Display for ErrorKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.tag())
    }
}

/// A Rust error's rendering into the Lua error table: its kind and the
/// kind's own string fields. `Display` supplies `message`.
///
/// The envelope renderer requires this of the driver's error type, so a
/// failure answered to Lua always carries a kind; a substrate that gains a
/// variant classifies it here.
#[doc(hidden)]
pub trait ErrorValue: std::fmt::Display {
    /// The kind the table's `kind` field names.
    fn kind(&self) -> ErrorKind;

    /// The kind's own fields, as `(name, value)` string pairs set beside
    /// `kind` and `message`. Empty for kinds without fields.
    fn fields(&self) -> Vec<(String, String)> {
        Vec::new()
    }
}

impl ErrorValue for Error {
    fn kind(&self) -> ErrorKind {
        match self {
            Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::LuaQuota { .. } => ErrorKind::Lua,
            Error::ContextExhausted { .. } => ErrorKind::ContextExhausted,
            Error::Interrupted => ErrorKind::Cancelled,
            Error::Tool { .. } => ErrorKind::Tool,
            Error::Internal(_) => ErrorKind::Internal,
            Error::Raised(raised) => raised.kind,
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        match self {
            Error::ContextExhausted { reason } => {
                vec![("reason".to_owned(), reason.tag().to_owned())]
            }
            Error::Raised(raised) => raised
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            _ => Vec::new(),
        }
    }
}

/// A structured error table read back into Rust: the kind, the message
/// `tostring` rendered, and the kind's string fields.
///
/// This is the shape a block coroutine's failure takes when the raised
/// value was an error table (built by the shim's `raise` or by
/// [`error_table`]) and no retained typed error was substituted for it -
/// the case for a Lua-side raise. The executor maps it back onto its own
/// substrate by kind.
#[derive(Debug, Clone, PartialEq, Eq)]
#[doc(hidden)]
pub struct Raised {
    /// The kind the table named.
    pub kind: ErrorKind,
    /// The message `tostring` renders.
    pub message: String,
    /// The kind's own fields (`reason`, `finish_reason`, ...), string-valued.
    pub fields: BTreeMap<String, String>,
}

impl Raised {
    /// The overflow reason a `context_exhausted` table carried, when it
    /// parses.
    #[must_use]
    pub fn overflow_reason(&self) -> Option<OverflowReason> {
        self.fields
            .get("reason")
            .and_then(|tag| OverflowReason::from_tag(tag))
    }
}

impl std::fmt::Display for Raised {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for Raised {}

/// Returns the VM's shared error metatable, creating it on first use.
///
/// `__tostring` returns the table's `message`, so an author's `tostring`
/// (and mlua's own stringification when the table surfaces as a coroutine
/// failure) renders exactly the message text; `__concat` renders the same
/// way, so `'prefix: ' .. err` keeps working where the error was a string.
fn metatable(lua: &Lua) -> mlua::Result<Table> {
    if let Value::Table(existing) = lua.named_registry_value::<Value>(METATABLE_REGISTRY)? {
        return Ok(existing);
    }
    let metatable = lua.create_table()?;
    let tostring = lua.create_function(|_lua, table: Table| table.raw_get::<Value>("message"))?;
    metatable.raw_set("__tostring", tostring)?;
    let concat = lua
        .load("local tostring = tostring; return function(a, b) return tostring(a) .. tostring(b) end")
        .eval::<Function>()?;
    metatable.raw_set("__concat", concat)?;
    lua.set_named_registry_value(METATABLE_REGISTRY, &metatable)?;
    Ok(metatable)
}

/// Sets `kind` on `fields`, fills a missing `message` from the kind, and
/// attaches the shared metatable.
fn finish_table(lua: &Lua, kind: ErrorKind, fields: Table) -> mlua::Result<Table> {
    fields.raw_set("kind", kind.tag())?;
    if matches!(fields.raw_get::<Value>("message")?, Value::Nil) {
        fields.raw_set("message", kind.tag())?;
    }
    fields.set_metatable(Some(metatable(lua)?))?;
    Ok(fields)
}

/// Renders a Rust error as the Lua error table: `kind`, `message` (its
/// display), and the kind's fields, under the shared metatable.
///
/// # Errors
/// Returns an `mlua` error if the table cannot be created on `lua`.
#[doc(hidden)]
pub fn error_table(lua: &Lua, error: &impl ErrorValue) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("message", error.to_string())?;
    for (name, value) in error.fields() {
        table.raw_set(name, value)?;
    }
    finish_table(lua, error.kind(), table)
}

/// Builds the `error_value(kind, fields)` chunk capture for the shim: the
/// Lua-side constructor of the same table shape, so a shim raise and a
/// Rust-raised error are indistinguishable to author code. An unknown kind
/// is a shim bug and fails the call.
///
/// # Errors
/// Returns an `mlua` error if the function cannot be created.
pub(crate) fn install_error_value(lua: &Lua) -> mlua::Result<Function> {
    lua.create_function(|lua, (tag, fields): (String, Option<Table>)| {
        let kind = ErrorKind::from_tag(&tag).ok_or_else(|| {
            mlua::Error::external(Error::Lua(format!(
                "unknown error kind {tag:?}; expected one of the protocol's kinds"
            )))
        })?;
        let fields = match fields {
            Some(fields) => fields,
            None => lua.create_table()?,
        };
        finish_table(lua, kind, fields)
    })
}

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
    fields: Vec<(String, String)>,
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

    fn fields(&self) -> Vec<(String, String)> {
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
    lua.create_function(|lua, value: Value| match value {
        Value::Error(error) => Ok(Value::Table(error_table(
            lua,
            &Classified::from_mlua(&error),
        )?)),
        other => Ok(other),
    })
}

/// Reads a raised Lua value back as a [`Raised`] when it is an error table
/// built on this VM (recognized by the shared metatable, so an author's own
/// table with a `kind` field is not mistaken for one). Any other value is
/// `None`.
///
/// # Errors
/// Returns an `mlua` error if the table's fields cannot be read.
pub(crate) fn raised_from(lua: &Lua, value: &Value) -> mlua::Result<Option<Raised>> {
    let Value::Table(table) = value else {
        return Ok(None);
    };
    let Some(attached) = table.metatable() else {
        return Ok(None);
    };
    if attached.to_pointer() != metatable(lua)?.to_pointer() {
        return Ok(None);
    }
    let Value::String(tag) = table.raw_get::<Value>("kind")? else {
        return Ok(None);
    };
    let Some(kind) = ErrorKind::from_tag(&tag.to_str()?) else {
        return Ok(None);
    };
    let message = match table.raw_get::<Value>("message")? {
        Value::String(message) => message.to_str()?.to_owned(),
        _ => kind.tag().to_owned(),
    };
    let mut fields = BTreeMap::new();
    for pair in table.pairs::<String, Value>() {
        let (name, value) = pair?;
        if name == "kind" || name == "message" {
            continue;
        }
        if let Value::String(value) = value {
            fields.insert(name, value.to_str()?.to_owned());
        }
    }
    Ok(Some(Raised {
        kind,
        message,
        fields,
    }))
}
