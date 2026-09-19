//! The yield-to-request validation: every field of a yielded table is
//! checked before use, a malformed yield fails the block with the fixed
//! direct-yield message, and an author-argument failure becomes the call's
//! answer so the shim raises it at the call site. The chat and loop
//! request parsers, which share the message-record validation, sit in the
//! `chat` sibling.

#[path = "parse-chat.rs"]
mod chat;

use mlua::{Lua, LuaSerdeExt, Value};

use chat::{parse_chat, parse_loop};

use crate::tools::tool_alias;
use crate::{Error, LuaModelHandle, Result, resolve_section_target};

use super::answer::Answer;
use super::request::{Request, StoreOp};

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
    /// author `pcall` catches it, exactly as the legacy callback's argument
    /// error surfaced.
    Call(Error),
}

/// Reads one author-supplied required string argument. Every wrong shape,
/// absent included, is the call's error: the legacy callback's argument
/// conversion failed at the call site too.
fn call_string(table: &mlua::Table, name: &str) -> std::result::Result<String, FieldFailure> {
    match table.raw_get::<Value>(name) {
        Ok(Value::String(value)) => value.to_str().map(|value| value.to_owned()).map_err(|_| {
            FieldFailure::Call(Error::Lua(format!("{name} must be a valid UTF-8 string")))
        }),
        Ok(other) => Err(FieldFailure::Call(Error::Lua(format!(
            "{name} must be a string, got {}",
            other.type_name()
        )))),
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
        Ok(Value::String(value)) => {
            value
                .to_str()
                .map(|value| Some(value.to_owned()))
                .map_err(|_| {
                    FieldFailure::Call(Error::Lua(format!("{name} must be a valid UTF-8 string")))
                })
        }
        Ok(other) => Err(FieldFailure::Call(Error::Lua(format!(
            "{name} must be a string, got {}",
            other.type_name()
        )))),
        Err(_) => Err(FieldFailure::Malformed),
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

/// How one yielded value parsed at the resume boundary.
#[derive(Debug)]
pub enum YieldParse {
    /// A well-formed request, ready to dispatch.
    Request(Request),
    /// A well-formed shim call whose author-supplied argument failed
    /// validation: the call's answer, resumed into the caller so the shim
    /// raises the error at the call site, exactly as the legacy callback's
    /// argument error surfaced.
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
    /// argument fails validation is [`YieldParse::Call`]: the error rides
    /// back as the call's answer so the shim raises it at the call site,
    /// keeping the legacy callback's errors catchable by an author `pcall`.
    /// Two boundary conversions keep their own byte-identical errors: a
    /// `call` target that is not a string fails as
    /// `resolve_section_target` fails, and a fanout collection fails as
    /// `collection_to_items` fails.
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
            "fanout" => classify(parse_fanout(lua, table), |error| Answer::Fanout(Err(error))),
            "tool_call" => classify(parse_tool_call(lua, table), |error| {
                Answer::ToolCallResult(Err(error))
            }),
            "chat" => classify(parse_chat(lua, table), |error| Answer::Chat(Err(error))),
            "loop" => classify(parse_loop(lua, table), |error| Answer::Loop(Err(error))),
            // No author arguments exist to fail validation: a well-formed
            // `user_input` yield is always the unit request.
            "user_input" => YieldParse::Request(Request::UserInput),
            "store" => classify(parse_store(table), |error| Answer::Store(Err(error))),
            "mcp" => match parse_mcp(lua, table) {
                Ok(request) => YieldParse::Request(request),
                Err(_) => YieldParse::Malformed(direct_yield_error()),
            },
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
/// optional leading handle's userdata whose frozen [`ModelBinding`] is
/// cloned out of its borrow while the VM handle is live.
///
/// The handle is author-supplied under namespace-only invocation
/// (`models.infer(handle?, prompt)`), so a wrong shape is the call's error,
/// not a malformed yield.
fn parse_infer(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let prompt = call_string(table, "prompt")?;
    let binding = match table.raw_get::<Value>("handle") {
        Ok(Value::Nil) => None,
        Ok(Value::UserData(userdata)) => match userdata.borrow::<LuaModelHandle>() {
            Ok(handle) => Some(handle.binding().clone()),
            Err(_) => {
                return Err(FieldFailure::Call(Error::Lua(
                    "models.infer handle must be a model handle".to_owned(),
                )));
            }
        },
        Ok(other) => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "models.infer handle must be a model handle, got {}",
                other.type_name()
            ))));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Infer { prompt, binding })
}

/// Parses a `call` request: the author-supplied `target` (validated
/// with the `resolve_section_target` rule, keeping its byte-identical
/// error) and `input`, plus the shim-produced `var` snapshot.
fn parse_call(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let target = match table.raw_get::<Value>("target") {
        Ok(value) => {
            resolve_section_target(value).map_err(|error| FieldFailure::Call(Error::lua(error)))?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let input = call_optional_string(table, "input")?;
    let var = shim_var(lua, table)?;
    Ok(Request::Call { target, input, var })
}

/// Parses a `fanout` request: the author-supplied `worker` heading and
/// `collection` (converted member-wise while the VM handle is live, keeping
/// the conversion's byte-identical errors), plus the shim-produced `var`
/// snapshot.
fn parse_fanout(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let worker = call_string(table, "worker")?;
    let items = match table.raw_get::<Value>("collection") {
        Ok(collection) => {
            crate::collection::collection_to_items(lua, &collection).map_err(FieldFailure::Call)?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let var = shim_var(lua, table)?;
    Ok(Request::Fanout { worker, items, var })
}

/// Parses a `tools.call` request: the author-supplied `alias` (a string or
/// a Tool object, decoded through the one alias-or-Tool polymorphism) and
/// `args`.
///
/// An absent or nil `args` parses as the empty object (the empty-argument
/// call every tool accepts). A non-table or JSON-unrepresentable `args` is
/// the call's error, framed exactly as the other author-argument failures,
/// so an author `pcall` catches it at the call site.
fn parse_tool_call(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let alias = match table.raw_get::<Value>("alias") {
        // Flatten to the call-error string so the answer frames exactly as
        // the other author-argument failures (`Error::Lua`, not a runtime
        // wrapper).
        Ok(value) => {
            tool_alias(&value).map_err(|error| FieldFailure::Call(Error::Lua(error.to_string())))?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let args = match table.raw_get::<Value>("args") {
        Ok(Value::Nil) => serde_json::Value::Object(serde_json::Map::new()),
        Ok(Value::Table(_)) => json_field(lua, table, "args").map_err(|_| {
            FieldFailure::Call(Error::Lua(
                "args must be a JSON-representable table".to_owned(),
            ))
        })?,
        Ok(other) => {
            return Err(FieldFailure::Call(Error::Lua(format!(
                "args must be a table, got {}",
                other.type_name()
            ))));
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    Ok(Request::ToolCall { alias, args })
}

/// Reads one author-supplied optional line bound: absent or nil is `None`,
/// an integer (or a float with an integral value, matching the legacy
/// callback's `i64` conversion) is `Some`, any other shape is the call's
/// error.
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
/// so the shim raises it at the call site - an author `pcall` catches it,
/// exactly as the legacy callback's argument conversion failed there.
fn parse_store(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
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

/// Parses a reserved `mcp` request. No call surface produces one, so every
/// field is shim-internal by construction.
fn parse_mcp(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let server = call_string(table, "server")?;
    let tool = call_string(table, "tool")?;
    let args = json_field(lua, table, "args").map_err(|_| FieldFailure::Malformed)?;
    Ok(Request::Mcp { server, tool, args })
}
