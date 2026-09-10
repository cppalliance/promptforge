use mlua::{Lua, Value, Variadic};
use promptforge_core_support::observe::NullObserver;
use promptforge_core_support::untrusted::GuardNonce;
use promptforge_store::StoreRef;
use serde_json::json;

use super::decode::{add_local_params_schema, collect_tools_add_entries, tool_alias};
use super::userdata::LuaToolHandle;
use super::{install_h2_tools, install_tool_call_counts};
use crate::handles::{LuaFanoutResult, ToolSet};
use crate::scope::ToolRuntime;
use crate::{SectionVm, ToolBinding};
use promptforge_tools::ToolId;
use std::sync::{Arc, Mutex};

fn echo_handle() -> LuaToolHandle {
    LuaToolHandle::from_binding(
        "echo",
        "echo tool",
        &ToolId::new("tests", "echo").expect("id"),
    )
}

#[test]
fn tool_alias_accepts_a_bare_alias_string() {
    let lua = Lua::new();
    let value = lua.create_string("echo").expect("string");
    assert_eq!(
        tool_alias(&Value::String(value)).expect("a string decodes"),
        "echo"
    );
}

#[test]
fn tool_alias_reads_the_alias_off_a_tool_object() {
    let lua = Lua::new();
    let userdata = lua.create_userdata(echo_handle()).expect("userdata");
    assert_eq!(
        tool_alias(&Value::UserData(userdata)).expect("a Tool object decodes"),
        "echo",
        "a Tool object contributes the alias it was bound under"
    );
}

#[test]
fn tool_alias_rejects_other_types_and_other_userdata() {
    let lua = Lua::new();
    let number = tool_alias(&Value::Integer(42)).expect_err("a number is not an alias");
    assert!(
        number
            .to_string()
            .contains("tools.call alias must be a string or Tool object, got integer"),
        "the rejection names the accepted forms: {number}"
    );
    // A userdata that is not a Tool object takes the same rejection; the
    // borrow failure must not leak mlua's type-mismatch wording.
    let fanout = lua
        .create_userdata(LuaFanoutResult::success(json!(1), "text"))
        .expect("userdata");
    let other = tool_alias(&Value::UserData(fanout)).expect_err("not a Tool object");
    assert!(
        other
            .to_string()
            .contains("tools.call alias must be a string or Tool object, got userdata"),
        "a foreign userdata gets the same rejection: {other}"
    );
}

#[test]
fn tools_add_entries_accept_strings_tools_and_arrays() {
    let lua = Lua::new();
    let tool = lua.create_userdata(echo_handle()).expect("userdata");
    let entries = collect_tools_add_entries(Variadic::from_iter([
        Value::String(lua.create_string("search").expect("string")),
        Value::String(lua.create_string("an override").expect("string")),
    ]))
    .expect("alias plus override decodes");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].alias, "search");
    assert_eq!(
        entries[0].description_override.as_deref(),
        Some("an override")
    );

    let array = lua
        .create_sequence_from(vec![Value::UserData(tool.clone())])
        .expect("array");
    array
        .raw_push(Value::String(lua.create_string("fetch").expect("string")))
        .expect("push");
    let entries = collect_tools_add_entries(Variadic::from_iter([Value::Table(array)]))
        .expect("the array form decodes");
    let aliases: Vec<&str> = entries.iter().map(|entry| entry.alias.as_str()).collect();
    assert_eq!(
        aliases,
        vec!["echo", "fetch"],
        "Tool objects and strings mix in the array form"
    );
}

#[test]
fn add_local_params_schema_builds_object_schema_with_required_fields() {
    let lua = Lua::new();
    let params = lua
        .load("{ query = 'string', limit = { 'integer', 'maximum hits' } }")
        .eval::<mlua::Table>()
        .expect("params table evaluates");
    let schema = add_local_params_schema(&params).expect("the schema builds");
    assert_eq!(
        schema["properties"],
        json!({
            "query": { "type": "string" },
            "limit": { "type": "integer", "description": "maximum hits" },
        })
    );
    // Lua hash-part iteration order is unspecified, so the required list is
    // compared as a set.
    let mut required: Vec<String> =
        serde_json::from_value(schema["required"].clone()).expect("the required list is strings");
    required.sort();
    assert_eq!(required, vec!["limit", "query"]);
    assert_eq!(schema["type"], "object");
}

#[test]
fn add_local_params_schema_rejects_an_unsupported_type() {
    let lua = Lua::new();
    let params = lua
        .load("{ payload = 'table' }")
        .eval::<mlua::Table>()
        .expect("params table evaluates");
    let error = add_local_params_schema(&params).expect_err("an unsupported type fails");
    assert!(
        error.to_string().contains("unsupported type \"table\""),
        "the rejection names the bad type: {error}"
    );
}

/// Installs the H2 namespace on a fresh VM and returns it.
fn lua_with_h2_tools() -> Lua {
    let lua = Lua::new();
    let globals = lua.globals();
    let runtime = Arc::new(Mutex::new(ToolRuntime {
        added: Vec::new(),
        description_overrides: std::collections::BTreeMap::default(),
    }));
    install_h2_tools(
        &lua,
        &globals,
        &ToolSet::default(),
        &runtime,
        &crate::vm::LocalTools::default(),
    )
    .expect("the H2 tools install cannot fail on a fresh VM");
    lua
}

#[test]
fn the_h2_namespace_carries_declaration_scoping_and_no_call_yet() {
    // `call` is absent here on purpose: it suspends, so the coroutine shim
    // prelude installs it - this table carries exactly the non-suspending
    // operations.
    let lua = lua_with_h2_tools();
    let (has_add, has_add_local, call_is_nil): (bool, bool, bool) = lua
        .load(
            "return type(tools.add) == 'function', \
                    type(tools.add_local) == 'function', \
                    tools.call == nil",
        )
        .eval()
        .expect("the namespace probe evaluates");
    assert!(has_add && has_add_local && call_is_nil);
    let bind_error = lua
        .load("local ok, err = pcall(tools.bind, 'x', 'y'); return not ok, tostring(err)")
        .eval::<(bool, String)>()
        .expect("the bind stub raises");
    assert!(bind_error.0);
    assert!(
        bind_error
            .1
            .contains("tools.bind is only available during live H1 execution"),
        "the H1-only operations stay forbidden in a section: {bind_error:?}"
    );
}

#[test]
fn the_shim_prelude_installs_tools_call_and_no_bare_global() {
    let nonce = GuardNonce::fresh();
    let observer = NullObserver::default();
    let mut vm = SectionVm::new(&nonce, "test-run", &observer, "Test")
        .expect("section VM construction cannot fail");
    vm.inject_host("", &json!({}), &StoreRef::memory())
        .expect("host injection cannot fail");
    vm.install_coro_shims().expect("the shim prelude installs");
    let (call_is_function, bare_is_nil): (bool, bool) = vm
        .lua()
        .load("return type(tools.call) == 'function', tool_call == nil")
        .eval()
        .expect("the namespace probe evaluates");
    assert!(call_is_function, "tools.call installs on the tools table");
    assert!(bare_is_nil, "the bare tool_call global is gone");
    vm.teardown(&observer, "Test");
}

#[test]
fn tool_call_counts_seed_read_and_reject_unknown_keys() {
    let lua = lua_with_h2_tools();
    let bound = ToolSet::for_test(
        vec![ToolBinding::for_test(
            "echo",
            "echo tool",
            Arc::new(EchoTool),
        )],
        Vec::new(),
    );
    let counts =
        install_tool_call_counts(&lua, &bound, bound.bindings()).expect("the counts install");
    assert_eq!(counts.get("echo").expect("read"), Some(0));
    let error = lua
        .load(
            "local ok, err = pcall(function() return tools.calls.ghost end); return tostring(err)",
        )
        .eval::<String>()
        .expect("the unknown-key read raises");
    assert!(
        error.contains("\"ghost\" has no seeded count"),
        "an unseeded key names itself and the seeded set: {error}"
    );
}

/// A trivial tool so the counts test can bind an alias.
struct EchoTool;

#[async_trait::async_trait]
impl promptforge_tools::Tool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::new("tests", "echo").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn wire_name(&self) -> &str {
        "echo"
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the Tool trait fixes this return type to &str"
    )]
    fn description(&self) -> &str {
        "echo tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object" })
    }

    async fn call(
        &self,
        _args: serde_json::Value,
    ) -> std::result::Result<promptforge_tools::ToolOutput, promptforge_tools::ToolError> {
        Ok(promptforge_tools::ToolOutput::trusted("echoed"))
    }
}
