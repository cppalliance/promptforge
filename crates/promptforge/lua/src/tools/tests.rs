use mlua::{Lua, Value, Variadic};
use promptforge_api_types::observe::NullObserver;
use promptforge_api_types::untrusted::GuardNonce;
use serde_json::json;

use super::decode::{add_local_params_schema, collect_tools_add_entries, tool_alias};
use super::userdata::LuaToolHandle;
use super::{install_tool_call_counts, install_tools};
use crate::handles::ToolSet;
use crate::scope::{TaskAllowlist, ToolRuntime};
use crate::{SectionVm, ToolBinding};
use promptforge_api_types::tools::ToolId;
use std::sync::{Arc, Mutex};

/// A fresh stock handle's access capability for a test VM.
fn fresh_access() -> Arc<promptforge_store::Access> {
    Arc::new(
        promptforge_vfs::empty()
            .acquire(shared_vfs::Origin::new("tool test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// A userdata that is not a Tool object, for the foreign-userdata rejection.
struct Foreign;

impl mlua::UserData for Foreign {}

fn echo_handle() -> LuaToolHandle {
    LuaToolHandle::from_binding(
        "echo",
        "echo tool",
        &ToolId::parse("tests/tools/echo").expect("id"),
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
    let foreign = lua.create_userdata(Foreign).expect("userdata");
    let other = tool_alias(&Value::UserData(foreign)).expect_err("not a Tool object");
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

/// Installs the tools namespace on a fresh VM and returns it with the
/// runtime the namespace records into.
fn lua_with_tools_and_runtime() -> (Lua, Arc<Mutex<ToolRuntime>>) {
    let lua = Lua::new();
    let globals = lua.globals();
    let runtime = Arc::new(Mutex::new(ToolRuntime {
        added: Vec::new(),
        description_overrides: std::collections::BTreeMap::default(),
        allowed_tasks: None,
    }));
    install_tools(
        &lua,
        &globals,
        &Arc::new(Mutex::new(ToolSet::default())),
        &runtime,
        &crate::vm::LocalTools::default(),
    )
    .expect("the tools install cannot fail on a fresh VM");
    (lua, runtime)
}

/// Installs the tools namespace on a fresh VM and returns it.
fn lua_with_tools() -> Lua {
    lua_with_tools_and_runtime().0
}

#[test]
fn allow_tasks_records_the_section_allowlist_and_rejects_bad_targets() {
    let (lua, runtime) = lua_with_tools_and_runtime();
    let allowlist = || {
        runtime
            .lock()
            .expect("the runtime mutex is not poisoned")
            .allowed_tasks
            .clone()
    };
    assert_eq!(allowlist(), None, "nothing is allowed before the call");
    lua.load("tools.allow_tasks()")
        .exec()
        .expect("the bare call allows any target");
    assert_eq!(allowlist(), Some(TaskAllowlist::Any));
    lua.load("tools.allow_tasks({ '## Research', ' ## Draft ' })")
        .exec()
        .expect("a list narrows the allowlist");
    let narrowed = allowlist().expect("the list is recorded");
    assert_eq!(
        narrowed,
        TaskAllowlist::Only(vec!["## Research".to_owned(), "## Draft".to_owned()]),
        "the latest call replaces the earlier grant, headings trimmed"
    );
    assert!(narrowed.permits(" ## Draft") && !narrowed.permits("## Other"));
    for (call, fragment) in [
        ("tools.allow_tasks('## Research')", "got string"),
        ("tools.allow_tasks({})", "at least one section"),
        ("tools.allow_tasks({ 7 })", "got integer"),
        ("tools.allow_tasks({ '  ' })", "non-empty"),
    ] {
        let error = lua
            .load(call)
            .exec()
            .expect_err("a malformed allowlist is refused");
        assert!(
            error.to_string().contains(fragment),
            "{call} names its fault: {error}"
        );
    }
    assert_eq!(
        allowlist(),
        Some(TaskAllowlist::Only(vec![
            "## Research".to_owned(),
            "## Draft".to_owned()
        ])),
        "a refused call leaves the recorded allowlist alone"
    );
}

#[test]
fn the_tools_namespace_carries_scoping_and_no_bind_or_call() {
    // `call` is absent here on purpose: it suspends, so the coroutine shim
    // prelude installs it - this table carries exactly the non-suspending
    // operations. `bind` is gone entirely: binding is the frontmatter's.
    let lua = lua_with_tools();
    let (has_add, has_add_local, has_always, call_is_nil, bind_is_nil): (
        bool,
        bool,
        bool,
        bool,
        bool,
    ) = lua
        .load(
            "return type(tools.add) == 'function', \
                    type(tools.add_local) == 'function', \
                    type(tools.always) == 'function', \
                    tools.call == nil, \
                    tools.bind == nil",
        )
        .eval()
        .expect("the namespace probe evaluates");
    assert!(has_add && has_add_local && has_always && call_is_nil && bind_is_nil);
}

#[test]
fn the_shim_prelude_installs_tools_call_and_no_bare_global() {
    let nonce = GuardNonce::fresh();
    let observer = NullObserver::default();
    let mut vm = SectionVm::new(&nonce, "test-run", &observer, "Test")
        .expect("section VM construction cannot fail");
    vm.inject_host("", &json!({}), &fresh_access())
        .expect("host injection cannot fail");
    vm.install_coro_shims(24, 8)
        .expect("the shim prelude installs");
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
    let lua = lua_with_tools();
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
impl promptforge_api_types::tools::Tool for EchoTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tests/tools/echo").expect("valid id")
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
    ) -> std::result::Result<
        promptforge_api_types::tools::ToolOutput,
        promptforge_api_types::tools::ToolError,
    > {
        Ok(promptforge_api_types::tools::ToolOutput::trusted("echoed"))
    }
}
