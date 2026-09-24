//! Protocol tests: yield parsing and answer envelopes.
//!
//! The submodules follow the protocol's own split: `parse` the generic
//! yield-to-request validation, `parse_chat` the message-list request,
//! `parse_tasks` the task-operation requests, `answer` the
//! answer-to-envelope round trips, and `answer_chat` the `chat` answer's
//! shapes. The helpers below are shared.

use std::num::NonZeroU32;

use mlua::{AnyUserData, Function, Lua, MultiValue, Value};
use serde_json::json;

use promptforge_model_client::model::{ModelBinding, ModelInvocation};
use promptforge_types::detail::model_id_from_validated;
use promptforge_types::ids::{TaskId, TaskOrigin};
use promptforge_types::metrics::{CallMetrics, ToolCallEvent};

use crate::{Error, LuaModelHandle};

use super::*;

/// A userdata that is neither a model handle nor a Tool object, for the
/// wrong-userdata argument cases.
struct OtherUserData;

impl mlua::UserData for OtherUserData {}

fn test_binding() -> ModelBinding {
    ModelBinding::new(
        "fast",
        "a fast model",
        model_id_from_validated("gateway", "test-model"),
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

/// Reads a failure envelope's payload as `(kind, tostring)`: every failure
/// that reaches Lua is a `{ kind, message, ... }` table whose `tostring` is
/// the message.
fn failure_parts(lua: &Lua, result: Value) -> (String, String) {
    lua.load("local err = ...; return err.kind, tostring(err)")
        .call(result)
        .expect("the failure table reads back through Lua")
}

/// Evaluates a Lua table constructor, so chat tests build message and
/// opts tables from the exact source an author would write.
fn lua_table(lua: &Lua, source: &str) -> mlua::Table {
    lua.load(source)
        .eval()
        .expect("test table source evaluates")
}

mod answer;
mod answer_chat;
mod parse;
mod parse_chat;
mod parse_tasks;
