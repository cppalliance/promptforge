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

use crate::lua::{LuaFanoutResult, LuaModelHandle, pack_sequence, resolve_section_target};
use crate::model::ModelBinding;
use crate::{Error, Result};

/// The fixed failure for a yield that is not a well-formed request table.
///
/// The coroutine global is stripped from author reach, so the only yields in
/// a well-formed run are shim yields, which are well-formed by construction;
/// anything else is a hand-rolled or corrupted yield and fails the block as a
/// loud authoring error rather than confusing the driver.
const DIRECT_YIELD: &str = "scripts may not yield directly";

/// Fails the block with the fixed direct-yield message.
fn direct_yield<T>() -> Result<T> {
    Err(Error::Lua(DIRECT_YIELD.to_owned()))
}

/// Reads one field off the request table.
///
/// Reads are raw: the table comes from script space, so a metatable must not
/// intercept or forge a field.
fn raw_field(table: &mlua::Table, name: &str) -> Result<Value> {
    table.raw_get::<Value>(name).or_else(|_| direct_yield())
}

/// Reads a required string field.
fn string_field(table: &mlua::Table, name: &str) -> Result<String> {
    match raw_field(table, name)? {
        Value::String(value) => Ok(value.to_str().or_else(|_| direct_yield())?.to_owned()),
        _ => direct_yield(),
    }
}

/// Reads an optional string field: absent or nil is `None`.
fn optional_string_field(table: &mlua::Table, name: &str) -> Result<Option<String>> {
    match raw_field(table, name)? {
        Value::Nil => Ok(None),
        Value::String(value) => Ok(Some(value.to_str().or_else(|_| direct_yield())?.to_owned())),
        _ => direct_yield(),
    }
}

/// Reads a required plain-table field as its JSON snapshot.
fn json_field(lua: &Lua, table: &mlua::Table, name: &str) -> Result<serde_json::Value> {
    match raw_field(table, name)? {
        value @ Value::Table(_) => lua.from_value(value).or_else(|_| direct_yield()),
        _ => direct_yield(),
    }
}

/// A validated suspending host call, parsed from the yielded table.
///
/// The parse happens at the resume boundary while the VM handle is live: the
/// fanout collection converts through the existing member-wise rules and the
/// handle userdata's [`ModelBinding`] is cloned out of its borrow, so nothing
/// lifetime-bound enters the enum.
#[derive(Debug)]
pub(crate) enum Request {
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
    /// Validates a yielded value into a `Request`.
    ///
    /// Every field is checked before use: the table comes from script space.
    ///
    /// # Errors
    /// Returns [`Error::Lua`] with "scripts may not yield directly" when the
    /// yield is not a well-formed request table: not a table, no `op`, an
    /// unknown `op`, or a field of the wrong shape. Two boundary conversions
    /// keep their own byte-identical errors instead: an `execute` target that
    /// is not a string fails as `resolve_section_target` fails today, and a
    /// fanout collection fails as `collection_to_items` fails today.
    pub(crate) fn from_yield(lua: &Lua, yielded: &Value) -> Result<Request> {
        let Value::Table(table) = yielded else {
            return direct_yield();
        };
        let op = match raw_field(table, "op")? {
            Value::String(op) => op.to_str().or_else(|_| direct_yield())?.to_owned(),
            _ => return direct_yield(),
        };
        match op.as_str() {
            "infer" => {
                let prompt = string_field(table, "prompt")?;
                let binding = match raw_field(table, "handle")? {
                    Value::Nil => None,
                    Value::UserData(userdata) => {
                        let handle = userdata
                            .borrow::<LuaModelHandle>()
                            .or_else(|_| direct_yield())?;
                        Some(handle.binding().clone())
                    }
                    _ => return direct_yield(),
                };
                Ok(Request::Infer { prompt, binding })
            }
            "execute" => {
                let target =
                    resolve_section_target(raw_field(table, "target")?).map_err(Error::lua)?;
                let input = optional_string_field(table, "input")?;
                let var = json_field(lua, table, "var")?;
                Ok(Request::Execute { target, input, var })
            }
            "fanout" => {
                let worker = string_field(table, "worker")?;
                let collection = raw_field(table, "collection")?;
                let items = crate::fanout::collection_to_items(lua, &collection)?;
                let var = json_field(lua, table, "var")?;
                Ok(Request::Fanout { worker, items, var })
            }
            "mcp" => {
                let server = string_field(table, "server")?;
                let tool = string_field(table, "tool")?;
                let args = json_field(lua, table, "args")?;
                Ok(Request::Mcp { server, tool, args })
            }
            _ => direct_yield(),
        }
    }

    /// The typed protocol error for a received `mcp` request.
    ///
    /// The `mcp` fields are reserved and no call surface produces the request
    /// yet, so the driver never dispatches one; receiving it fails the chain
    /// with this error rather than reaching an unimplemented path.
    pub(crate) fn mcp_reserved() -> Error {
        Error::Lua("mcp requests are reserved: no dispatcher exists yet".to_owned())
    }

    /// The typed protocol error for a received `fanout` request before the
    /// scheduler's fanout dispatch exists.
    ///
    /// The `fanout` shim is not installed yet, so no call surface produces
    /// the request; receiving one fails the chain with this error rather
    /// than reaching an unimplemented path.
    // Consumed by the scheduler driver until the fanout step replaces it.
    #[allow(dead_code)]
    pub(crate) fn fanout_reserved() -> Error {
        Error::Lua("fanout requests are reserved: no dispatcher exists yet".to_owned())
    }
}

/// One dispatched request's outcome, rendered to the `(ok, result)` envelope
/// at resume time.
///
/// The typed [`Error`] is never flattened into the envelope: on failure the
/// envelope carries only the display string for the shim to raise, and
/// [`into_envelope`](Answer::into_envelope) hands the typed error back to the
/// driver, which retains it against the pending request and substitutes it
/// when the shim-raised error surfaces as the coroutine's failure. This holds
/// uniformly for leaf and structural answers: the enum owns the typed error
/// until the envelope is rendered, so an `Execute` or `Fanout` failure
/// round-trips with its structure intact, never stringified.
#[derive(Debug)]
pub(crate) enum Answer {
    /// The completion text for an `infer` request.
    Infer(Result<String>),
    /// The contained chain's final text for an `execute` request.
    Execute(Result<String>),
    /// The ordered arm results for a `fanout` request, in collection order.
    Fanout(Result<Vec<LuaFanoutResult>>),
}

impl Answer {
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
    pub(crate) fn into_envelope(self, lua: &Lua) -> mlua::Result<(MultiValue, Option<Error>)> {
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
    use crate::model::{ModelId, ModelInvocation};

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

    fn assert_direct_yield(result: Result<Request>) {
        match result {
            Err(Error::Lua(message)) => assert_eq!(message, "scripts may not yield directly"),
            other => panic!("expected the direct-yield Lua error, got {other:?}"),
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed infer parses");
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed infer parses");
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed execute parses");
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed execute parses");
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed fanout parses");
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
        let request =
            Request::from_yield(&lua, &Value::Table(table)).expect("a well-formed mcp parses");
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
    fn an_infer_with_a_missing_or_non_string_prompt_is_rejected() {
        let lua = Lua::new();
        let missing = request_table(&lua, "infer");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(missing)));
        let typed_wrong = request_table(&lua, "infer");
        typed_wrong.raw_set("prompt", 42).expect("raw_set");
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(typed_wrong)));
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
            Err(Error::LuaRuntime { message, .. }) => {
                assert!(
                    message.contains("section target must be a string, got integer"),
                    "unexpected message: {message}"
                );
            }
            other => panic!("expected the resolve_section_target error, got {other:?}"),
        }
    }

    #[test]
    fn a_fanout_with_a_non_string_worker_is_rejected() {
        let lua = Lua::new();
        let table = request_table(&lua, "fanout");
        table.raw_set("worker", 42).expect("raw_set");
        let collection = lua.create_table().expect("table creation cannot fail");
        table.raw_set("collection", collection).expect("raw_set");
        set_var_snapshot(&lua, &table);
        assert_direct_yield(Request::from_yield(&lua, &Value::Table(table)));
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
            Err(Error::Lua(message)) => assert_eq!(
                message,
                "fanout collection member at index 1 is a function; members must be data"
            ),
            other => panic!("expected the collection member error, got {other:?}"),
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
        let (envelope, retained) = Answer::Infer(Ok("completion".to_owned()))
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
        let (envelope, retained) = Answer::Execute(Ok("chain text".to_owned()))
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
        let (envelope, retained) = Answer::Fanout(Ok(results))
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
