//! The structured error value every failure takes when it reaches Lua.
//!
//! A failure that crosses into author code - a shim's own argument error,
//! a Rust-raised error answered through the `(ok, result)` envelope, a
//! Engine function's own failure (`tools.offer`, `models.get`, a `sys` or `var`
//! guard) caught by `pcall`, or a shim raise such as
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

#[path = "error-value-classify.rs"]
mod classify;
#[path = "error-value-store.rs"]
mod store;

use std::collections::BTreeMap;
use std::str::FromStr;

use mlua::{Function, Lua, Table, Value};

use crate::compactors::OverflowReason;
use crate::error::{Error, accepted_tags};

pub(crate) use classify::{install_normalize_failure, normalized};
pub use store::store_error_value_fields;

/// The registry key of the shared error metatable, created on first use per
/// VM so every error table on that VM - Lua-built or Rust-built - shares
/// the same identity and the read-back can recognize it.
const METATABLE_REGISTRY: &str = "runvil.error_value.metatable";

/// The closed vocabulary of failure kinds an author can branch on.
///
/// The tag is the `kind` string the Lua table holds; the set is fixed by
/// the protocol and a new failure classifies into one of these rather than
/// inventing a tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorKind {
    /// The model tool loop ran its iteration cap without a final reply.
    ToolLoopExhausted,
    /// A request overflowed the model's context window and the selected
    /// compactor does not compact; includes `reason`.
    ContextExhausted,
    /// The model returned a turn with no product; includes `finish_reason`
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
    /// The run was cancelled.
    Cancelled,
    /// A store operation's own failure; includes `reason` and the
    /// [`VfsError`](runvil_vfs::VfsError) variant's fields.
    Store,
    /// A Lua authoring or runtime failure: a compile error, a runtime error
    /// in author code, a shim's argument error, or an exhausted Engine quota.
    Lua,
    /// An internal invariant was violated, or a failure outside the prompt the
    /// author cannot act on (transport, backend, store, configuration).
    Internal,
}

impl ErrorKind {
    /// Every kind, in declaration order.
    pub const ALL: &'static [ErrorKind] = &[
        ErrorKind::ToolLoopExhausted,
        ErrorKind::ContextExhausted,
        ErrorKind::EmptyModelReply,
        ErrorKind::OutOfScopeTool,
        ErrorKind::UnboundTool,
        ErrorKind::Tool,
        ErrorKind::TaskNotOwned,
        ErrorKind::TaskConsumed,
        ErrorKind::TasksLive,
        ErrorKind::Cancelled,
        ErrorKind::Store,
        ErrorKind::Lua,
        ErrorKind::Internal,
    ];

    /// Returns the `kind` string the Lua table holds.
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
            ErrorKind::Store => "store",
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
            "store" => Some(ErrorKind::Store),
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

impl FromStr for ErrorKind {
    type Err = ParseErrorKindError;

    /// Parses a kind from its exact tag, as [`ErrorKind::from_tag`] does.
    fn from_str(tag: &str) -> Result<ErrorKind, ParseErrorKindError> {
        ErrorKind::from_tag(tag).ok_or_else(|| ParseErrorKindError {
            input: tag.to_owned(),
        })
    }
}

/// The error returned when text is not an [`ErrorKind`] tag.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "invalid error kind `{input}`: required one of {}",
    accepted_tags(ErrorKind::ALL.iter().map(|kind| kind.tag()))
)]
pub struct ParseErrorKindError {
    /// The rejected text.
    input: String,
}

impl ParseErrorKindError {
    /// Returns the rejected text.
    #[must_use]
    pub fn input(&self) -> &str {
        &self.input
    }
}

/// One field of an error table: a string or an integer, so a field like
/// a store anchor's `count` stays numeric in Lua instead of being
/// stringified.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ErrorField {
    /// A string-valued field.
    String(String),
    /// An integer-valued field (a store anchor's `count`).
    Integer(i64),
}

impl ErrorField {
    /// The field's string value, when it is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            ErrorField::String(value) => Some(value),
            ErrorField::Integer(_) => None,
        }
    }

    /// The field's integer value, when it is one.
    #[must_use]
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            ErrorField::String(_) => None,
            ErrorField::Integer(value) => Some(*value),
        }
    }
}

/// A Rust error's rendering into the Lua error table: its kind and the
/// kind's own fields. `Display` supplies `message`.
///
/// The envelope renderer requires this of the driver's error type, so a
/// failure answered to Lua always has a kind; an internal type that gains a
/// variant classifies it here.
pub trait ErrorValue: std::fmt::Display {
    /// The kind the table's `kind` field names.
    fn kind(&self) -> ErrorKind;

    /// The kind's own fields, as `(name, value)` pairs set beside
    /// `kind` and `message`. Empty for kinds without fields.
    fn fields(&self) -> Vec<(String, ErrorField)> {
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
            Error::Store { .. } => ErrorKind::Store,
            Error::Internal(_) => ErrorKind::Internal,
            Error::Raised(raised) => raised.kind,
        }
    }

    fn fields(&self) -> Vec<(String, ErrorField)> {
        match self {
            Error::ContextExhausted { reason } => {
                vec![(
                    "reason".to_owned(),
                    ErrorField::String(reason.tag().to_owned()),
                )]
            }
            Error::Store { cause, .. } => store_error_value_fields(cause),
            Error::Raised(raised) => raised
                .fields
                .iter()
                .map(|(name, value)| (name.clone(), value.clone()))
                .collect(),
            Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::LuaQuota { .. }
            | Error::Interrupted
            | Error::Tool { .. }
            | Error::Internal(_) => Vec::new(),
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
/// internal type by kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raised {
    /// The kind the table named.
    pub kind: ErrorKind,
    /// The message `tostring` renders.
    pub message: String,
    /// The kind's own fields (`reason`, `finish_reason`, ...), string- or
    /// integer-valued (a store anchor's `count`).
    pub fields: BTreeMap<String, ErrorField>,
}

impl Raised {
    /// The overflow reason a `context_exhausted` table held, when it
    /// parses.
    #[must_use]
    pub fn overflow_reason(&self) -> Option<OverflowReason> {
        self.fields
            .get("reason")
            .and_then(ErrorField::as_str)
            .and_then(OverflowReason::from_tag)
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
pub fn error_table(lua: &Lua, error: &impl ErrorValue) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.raw_set("message", error.to_string())?;
    for (name, value) in error.fields() {
        let value = match value {
            ErrorField::String(value) => Value::String(lua.create_string(value)?),
            ErrorField::Integer(value) => Value::Integer(value),
        };
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
        let kind = tag.parse::<ErrorKind>().map_err(|error| {
            mlua::Error::external(Error::Internal(format!("the shim raised {error}").into()))
        })?;
        let fields = match fields {
            Some(fields) => fields,
            None => lua.create_table()?,
        };
        finish_table(lua, kind, fields)
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
    // `pairs` order is unspecified, but `fields` is a `BTreeMap`, so the
    // raised value's field order never depends on it.
    let mut fields = BTreeMap::new();
    for pair in table.pairs::<String, Value>() {
        let (name, value) = pair?;
        if name == "kind" || name == "message" {
            continue;
        }
        let field = match value {
            Value::String(value) => ErrorField::String(value.to_str()?.to_owned()),
            Value::Integer(value) => ErrorField::Integer(value),
            _ => continue,
        };
        fields.insert(name, field);
    }
    Ok(Some(Raised {
        kind,
        message,
        fields,
    }))
}

#[cfg(test)]
#[path = "error-value-tests.rs"]
mod tests;
