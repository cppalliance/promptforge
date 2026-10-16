//! The yield-to-request validation: every field of a yielded table is
//! checked before use, a malformed yield fails the block with the fixed
//! direct-yield message, and an author-argument failure becomes the call's
//! answer so the shim raises it at the call site. The per-op parsers sit
//! in the siblings: `chat` (which owns the message-record validation),
//! `store`, `tasks`, and `tools`.

mod chat;
mod store;
mod tasks;
mod tools;

use mlua::{Lua, LuaSerdeExt, Value};
use runvil_model_client::model::ModelBinding;

use chat::parse_chat;
pub(crate) use chat::{NOT_A_LIST, parse_record};
use store::parse_store;
use tasks::{
    parse_cancel, parse_concurrency, parse_join_any, parse_note, parse_pending, parse_ready,
    parse_spawn, parse_status, parse_timer,
};
use tools::{parse_local_tool_done, parse_tool_call, parse_tool_calls};

use crate::error::{Error, Result};
use crate::handles::resolve_section_target;
use crate::models::LuaModelHandle;

use super::answer::Answer;
use super::request::Request;

/// The fixed failure for a yield that is not a well-formed request table.
///
/// The coroutine global is stripped from author reach, so the only yields in
/// a well-formed run are shim yields, which are well-formed by construction;
/// anything else is a hand-rolled or corrupted yield and fails the block as a
/// loud authoring error rather than confusing the driver.
const DIRECT_YIELD: &str = "scripts may not yield directly";

/// The fixed direct-yield failure.
fn direct_yield_error() -> Error {
    Error::Lua(DIRECT_YIELD.to_owned())
}

/// Fails the block with the fixed direct-yield message.
fn direct_yield<T>() -> Result<T> {
    Err(direct_yield_error())
}

/// Reads one field off the request table.
///
/// Reads are raw: the table comes from script space, so a metatable must not
/// intercept or forge a field.
fn raw_field(table: &mlua::Table, name: &str) -> Result<Value> {
    table.raw_get::<Value>(name).or_else(|_| direct_yield())
}

/// Reads a required plain-table field as its JSON snapshot.
fn json_field(lua: &Lua, table: &mlua::Table, name: &str) -> Result<serde_json::Value> {
    match raw_field(table, name)? {
        value @ Value::Table(_) => lua.from_value(value).or_else(|_| direct_yield()),
        _ => direct_yield(),
    }
}

/// How reading one request field failed.
enum FieldFailure {
    /// A shim-internal field was absent or unreadable: the shims set those
    /// fields by construction, so the yield is malformed.
    Malformed,
    /// An author-supplied argument had the wrong shape: the call's error,
    /// resumed as the answer so the shim raises it at the call site - an
    /// author `pcall` catches it.
    Call(Error),
}

/// Frames one author-argument failure as the call's error.
fn call_error(message: String) -> FieldFailure {
    FieldFailure::Call(Error::Lua(message))
}

/// Reads one author-supplied string value as UTF-8 text; invalid UTF-8 is
/// the call's error naming the argument.
fn call_text(value: &mlua::LuaString, name: &str) -> std::result::Result<String, FieldFailure> {
    match value.to_str() {
        Ok(text) => Ok(text.to_owned()),
        Err(_) => Err(call_error(format!("{name} must be a valid UTF-8 string"))),
    }
}

/// Reads one author-supplied required string argument. Every wrong shape,
/// absent included, is the call's error, raised at the call site.
fn call_string(table: &mlua::Table, name: &str) -> std::result::Result<String, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::String(value)) => call_text(&value, name),
        Ok(other) => Err(call_error(format!(
            "{name} must be a string, got {}",
            other.type_name()
        ))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads one author-supplied optional string argument: absent or nil is
/// `None`, any other wrong shape is the call's error.
fn call_optional_string(
    table: &mlua::Table,
    name: &str,
) -> std::result::Result<Option<String>, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::String(value)) => call_text(&value, name).map(Some),
        Ok(other) => Err(call_error(format!(
            "{name} must be a string, got {}",
            other.type_name()
        ))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads one shim-produced optional string field: absent or nil is `None`,
/// a string is `Some`, and any other shape is a malformed yield, since the
/// shims set the field by construction and no author argument reaches it.
fn shim_optional_string(
    table: &mlua::Table,
    name: &str,
) -> std::result::Result<Option<String>, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::String(value)) => match value.to_str() {
            Ok(text) => Ok(Some(text.to_owned())),
            Err(_) => Err(FieldFailure::Malformed),
        },
        Ok(_) | Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads one shim-produced optional integer field: absent or nil is
/// `None`, an integer within `u32` range is `Some`, and any other shape (a
/// float included) is a malformed yield, since the shims set the field by
/// construction and no author argument reaches it.
fn shim_optional_u32(
    table: &mlua::Table,
    name: &str,
) -> std::result::Result<Option<u32>, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::Integer(value)) => match u32::try_from(value) {
            Ok(value) => Ok(Some(value)),
            Err(_) => Err(FieldFailure::Malformed),
        },
        Ok(_) | Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Reads the shim-produced `var` snapshot; a failure is a malformed yield,
/// since the snapshot helper produces a plain JSON-representable table by
/// construction.
fn shim_var(
    lua: &Lua,
    table: &mlua::Table,
) -> std::result::Result<serde_json::Value, FieldFailure> {
    json_field(lua, table, "var").map_err(|_| FieldFailure::Malformed)
}

/// Reads the author-supplied `target` of a `call` or `spawn` with the
/// `resolve_section_target` rule, so a non-string target keeps its
/// byte-identical error.
fn call_target(table: &mlua::Table) -> std::result::Result<String, FieldFailure> {
    match table.raw_get::<Value>("target") {
        Ok(value) => {
            resolve_section_target(value).map_err(|error| FieldFailure::Call(Error::lua(error)))
        }
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// How one yielded value parsed at the resume boundary.
#[derive(Debug)]
pub enum YieldParse {
    /// A well-formed request, ready to dispatch.
    Request(Request),
    /// A well-formed shim call whose author-supplied argument failed
    /// validation: the call's answer, resumed into the caller so the shim
    /// raises the error at the call site.
    Call(Answer<Error>),
    /// Not a well-formed request table: a hand-rolled or corrupted yield,
    /// failing the block with the fixed direct-yield message.
    Malformed(Error),
}

impl Request {
    /// Validates a yielded value at the resume boundary.
    ///
    /// Every field is checked before use: the table comes from script space.
    /// A yield that is not a well-formed request table (not a table, no
    /// `op`, an unknown `op`, a shim-internal field of the wrong shape) is
    /// [`YieldParse::Malformed`] and fails the block with "scripts may not
    /// yield directly". A well-formed shim call whose author-supplied
    /// argument fails validation is [`YieldParse::Call`]: the error is
    /// returned as the call's answer so the shim raises it at the call site,
    /// where an author `pcall` can catch it. One boundary conversion keeps
    /// its own byte-identical error: a `call` or `spawn` target that is not
    /// a string fails as `resolve_section_target` fails.
    pub fn from_yield(lua: &Lua, yielded: &Value) -> YieldParse {
        let Value::Table(table) = yielded else {
            return YieldParse::Malformed(direct_yield_error());
        };
        let op = match raw_field(table, "op") {
            Ok(Value::String(op)) => match op.to_str() {
                Ok(op) => op.to_owned(),
                Err(_) => return YieldParse::Malformed(direct_yield_error()),
            },
            _ => return YieldParse::Malformed(direct_yield_error()),
        };
        match op.as_str() {
            "infer" => classify(parse_infer(table), |error| Answer::Infer(Err(error))),
            "call" => classify(parse_call(lua, table), |error| Answer::Call(Err(error))),
            "spawn" => classify(parse_spawn(lua, table), |error| Answer::Spawn(Err(error))),
            "timer" => classify(parse_timer(table), |error| Answer::Timer(Err(error))),
            "join_any" => classify(parse_join_any(table), |error| Answer::JoinAny(Err(error))),
            "ready" => classify(parse_ready(table), |error| Answer::Ready(Err(error))),
            "status" => classify(parse_status(table), |error| Answer::Status(Err(error))),
            "pending" => classify(parse_pending(table), |error| Answer::Pending(Err(error))),
            "note" => classify(parse_note(table), |error| Answer::Note(Err(error))),
            "concurrency" => classify(parse_concurrency(table), |error| {
                Answer::Concurrency(Err(error))
            }),
            "cancel" => classify(parse_cancel(table), |error| Answer::Cancel(Err(error))),
            "tool_call" => classify(parse_tool_call(lua, table), |error| {
                Answer::ToolCallResult(Err(error))
            }),
            "tool_calls" => classify(parse_tool_calls(lua, table), |error| {
                Answer::ToolCallsResult(Err(error))
            }),
            "local_tool_done" => match parse_local_tool_done(table) {
                Some(request) => YieldParse::Request(request),
                None => YieldParse::Malformed(direct_yield_error()),
            },
            "chat" => classify(parse_chat(table), |error| Answer::Chat(Err(error))),
            "store" => classify(parse_store(table), |error| Answer::Store(Err(error))),
            _ => YieldParse::Malformed(direct_yield_error()),
        }
    }
}

/// Maps one per-op parse to the boundary outcome: a validated request, an
/// author-argument failure as the call's answer, or a malformed yield.
fn classify(
    parsed: std::result::Result<Request, FieldFailure>,
    answer: impl FnOnce(Error) -> Answer<Error>,
) -> YieldParse {
    match parsed {
        Ok(request) => YieldParse::Request(request),
        Err(FieldFailure::Call(error)) => YieldParse::Call(answer(error)),
        Err(FieldFailure::Malformed) => YieldParse::Malformed(direct_yield_error()),
    }
}

/// Parses an `infer` request: the author-supplied `prompt`, and the
/// optional handle's userdata whose frozen [`ModelBinding`] is cloned out
/// of its borrow while the VM handle is live.
///
/// The handle is the receiver of `h:infer(prompt)`, so a wrong shape is
/// the call's error, not a malformed yield.
fn parse_infer(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let prompt = call_string(table, "prompt")?;
    let binding = call_handle(table, "models.infer")?;
    Ok(Request::Infer { prompt, binding })
}

/// Reads the optional model handle of `call` (`models.infer` or
/// `models.loop`) off the request's `handle` field, which a handle's
/// `infer` or `loop` sets from its receiver: absent or nil is `None`, a
/// model handle's userdata is its frozen [`ModelBinding`] cloned out of
/// its borrow while the VM handle is live, and any other value is the
/// call's error naming the call, since the receiver is author-supplied.
fn call_handle(
    table: &mlua::Table,
    call: &str,
) -> std::result::Result<Option<ModelBinding>, FieldFailure> {
    match table.raw_get::<Value>("handle") {
        Ok(Value::Nil) => Ok(None),
        Ok(Value::UserData(userdata)) => match userdata.borrow::<LuaModelHandle>() {
            Ok(handle) => Ok(Some(handle.binding().clone())),
            Err(_) => Err(call_error(format!("{call} handle must be a model handle"))),
        },
        Ok(other) => Err(call_error(format!(
            "{call} handle must be a model handle, got {}",
            other.type_name()
        ))),
        Err(_) => Err(FieldFailure::Malformed),
    }
}

/// Parses a `call` request: the author-supplied `target` (validated
/// with the `resolve_section_target` rule, keeping its byte-identical
/// error) and `input`, plus the shim-produced `var` snapshot.
fn parse_call(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let target = call_target(table)?;
    let input = call_optional_string(table, "input")?;
    let var = shim_var(lua, table)?;
    Ok(Request::Call { target, input, var })
}
