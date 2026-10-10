//! Tests for the `tools` namespace installers, alias decoding, and the local params schema.

use mlua::{Lua, Value, Variadic};
use promptforge_types::untrusted::GuardNonce;
use serde_json::json;

use super::decode::{collect_offer_entries, local_params_schema, tool_name};
use super::userdata::LuaToolHandle;
use super::{install_tool_call_counts, install_tools};
use crate::handles::ToolSet;
use crate::scope::{TaskAllowlist, ToolRuntime};
use crate::{SectionVm, ToolBinding};
use promptforge_types::tools::ToolId;
use std::sync::{Arc, Mutex};

#[path = "tests-offering.rs"]
mod offering;

/// A fresh default handle's access capability for a test VM.
fn fresh_access() -> Arc<crate::Access> {
    Arc::new(
        promptforge_vfs::VfsRef::default()
            .acquire(promptforge_vfs::Origin::new("tool test fixture"))
            .expect("the stock backend acquires"),
    )
}

/// A userdata that is not a Tool object, for the foreign-userdata rejection.
struct Foreign;

impl mlua::UserData for Foreign {}

fn echo_handle() -> LuaToolHandle {
    LuaToolHandle::from_binding(&ToolBinding::from_descriptor("tools_echo", &echo_tool()))
}

#[test]
fn tool_name_accepts_a_bare_string() {
    let lua = Lua::new();
    let value = lua.create_string("tools/echo").expect("string");
    assert_eq!(
        tool_name(&Value::String(value)).expect("a string decodes"),
        "tools/echo"
    );
}

#[test]
fn tool_name_reads_the_id_off_a_tool_object() {
    let lua = Lua::new();
    let userdata = lua.create_userdata(echo_handle()).expect("userdata");
    assert_eq!(
        tool_name(&Value::UserData(userdata)).expect("a tool object decodes"),
        "tools/echo",
        "a tool object stands for its canonical id, not its wire name"
    );
}

#[test]
fn tool_name_rejects_tables_other_types_and_other_userdata() {
    let lua = Lua::new();
    let number = tool_name(&Value::Integer(42)).expect_err("a number is not a tool");
    assert!(
        number
            .to_string()
            .contains("tools.call takes a tool id, a local alias, or a tool object, got integer"),
        "the rejection names the accepted forms: {number}"
    );
    let record = lua
        .load("{ id = 'tools/echo', name = 'tools_echo' }")
        .eval::<mlua::Table>()
        .expect("the record evaluates");
    let table = tool_name(&Value::Table(record)).expect_err("a record is not a tool");
    assert!(
        table.to_string().contains("got table"),
        "a plain record no longer stands for a tool: {table}"
    );
    // A userdata that is not a tool object takes the same rejection; the
    // borrow failure must not leak mlua's type-mismatch wording.
    let foreign = lua.create_userdata(Foreign).expect("userdata");
    let other = tool_name(&Value::UserData(foreign)).expect_err("not a tool object");
    assert!(
        other
            .to_string()
            .contains("tools.call takes a tool id, a local alias, or a tool object, got userdata"),
        "a foreign userdata gets the same rejection: {other}"
    );
}

#[test]
fn tools_offer_entries_accept_strings_tool_objects_and_arrays() {
    let lua = Lua::new();
    let tool = lua.create_userdata(echo_handle()).expect("userdata");
    let entries = collect_offer_entries(
        "tools.offer",
        Variadic::from_iter([
            Value::String(lua.create_string("web/search").expect("string")),
            Value::String(lua.create_string("an override").expect("string")),
        ]),
    )
    .expect("an id plus override decodes");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "web/search");
    assert_eq!(
        entries[0].description_override.as_deref(),
        Some("an override")
    );
    let entries = collect_offer_entries(
        "tools.offer",
        Variadic::from_iter([
            Value::UserData(tool.clone()),
            Value::String(lua.create_string("an override").expect("string")),
        ]),
    )
    .expect("a tool object plus override decodes");
    assert_eq!(entries[0].name, "tools/echo");
    assert_eq!(
        entries[0].description_override.as_deref(),
        Some("an override")
    );

    let array = lua
        .create_sequence_from(vec![Value::UserData(tool)])
        .expect("array");
    array
        .raw_push(Value::String(
            lua.create_string("web/fetch").expect("string"),
        ))
        .expect("push");
    let entries = collect_offer_entries("tools.offer", Variadic::from_iter([Value::Table(array)]))
        .expect("the array form decodes");
    let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["tools/echo", "web/fetch"],
        "tool objects and ids mix in the array form"
    );
}

#[test]
fn local_params_schema_builds_object_schema_with_required_fields() {
    let lua = Lua::new();
    let params = lua
        .load("{ query = 'string', limit = { 'integer', 'maximum hits' } }")
        .eval::<mlua::Table>()
        .expect("params table evaluates");
    let schema = local_params_schema(&params).expect("the schema builds");
    assert_eq!(
        schema["properties"],
        json!({
            "query": { "type": "string" },
            "limit": { "type": "integer", "description": "maximum hits" },
        })
    );
    assert_eq!(
        schema["required"],
        json!(["limit", "query"]),
        "required is emitted in sorted order, not Lua hash order"
    );
    assert_eq!(schema["type"], "object");
}

#[test]
fn local_params_schema_sorts_required_so_the_schema_text_is_deterministic() {
    // `required` is the one Rust-side `table.pairs` walk whose output is an
    // ordered array rather than a table or map, so it must impose an order
    // itself: left as the walk produced it, two VMs emit different schema
    // text for the same params, and a stored tool definition stops being
    // replayable.
    let lua = Lua::new();
    let params = lua
        .load(
            "{ zulu = 'string', alpha = 'string', mike = 'string', bravo = 'string', \
               yankee = 'string', charlie = 'string', oscar = 'string', delta = 'string' }",
        )
        .eval::<mlua::Table>()
        .expect("params table evaluates");
    let schema = local_params_schema(&params).expect("the schema builds");
    assert_eq!(
        schema["required"],
        json!([
            "alpha", "bravo", "charlie", "delta", "mike", "oscar", "yankee", "zulu"
        ]),
        "required must be sorted, not left in Lua hash order"
    );
}

#[test]
fn local_params_schema_rejects_an_unsupported_type() {
    let lua = Lua::new();
    let params = lua
        .load("{ payload = 'table' }")
        .eval::<mlua::Table>()
        .expect("params table evaluates");
    let error = local_params_schema(&params).expect_err("an unsupported type fails");
    assert!(
        error.to_string().contains("unsupported type \"table\""),
        "the rejection names the bad type: {error}"
    );
}

/// Installs the tools namespace over `set` on a fresh VM and returns it
/// with the shared set and the runtime the namespace records into.
fn lua_over(set: ToolSet) -> (Lua, Arc<Mutex<ToolSet>>, Arc<Mutex<ToolRuntime>>) {
    let lua = Lua::new();
    let globals = lua.globals();
    let set = Arc::new(Mutex::new(set));
    let runtime = Arc::new(Mutex::new(ToolRuntime {
        added: Vec::new(),
        description_overrides: std::collections::BTreeMap::default(),
        allowed_tasks: None,
    }));
    install_tools(
        &lua,
        &globals,
        &set,
        &runtime,
        &crate::vm::LocalTools::default(),
    )
    .expect("the tools install cannot fail on a fresh VM");
    (lua, set, runtime)
}

/// Installs the tools namespace on a fresh VM and returns it with the
/// runtime the namespace records into.
fn lua_with_tools_and_runtime() -> (Lua, Arc<Mutex<ToolRuntime>>) {
    let (lua, _, runtime) = lua_over(ToolSet::default());
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
fn the_tools_namespace_exposes_scoping_without_bind_or_call() {
    // `call` is absent here on purpose: it suspends, so the coroutine shim
    // prelude installs it - this table exposes exactly the non-suspending
    // operations. `bind` is absent too.
    let lua = lua_with_tools();
    let (has_offer, has_offer_local, has_always_offer, call_is_nil, bind_is_nil): (
        bool,
        bool,
        bool,
        bool,
        bool,
    ) = lua
        .load(
            "return type(tools.offer) == 'function', \
                    type(tools.offer_local) == 'function', \
                    type(tools.always_offer) == 'function', \
                    tools.call == nil, \
                    tools.bind == nil",
        )
        .eval()
        .expect("the namespace probe evaluates");
    assert!(has_offer && has_offer_local && has_always_offer && call_is_nil && bind_is_nil);
}

#[test]
fn the_shim_prelude_installs_tools_call_and_no_bare_global() {
    let nonce = GuardNonce::from_seed(1);
    let observer = crate::tests::recording::null_emitter();
    let mut vm =
        SectionVm::new(&nonce, &observer, "Test").expect("section VM construction cannot fail");
    vm.inject_values("", &json!({}), &fresh_access())
        .expect("value injection cannot fail");
    vm.install_coro_shims(24)
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
fn tool_call_counts_seed_by_id_and_explain_an_unseeded_key() {
    let lua = lua_with_tools();
    let bound = ToolSet::for_test(
        Vec::new(),
        Vec::new(),
        vec![
            ToolBinding::from_descriptor("tools_echo", &echo_tool()),
            ToolBinding::from_descriptor("tools_other", &tool_at("tools/other")),
        ],
    );
    let counts =
        install_tool_call_counts(&lua, &bound, &bound.offered()[..1]).expect("the counts install");
    assert_eq!(counts.get("tools/echo").expect("read"), Some(0));
    assert_eq!(
        counts.get("tools_echo").expect("read"),
        None,
        "counts key a catalog tool by its id, never its wire name"
    );
    let read = |key: &str| -> String {
        lua.load(format!(
            "local ok, err = pcall(function() return tools.calls[{key:?}] end); return tostring(err)"
        ))
        .eval::<String>()
        .expect("the unseeded read raises")
    };
    let offered = read("tools/other");
    assert!(
        offered.contains(
            "tools.calls: \"tools/other\" has no seeded count; seeded names: [\"tools/echo\"] \
             (a catalog tool that was neither offered in this section nor called with \
             tools.call)"
        ),
        "an offered id that was never seeded says so: {offered}"
    );
    for key in ["ghost", "tools_echo"] {
        let error = read(key);
        assert!(
            error.contains(&format!("tools.calls: {key:?} has no seeded count"))
                && error.contains(" - check for typos or offer it with tools.offer"),
            "an unknown key names itself and the remedy: {error}"
        );
    }
}

/// A trivial tool as data, so the counts test can bind an id.
fn echo_tool() -> promptforge_types::tools::ToolDescriptor {
    tool_at("tools/echo")
}

/// A tool as data under `id`, described as `<id> tool`.
fn tool_at(id: &str) -> promptforge_types::tools::ToolDescriptor {
    promptforge_types::tools::ToolDescriptor::new(
        ToolId::parse(id).expect("valid id"),
        format!("{id} tool"),
        json!({ "type": "object" }),
    )
}
