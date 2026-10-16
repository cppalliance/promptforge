//! Protocol tests: yield parsing and answer envelopes.
//!
//! The submodules follow the protocol's own split: `parse` the generic
//! yield-to-request validation, `parse_chat` the message-list request,
//! `parse_tasks` the task-operation requests, `parse_tool_call` the
//! `tool_call` request, `answer` the answer-to-envelope round trips,
//! `answer_chat` the `chat` answer's typed error, and `answer_retained`
//! when a block's failure is the answer's retained error. The helpers
//! below are shared.

use std::num::NonZeroU32;

use mlua::{AnyUserData, Function, IntoLuaMulti, Lua, LuaSerdeExt, MultiValue, Value};
use serde_json::json;

use runvil_model_client::model::{ModelBinding, ModelId, ModelInvocation};
use runvil_types::ids::{TaskId, TaskOrigin};

use crate::Error;
use crate::models::LuaModelHandle;

use super::*;

/// A userdata that is neither a model handle nor a Tool object, for the
/// wrong-userdata argument cases.
struct OtherUserData;

impl mlua::UserData for OtherUserData {}

fn test_binding() -> ModelBinding {
    ModelBinding::new(
        "fast",
        "a fast model",
        ModelId::gateway("test-model").expect("a static model id is valid"),
        ModelInvocation::new(),
        NonZeroU32::new(4096).expect("4096 is non-zero"),
    )
}

fn handle_userdata(lua: &Lua) -> AnyUserData {
    lua.create_userdata(LuaModelHandle::from_binding(test_binding()))
        .expect("userdata creation cannot fail on a fresh VM")
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

/// Evaluates a Lua table constructor, so each test builds its yield,
/// message, and opts tables from the exact source a shim or an author
/// would write.
fn lua_table(lua: &Lua, source: &str) -> mlua::Table {
    lua.load(source)
        .eval()
        .expect("test table source evaluates")
}

/// Runs a Lua chunk that names `args` from `...` and returns a table, for
/// a field no source text can write, such as a userdata.
fn lua_table_with(lua: &Lua, chunk: &str, args: impl IntoLuaMulti) -> mlua::Table {
    lua.load(chunk)
        .call(args)
        .expect("test table chunk returns a table")
}

mod answer;
mod answer_chat;
mod answer_retained;
mod parse;
mod parse_chat;
mod parse_tasks;
mod parse_tool_call;
