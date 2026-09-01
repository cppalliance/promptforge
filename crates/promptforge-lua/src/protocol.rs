//! The coroutine protocol: validated request and answer types for the
//! yield/resume boundary between section Lua and the scheduler driver.
//!
//! A suspending host call (`models.infer`, `handle:infer`, `execute`,
//! `fanout`) is a Lua-side shim that yields a request table; the driver
//! validates the yield into a [`Request`], dispatches it, and resumes the
//! coroutine with the `(ok, result)` envelope rendered from an [`Answer`].
//! The two enums are the audit surface: what a script can cause the host to
//! do is one short read, and each variant's fields are the compiler-checked
//! per-message contract.

use mlua::{Lua, LuaSerdeExt, MultiValue, Value};

use promptforge_model_client::model::ModelBinding;

use crate::{
    Error, LuaFanoutResult, LuaModelHandle, Result, pack_sequence, resolve_section_target,
};

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

/// A validated suspending host call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: the
/// fanout collection converts through the existing member-wise rules and the
/// handle userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
/// lifetime-bound enters the enum.
#[derive(Debug)]
pub enum Request {
    /// `models.infer` (`binding: None`: resolve the section's current model)
    /// or `handle:infer` (`binding: Some`: the handle's frozen binding).
    Infer {
        /// The author-supplied prompt text.
        prompt: String,
        /// The handle's frozen binding for `handle:infer`, else `None`.
        binding: Option<ModelBinding>,
    },
    /// `execute(target, input?)`: run a contained chain over the target's
    /// slice.
    Execute {
        /// The heading string, validated with the `resolve_section_target`
        /// rule so a non-string target keeps its byte-identical error.
        target: String,
        /// The optional input override; `None` runs under the run's own args.
        input: Option<String>,
        /// The caller's `var` snapshot, seeded into the chain and discarded
        /// when it ends.
        var: serde_json::Value,
    },
    /// `fanout(worker, collection)`: the collection already converted
    /// member-wise through the existing rules.
    Fanout {
        /// The worker heading string, resolved by the driver against the
        /// caller's visible set.
        worker: String,
        /// The converted collection members: the array part in order, then
        /// the hash part as `{"key", "value"}` pairs.
        items: Vec<serde_json::Value>,
        /// The caller's `var` snapshot; each arm seeds from its own clone.
        var: serde_json::Value,
    },
    /// Reserved. Never dispatched: receiving one is a typed protocol error.
    // The fields are read only by this module's own tests; production parses
    // them for strict validation and never reads them until the variant
    // gains a dispatch.
    #[allow(dead_code)]
    Mcp {
        /// The reserved server name.
        server: String,
        /// The reserved tool name.
        tool: String,
        /// The reserved argument payload.
        args: serde_json::Value,
    },
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
    /// Two boundary conversions keep their own byte-identical errors: an
    /// `execute` target that is not a string fails as
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
            "execute" => classify(parse_execute(lua, table), |error| {
                Answer::Execute(Err(error))
            }),
            "fanout" => classify(parse_fanout(lua, table), |error| Answer::Fanout(Err(error))),
            "mcp" => match parse_mcp(lua, table) {
                Ok(request) => YieldParse::Request(request),
                Err(_) => YieldParse::Malformed(direct_yield_error()),
            },
            _ => YieldParse::Malformed(direct_yield_error()),
        }
    }

    /// The typed protocol error for a received `mcp` request.
    ///
    /// The `mcp` fields are reserved and no call surface produces the request
    /// yet, so the driver never dispatches one; receiving it fails the chain
    /// with this error rather than reaching an unimplemented path.
    #[must_use]
    pub fn mcp_reserved() -> Error {
        Error::Lua("mcp requests are reserved: no dispatcher exists yet".to_owned())
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
/// shim-produced `handle` userdata whose frozen [`ModelBinding`] is cloned
/// out of its borrow while the VM handle is live.
fn parse_infer(table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let prompt = call_string(table, "prompt")?;
    let binding = match table.raw_get::<Value>("handle") {
        Ok(Value::Nil) => None,
        Ok(Value::UserData(userdata)) => match userdata.borrow::<LuaModelHandle>() {
            Ok(handle) => Some(handle.binding().clone()),
            Err(_) => return Err(FieldFailure::Malformed),
        },
        _ => return Err(FieldFailure::Malformed),
    };
    Ok(Request::Infer { prompt, binding })
}

/// Parses an `execute` request: the author-supplied `target` (validated
/// with the `resolve_section_target` rule, keeping its byte-identical
/// error) and `input`, plus the shim-produced `var` snapshot.
fn parse_execute(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let target = match table.raw_get::<Value>("target") {
        Ok(value) => {
            resolve_section_target(value).map_err(|error| FieldFailure::Call(Error::lua(error)))?
        }
        Err(_) => return Err(FieldFailure::Malformed),
    };
    let input = call_optional_string(table, "input")?;
    let var = shim_var(lua, table)?;
    Ok(Request::Execute { target, input, var })
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

/// Parses a reserved `mcp` request. No call surface produces one, so every
/// field is shim-internal by construction.
fn parse_mcp(lua: &Lua, table: &mlua::Table) -> std::result::Result<Request, FieldFailure> {
    let server = call_string(table, "server")?;
    let tool = call_string(table, "tool")?;
    let args = json_field(lua, table, "args").map_err(|_| FieldFailure::Malformed)?;
    Ok(Request::Mcp { server, tool, args })
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

/// One dispatched request's outcome, rendered to the `(ok, result)` envelope
/// at resume time.
///
/// The typed error is never flattened into the envelope: on failure the
/// envelope carries only the display string for the shim to raise, and
/// [`into_envelope`](Answer::into_envelope) hands the typed error back to the
/// driver, which retains it against the pending request and substitutes it
/// when the shim-raised error surfaces as the coroutine's failure. This holds
/// uniformly for leaf and structural answers: the enum owns the typed error
/// until the envelope is rendered, so an `Execute` or `Fanout` failure
/// round-trips with its structure intact, never stringified.
///
/// The error type is the driver's: the Lua side produces
/// `Answer<`[`Error`]`>` (argument-validation failures at the yield
/// boundary), while the executor's scheduler drives `Answer` over its own
/// substrate so a dispatch failure (a gateway completion error, a binding
/// failure) round-trips typed.
#[derive(Debug)]
pub enum Answer<E> {
    /// The completion text for an `infer` request.
    Infer(std::result::Result<String, E>),
    /// The contained chain's final text for an `execute` request.
    Execute(std::result::Result<String, E>),
    /// The ordered arm results for a `fanout` request, in collection order.
    Fanout(std::result::Result<Vec<LuaFanoutResult>, E>),
}

impl<E> Answer<E> {
    /// Maps the carried error type, leaving every success value untouched.
    pub fn map_error<F>(self, map: impl FnOnce(E) -> F) -> Answer<F> {
        match self {
            Answer::Infer(result) => Answer::Infer(result.map_err(map)),
            Answer::Execute(result) => Answer::Execute(result.map_err(map)),
            Answer::Fanout(result) => Answer::Fanout(result.map_err(map)),
        }
    }
}

impl<E: std::fmt::Display> Answer<E> {
    /// Renders the `(ok, result)` resume values for the shim.
    ///
    /// On success the envelope is `(true, text)` or, for a fanout, `(true,
    /// sequence)` with the packed 1-based result table built on the chain's
    /// VM. On failure it is `(false, message)`, where `message` is the
    /// error's display string - the shim raises it with `error(result, 0)`,
    /// so the author sees exactly the host's message - and the typed
    /// [`Error`] is returned alongside for the driver to retain.
    ///
    /// # Errors
    /// Returns an `mlua` error if a Lua string, userdata, or table cannot be
    /// created on `lua`.
    pub fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<E>)> {
        match self {
            Answer::Infer(Ok(text)) | Answer::Execute(Ok(text)) => {
                let text = lua.create_string(&text)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::String(text)]),
                    None,
                ))
            }
            Answer::Fanout(Ok(results)) => {
                let mut handles = Vec::with_capacity(results.len());
                for result in results {
                    handles.push(lua.create_userdata(result)?);
                }
                let sequence = pack_sequence(lua, handles)?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(true), Value::Table(sequence)]),
                    None,
                ))
            }
            Answer::Infer(Err(error))
            | Answer::Execute(Err(error))
            | Answer::Fanout(Err(error)) => {
                let message = lua.create_string(error.to_string())?;
                Ok((
                    MultiValue::from_vec(vec![Value::Boolean(false), Value::String(message)]),
                    Some(error),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU32;

    use mlua::{AnyUserData, Function};
    use serde_json::json;

    use super::*;
    use promptforge_model_client::model::{ModelId, ModelInvocation};

    fn test_binding() -> ModelBinding {
        ModelBinding::new(
            "fast",
            "a fast model",
            ModelId::from_validated("gateway", "test-model"),
            ModelInvocation {
                temperature: None,
                max_tokens: None,
                thinking: None,
            },
            NonZeroU32::new(4096).expect("4096 is non-zero"),
        )
    }

    fn handle_userdata(lua: &Lua) -> AnyUserData {
        lua.create_userdata(LuaModelHandle::from_binding(&test_binding()))
            .expect("userdata creation cannot fail on a fresh VM")
    }

    fn request_table(lua: &Lua, op: &str) -> mlua::Table {
        let table = lua.create_table().expect("table creation cannot fail");
        table
            .raw_set("op", op)
            .expect("raw_set on a fresh table cannot fail");
        table
    }

    fn set_var_snapshot(lua: &Lua, table: &mlua::Table) {
        let var = lua.create_table().expect("table creation cannot fail");
        var.raw_set("k", 1)
            .expect("raw_set on a fresh table cannot fail");
        table
            .raw_set("var", var)
            .expect("raw_set on a fresh table cannot fail");
    }

    fn assert_direct_yield(parse: YieldParse) {
        match parse {
            YieldParse::Malformed(Error::Lua(message)) => {
                assert_eq!(message, "scripts may not yield directly");
            }
            other => panic!("expected the direct-yield Lua error, got {other:?}"),
        }
    }

    fn expect_request(parse: YieldParse) -> Request {
        match parse {
            YieldParse::Request(request) => request,
            other => panic!("expected a well-formed request, got {other:?}"),
        }
    }

    fn echo_through_lua(lua: &Lua, envelope: MultiValue) -> (bool, Value) {
        let echo: Function = lua
            .create_function(|_, (ok, result): (bool, Value)| Ok((ok, result)))
            .expect("echo function creation cannot fail");
        echo.call::<(bool, Value)>(envelope)
            .expect("the envelope round-trips through Lua")
    }

    #[test]
    fn infer_without_a_handle_parses() {
        let lua = Lua::new();
        let table = request_table(&lua, "infer");
        table.raw_set("prompt", "summarize this").expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Infer { prompt, binding } => {
                assert_eq!(prompt, "summarize this");
                assert_eq!(binding, None);
            }
            other => panic!("expected an infer request, got {other:?}"),
        }
    }

    #[test]
    fn infer_with_a_handle_clones_its_frozen_binding() {
        let lua = Lua::new();
        let table = request_table(&lua, "infer");
        table.raw_set("prompt", "hi").expect("raw_set");
        table
            .raw_set("handle", handle_userdata(&lua))
            .expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Infer {
                binding: Some(binding),
                ..
            } => {
                assert_eq!(binding.alias(), "fast");
                assert_eq!(binding.id().name(), "test-model");
            }
            other => panic!("expected an infer request with a binding, got {other:?}"),
        }
    }

    #[test]
    fn execute_parses_target_input_and_var_snapshot() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        table.raw_set("input", "override").expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Execute { target, input, var } => {
                assert_eq!(target, "## Child");
                assert_eq!(input.as_deref(), Some("override"));
                assert_eq!(var, json!({ "k": 1 }));
            }
            other => panic!("expected an execute request, got {other:?}"),
        }
    }

    #[test]
    fn execute_without_input_yields_none() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Execute { input, .. } => assert_eq!(input, None),
            other => panic!("expected an execute request, got {other:?}"),
        }
    }

    #[test]
    fn fanout_parses_and_converts_the_collection_member_wise() {
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", "### Worker").expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        collection.raw_set(1, "a").expect("raw_set");
        collection.raw_set(2, 2).expect("raw_set");
        collection.raw_set("key", true).expect("raw_set");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Fanout { worker, items, var } => {
                assert_eq!(worker, "### Worker");
                assert_eq!(
                    items,
                    vec![json!("a"), json!(2), json!({ "key": "key", "value": true })]
                );
                assert_eq!(var, json!({ "k": 1 }));
            }
            other => panic!("expected a fanout request, got {other:?}"),
        }
    }

    #[test]
    fn mcp_reserved_fields_parse() {
        let lua = Lua::new();
        let table = request_table(&lua, "mcp");
        table.raw_set("server", "srv").expect("raw_set");
        table.raw_set("tool", "tl").expect("raw_set");
        let args = lua.create_table().expect("table creation cannot fail");
        table.raw_set("args", args).expect("raw_set");
        let request = expect_request(Request::from_yield(&lua, &Value::Table(table)));
        match request {
            Request::Mcp { server, tool, args } => {
                assert_eq!(server, "srv");
                assert_eq!(tool, "tl");
                assert_eq!(args, json!({}));
            }
            other => panic!("expected an mcp request, got {other:?}"),
        }
    }

    #[test]
    fn a_received_mcp_request_is_a_typed_protocol_error() {
        match Request::mcp_reserved() {
            Error::Lua(message) => assert!(message.contains("mcp")),
            other => panic!("expected a typed Lua protocol error, got {other:?}"),
        }
    }

    #[test]
    fn a_non_table_yield_is_rejected() {
        let lua = Lua::new();
        assert_direct_yield(Request::from_yield(&lua, &Value::Integer(1)));
        let text = lua.create_string("infer").expect("string creation");
        assert_direct_yield(Request::from_yield(&lua, &Value::String(text)));
    }

    #[test]
    fn a_yield_without_an_op_is_rejected() {
        let lua = Lua::new();
        let table = lua.create_table().expect("table creation cannot fail");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_unknown_op_is_rejected() {
        let lua = Lua::new();
        let table = request_table(&lua, "teleport");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_infer_with_a_missing_or_non_string_prompt_is_the_calls_error() {
        // The author-facing argument error rides back as the call's answer,
        // so the shim raises it at the call site (pcall-able), exactly as
        // the legacy callback's conversion error surfaced.
        let lua = Lua::new();
        let missing = request_table(&lua, "infer");
        match Request::from_yield(&lua, &Value::Table(missing)) {
            YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
                assert_eq!(message, "prompt must be a string, got nil");
            }
            other => panic!("expected the prompt call error, got {other:?}"),
        }
        let typed_wrong = request_table(&lua, "infer");
        typed_wrong.raw_set("prompt", 42).expect("raw_set");
        match Request::from_yield(&lua, &Value::Table(typed_wrong)) {
            YieldParse::Call(Answer::Infer(Err(Error::Lua(message)))) => {
                assert_eq!(message, "prompt must be a string, got integer");
            }
            other => panic!("expected the prompt call error, got {other:?}"),
        }
    }

    #[test]
    fn an_infer_with_a_wrong_handle_type_is_rejected() {
        let lua = Lua::new();
        let as_string = request_table(&lua, "infer");
        as_string.raw_set("prompt", "hi").expect("raw_set");
        as_string
            .raw_set("handle", "not a handle")
            .expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(as_string)));
        let as_other_userdata = request_table(&lua, "infer");
        as_other_userdata.raw_set("prompt", "hi").expect("raw_set");
        let wrong = lua
            .create_userdata(LuaFanoutResult::success(json!(1), "x"))
            .expect("userdata creation cannot fail on a fresh VM");
        as_other_userdata.raw_set("handle", wrong).expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(as_other_userdata)));
    }

    #[test]
    fn an_execute_with_a_non_string_target_keeps_the_resolve_error() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", 42).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Execute(Err(Error::LuaRuntime { message, .. }))) => {
                assert!(
                    message.contains("section target must be a string, got integer"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected the resolve_section_target call error, got {other:?}"),
        }
    }

    #[test]
    fn a_fanout_with_a_non_string_worker_is_the_calls_error() {
        // The author-facing argument error rides back as the call's answer,
        // so the shim raises it at the call site (pcall-able), exactly as
        // the legacy callback's conversion error surfaced.
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", 42).expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => {
                assert_eq!(message, "worker must be a string, got integer");
            }
            other => panic!("expected the worker call error, got {other:?}"),
        }
    }

    #[test]
    fn a_request_without_a_var_snapshot_is_rejected() {
        let lua = Lua::new();
        let table = request_table(&lua, "execute");
        table.raw_set("target", "## Child").expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn fanout_collection_member_errors_stay_byte_identical() {
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", "### Worker").expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        let member = lua
            .create_function(|_, ()| Ok(()))
            .expect("function creation cannot fail");
        collection.raw_set(1, member).expect("raw_set");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        match Request::from_yield(&lua, &Value::Table(table)) {
            YieldParse::Call(Answer::Fanout(Err(Error::Lua(message)))) => assert_eq!(
                message,
                "fanout collection member at index 1 is a function; members must be data"
            ),
            other => panic!("expected the collection member call error, got {other:?}"),
        }
    }

    #[test]
    fn metatable_spoofed_fields_are_not_read() {
        let lua = Lua::new();
        let table = lua.create_table().expect("table creation cannot fail");
        let index = lua.create_table().expect("table creation cannot fail");
        index.raw_set("op", "infer").expect("raw_set");
        index.raw_set("prompt", "hi").expect("raw_set");
        let metatable = lua.create_table().expect("table creation cannot fail");
        metatable.raw_set("__index", index).expect("raw_set");
        table
            .set_metatable(Some(metatable))
            .expect("set_metatable on a fresh table cannot fail");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
    }

    #[test]
    fn an_ok_infer_answer_round_trips_through_lua() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::<Error>::Infer(Ok("completion".to_owned()))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(ok);
        let Value::String(text) = result else {
            panic!("expected a string result, got {result:?}");
        };
        assert_eq!(text.to_str().expect("the text is UTF-8"), "completion");
    }

    #[test]
    fn an_ok_execute_answer_round_trips_through_lua() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::<Error>::Execute(Ok("chain text".to_owned()))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(ok);
        let Value::String(text) = result else {
            panic!("expected a string result, got {result:?}");
        };
        assert_eq!(text.to_str().expect("the text is UTF-8"), "chain text");
    }

    #[test]
    fn an_err_answer_round_trips_and_retains_the_typed_error() {
        let lua = Lua::new();
        let (envelope, retained) = Answer::Execute(Err(Error::LuaQuota {
            resource: "instruction",
        }))
        .into_envelope(&lua)
        .expect("the envelope renders");
        match retained {
            Some(Error::LuaQuota {
                resource: "instruction",
            }) => {}
            other => panic!("expected the retained LuaQuota error, got {other:?}"),
        }
        let (ok, result) = echo_through_lua(&lua, envelope);
        assert!(!ok);
        let Value::String(message) = result else {
            panic!("expected a string message, got {result:?}");
        };
        assert_eq!(
            message.to_str().expect("the message is UTF-8"),
            "lua instruction quota exceeded"
        );
    }

    #[test]
    fn an_ok_fanout_answer_round_trips_as_an_ordered_result_sequence() {
        let lua = Lua::new();
        let results = vec![
            LuaFanoutResult::success(json!("a"), "text-a"),
            LuaFanoutResult::exhausted_stub(json!("b"), "stub-b"),
        ];
        let (envelope, retained) = Answer::<Error>::Fanout(Ok(results))
            .into_envelope(&lua)
            .expect("the envelope renders");
        assert!(retained.is_none());
        let (ok, len, first_text, second_ok, second_exhausted, rendered): (
            bool,
            i64,
            String,
            bool,
            bool,
            String,
        ) = lua
            .load(
                "local ok, seq = ...; \
                 return ok, #seq, seq[1].text, seq[2].ok, seq[2].exhausted, tostring(seq[1])",
            )
            .call(envelope)
            .expect("the sequence reads back through Lua");
        assert!(ok);
        assert_eq!(len, 2);
        assert_eq!(first_text, "text-a");
        assert!(!second_ok);
        assert!(second_exhausted);
        assert_eq!(rendered, "text-a");
    }
}
