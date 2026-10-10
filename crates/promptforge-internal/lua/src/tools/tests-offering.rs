//! Tests for the id-keyed `tools` namespace: tool objects, `tools.offer`
//! and `tools.always_offer` over ids and tool objects, the `required` and
//! `extras` lists, `tools.get`, `tools.offer_local`, and the removed
//! slot-era functions.

use std::sync::{Arc, Mutex};

use mlua::{Lua, Value, Variadic};
use promptforge_types::plugins::PluginId;

use super::super::decode::collect_tools_add_entries;
use super::{lua_over, tool_at};
use crate::ToolBinding;
use crate::handles::ToolSet;
use crate::scope::ToolRuntime;

/// A set declaring the Plugin `web` and offering its two tools beside the
/// two of the undeclared Plugin `gh`, in id order, under their wire names.
fn offering_set() -> ToolSet {
    ToolSet::for_test(
        vec![PluginId::parse("web").expect("valid Plugin")],
        Vec::new(),
        vec![
            ToolBinding::from_descriptor("gh_issues_list", &tool_at("gh/issues.list")),
            ToolBinding::from_descriptor("gh_search", &tool_at("gh/search")),
            ToolBinding::from_descriptor("web_fetch", &tool_at("web/fetch")),
            ToolBinding::from_descriptor("web_search", &tool_at("web/search")),
        ],
    )
}

/// The section's scope as `(wire name, model-facing override)` pairs.
fn scoped(
    set: &Arc<Mutex<ToolSet>>,
    runtime: &Arc<Mutex<ToolRuntime>>,
) -> Vec<(String, Option<String>)> {
    let snapshot = set.lock().expect("the set mutex is not poisoned").clone();
    crate::current_tool_bindings(&snapshot, runtime)
        .expect("the scope resolves")
        .iter()
        .map(|binding| {
            (
                binding.alias().to_owned(),
                binding.model_description().map(str::to_owned),
            )
        })
        .collect()
}

/// A scope entry without an override.
fn plain(name: &str) -> (String, Option<String>) {
    (name.to_owned(), None)
}

#[test]
fn a_tool_object_reads_its_id_and_catalog_description_and_refuses_assignment() {
    // The binding carries a prompt-wide override before `install_tools`
    // builds the objects, the state a section VM sees after H1's
    // `tools.always_offer`.
    let mut set = offering_set();
    set.offered
        .iter_mut()
        .find(|binding| binding.alias() == "gh_search")
        .expect("the set offers gh/search")
        .model_description = Some("an H1 prompt-wide override".to_owned());
    let (lua, _, _) = lua_over(set);
    let result: (String, String, bool, String) = lua
        .load(
            "local tool = tools.get('gh/search')\n\
             tools.always_offer(tool, 'a prompt-wide override')\n\
             tools.offer(tool, 'a section override')\n\
             local ok, err = pcall(function() tool.description = 'x' end)\n\
             return tool.id, tool.description, ok, tostring(err)",
        )
        .eval()
        .expect("the probe evaluates");
    assert_eq!(result.0, "gh/search");
    assert_eq!(
        result.1, "gh/search tool",
        "description stays the catalog text whatever override is set"
    );
    assert!(!result.2, "assigning a field is refused");
    assert!(
        result
            .3
            .contains("Tool objects are frozen: cannot assign field \"description\""),
        "the refusal names the field: {}",
        result.3
    );
}

#[test]
fn offer_decodes_ids_tool_objects_and_arrays_into_wire_names() {
    let (lua, set, runtime) = lua_over(offering_set());
    lua.load(
        "tools.offer('web/search')\n\
         tools.offer(tools.get('gh/search'))\n\
         tools.offer({ 'gh/issues.list', tools.get('web/fetch'), 'web/search' })\n\
         tools.offer()",
    )
    .exec()
    .expect("ids, objects, and arrays offer");
    assert_eq!(
        scoped(&set, &runtime),
        vec![
            plain("web_search"),
            plain("gh_search"),
            plain("gh_issues_list"),
            plain("web_fetch"),
        ],
        "each tool enters the scope once, under its wire name, in first-offer order"
    );
}

#[test]
fn offer_and_always_offer_record_nothing_when_any_entry_is_not_a_catalog_tool() {
    for bad in ["gh/missing", "Not An Id", "gh_search", "helper"] {
        let (lua, set, runtime) = lua_over(offering_set());
        lua.load("tools.offer_local('helper', 'a local helper', {}, function() return '' end)")
            .exec()
            .expect("the local tool registers");
        for call in ["offer", "always_offer"] {
            let error: String = lua
                .load(format!(
                    "local ok, err = pcall(tools.{call}, {{ 'web/search', {bad:?} }})\n\
                     assert(not ok, 'the bad entry is refused')\n\
                     return tostring(err)"
                ))
                .eval()
                .expect("the refusal is caught");
            assert!(
                error.contains(&format!(
                    "tools.{call}: {bad:?} is not a catalog tool in this run"
                )),
                "tools.{call} names {bad:?}: {error}"
            );
        }
        assert!(
            scoped(&set, &runtime).is_empty(),
            "a refused call records no entry, the valid one included ({bad:?})"
        );
        assert!(
            set.lock().expect("the set mutex").always().is_empty(),
            "a refused always_offer records nothing ({bad:?})"
        );
    }
}

#[test]
fn offering_a_tool_object_matches_offering_its_id() {
    let by_object = lua_over(offering_set());
    by_object
        .0
        .load("tools.offer(tools.get('gh/search'), 'an override')")
        .exec()
        .expect("the object offers");
    let by_id = lua_over(offering_set());
    by_id
        .0
        .load("tools.offer('gh/search', 'an override')")
        .exec()
        .expect("the id offers");
    assert_eq!(
        scoped(&by_object.1, &by_object.2),
        scoped(&by_id.1, &by_id.2)
    );
    assert_eq!(
        scoped(&by_id.1, &by_id.2),
        vec![("gh_search".to_owned(), Some("an override".to_owned()))]
    );
}

#[test]
fn always_offer_takes_an_array_with_an_extra_and_keeps_override_precedence() {
    let (lua, set, runtime) = lua_over(offering_set());
    lua.load(
        "tools.always_offer({ 'web/search', tools.extras()[1] })\n\
         tools.always_offer('web/fetch', 'prompt-wide fetch')\n\
         tools.always_offer(tools.get('gh/search'), 'prompt-wide search')\n\
         tools.always_offer('web/search')\n\
         tools.offer('gh/search', 'section search')\n\
         tools.offer('web/fetch')",
    )
    .exec()
    .expect("always_offer takes arrays, ids, objects, and extras");
    assert_eq!(
        set.lock().expect("the set mutex").always(),
        ["web_search", "gh_issues_list", "web_fetch", "gh_search"],
        "the prompt-wide offers record wire names once, in first-offer order"
    );
    assert_eq!(
        scoped(&set, &runtime),
        vec![
            plain("web_search"),
            plain("gh_issues_list"),
            ("web_fetch".to_owned(), Some("prompt-wide fetch".to_owned())),
            ("gh_search".to_owned(), Some("section search".to_owned())),
        ],
        "prompt-wide tools come first, each once, and a section override wins \
         over the prompt-wide one"
    );
}

#[test]
fn required_and_extras_split_the_offering_by_declared_plugin_and_share_objects() {
    let (lua, _, _) = lua_over(offering_set());
    let result: (String, String, bool, bool, bool, bool) = lua
        .load(
            "local function ids(list)\n\
               local out = {}\n\
               for i, tool in ipairs(list) do out[i] = tool.id end\n\
               return table.concat(out, ',')\n\
             end\n\
             local required, extras = tools.required(), tools.extras()\n\
             return ids(required), ids(extras), \
               required[1] == tools.get('web/fetch'), \
               extras[2] == tools.get('gh/search'), \
               tools.required() ~= required, \
               tools.required()[2] == required[2]",
        )
        .eval()
        .expect("the lists evaluate");
    assert_eq!(
        result,
        (
            "web/fetch,web/search".to_owned(),
            "gh/issues.list,gh/search".to_owned(),
            true,
            true,
            true,
            true,
        ),
        "a declared Plugin's tools are required, the rest extras, both in id \
         order, every list a fresh array of the same shared objects"
    );
}

#[test]
fn tools_get_reads_one_object_by_id_and_nil_for_anything_else() {
    let (lua, _, _) = lua_over(offering_set());
    let result: (String, bool, bool, bool, bool, String) = lua
        .load(
            "tools.offer_local('helper', 'a local helper', {}, function() return '' end)\n\
             local ok, err = pcall(tools.get, 42)\n\
             assert(not ok, 'a non-string is refused')\n\
             return tools.get('gh/search').id, tools.get('gh/missing') == nil, \
               tools.get('helper') == nil, tools.get('Not An Id') == nil, \
               tools.get('gh_search') == nil, tostring(err)",
        )
        .eval()
        .expect("the probe evaluates");
    assert_eq!(result.0, "gh/search");
    assert!(
        result.1 && result.2 && result.3 && result.4,
        "a miss, a local alias, a malformed id, and a wire name all read nil: {result:?}"
    );
    assert!(
        result.5.contains("tools.get takes a tool id, got integer"),
        "the refusal names the type: {}",
        result.5
    );
}

#[test]
fn offer_local_registers_a_local_tool_and_names_itself_in_its_errors() {
    let (lua, _, _) = lua_over(offering_set());
    let errors: (String, String, String) = lua
        .load(
            "tools.offer_local('gh_search', 'a local search', {}, function() return '' end)\n\
             local _, duplicate = pcall(tools.offer_local, 'gh_search', 'again', {}, function() end)\n\
             local _, param = pcall(tools.offer_local, 'other', 'x', { p = 'table' }, function() end)\n\
             local _, alias = pcall(tools.offer_local, 'web/search', 'x', {}, function() end)\n\
             return tostring(duplicate), tostring(param), tostring(alias)",
        )
        .eval()
        .expect("a local tool may take an offered wire name");
    assert!(
        errors
            .0
            .contains("tools.offer_local alias \"gh_search\" is already registered"),
        "{}",
        errors.0
    );
    assert!(
        errors
            .1
            .contains("tools.offer_local param \"p\" has unsupported type \"table\""),
        "{}",
        errors.1
    );
    assert!(
        errors.2.contains("invalid alias \"web/search\""),
        "a local alias keeps the alias grammar: {}",
        errors.2
    );
}

#[test]
fn the_slot_era_functions_are_not_installed() {
    let (lua, _, _) = lua_over(offering_set());
    let result: (bool, bool, bool, bool, String) = lua
        .load(
            "local kinds = {}\n\
             for _, name in ipairs({ 'offer', 'always_offer', 'offer_local', 'required', \
               'extras', 'get', 'allow_tasks' }) do kinds[#kinds + 1] = type(tools[name]) end\n\
             return tools.add == nil, tools.always == nil, tools.offered == nil, \
               tools.add_local == nil, table.concat(kinds, ',')",
        )
        .eval()
        .expect("the namespace probe evaluates");
    assert_eq!(
        result,
        (
            true,
            true,
            true,
            true,
            "function,function,function,function,function,function,function".to_owned(),
        )
    );
}

#[test]
fn tools_entries_name_the_calling_function_in_every_shape_error() {
    let lua = Lua::new();
    let string = |text: &str| Value::String(lua.create_string(text).expect("string"));
    let array = lua
        .create_sequence_from(vec![string("web/search")])
        .expect("array");
    for (args, fragment) in [
        (
            vec![Value::Table(array), string("x")],
            "tools.always_offer array form takes no override",
        ),
        (
            vec![Value::Integer(42)],
            "tools.always_offer takes tool ids, tool objects, or arrays of them, got integer",
        ),
        (
            vec![string("web/search"), Value::Integer(42)],
            "tools.always_offer override must be a string, got integer",
        ),
        (
            vec![string("web/search"), string("x"), string("y")],
            "tools.always_offer takes one tool plus an optional override, got extra string",
        ),
    ] {
        let error = collect_tools_add_entries("tools.always_offer", Variadic::from_iter(args))
            .err()
            .expect("the shape is refused");
        assert!(error.to_string().contains(fragment), "{fragment}: {error}");
    }
}
