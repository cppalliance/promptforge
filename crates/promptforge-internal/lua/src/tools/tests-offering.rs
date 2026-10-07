//! Tests for the offering in the `tools` namespace: `tools.offered()`
//! records, records in `tools.add` and `tools.call`, and the operations
//! that stay on the frontmatter's slots.

use mlua::{Lua, Value, Variadic};

use super::super::decode::{collect_tools_add_entries, tool_alias};
use super::{echo_tool, lua_over, tool_at};
use crate::ToolBinding;
use crate::handles::ToolSet;

/// A set declaring `echo` and offering the two tools of the undeclared
/// Plugin `gh`, in id order, under their model-facing names.
fn offering_set() -> ToolSet {
    ToolSet::for_test(
        vec![ToolBinding::for_test("echo", "echo tool", &echo_tool())],
        Vec::new(),
        vec![
            ToolBinding::from_descriptor("gh_issues_list", &tool_at("gh/issues.list")),
            ToolBinding::from_descriptor("gh_search", &tool_at("gh/search")),
        ],
    )
}

#[test]
fn tool_alias_reads_the_name_off_an_offered_tool_record() {
    let lua = Lua::new();
    let record = lua
        .load("{ id = 'gh/search', name = 'gh_search', plugin = 'gh' }")
        .eval::<mlua::Table>()
        .expect("the record evaluates");
    assert_eq!(
        tool_alias(&Value::Table(record)).expect("a record decodes"),
        "gh_search"
    );
    let nameless = lua
        .load("{ 'gh_search' }")
        .eval::<mlua::Table>()
        .expect("the list evaluates");
    let error = tool_alias(&Value::Table(nameless))
        .expect_err("a table without a string name is not a record");
    assert!(
        error
            .to_string()
            .contains("tools.call alias must be a string, Tool object, or tool record, got table"),
        "the rejection names the record form: {error}"
    );
}

#[test]
fn tools_add_entries_read_an_offered_record_as_one_entry_and_mix_records_in_a_list() {
    let lua = Lua::new();
    let record = lua
        .load("{ name = 'gh_search' }")
        .eval::<mlua::Table>()
        .expect("the record evaluates");
    let entries = collect_tools_add_entries(Variadic::from_iter([
        Value::Table(record.clone()),
        Value::String(lua.create_string("an override").expect("string")),
    ]))
    .expect("a record takes an override like a single alias");
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].alias, "gh_search");
    assert_eq!(
        entries[0].description_override.as_deref(),
        Some("an override")
    );
    let list = lua
        .create_sequence_from(vec![
            Value::Table(record),
            Value::String(lua.create_string("echo").expect("string")),
        ])
        .expect("list");
    let entries = collect_tools_add_entries(Variadic::from_iter([Value::Table(list)]))
        .expect("a list of records and aliases decodes");
    let aliases: Vec<&str> = entries.iter().map(|entry| entry.alias.as_str()).collect();
    assert_eq!(aliases, vec!["gh_search", "echo"]);
}

#[test]
fn tools_offered_returns_a_fresh_plain_record_per_offered_tool_in_order() {
    let (lua, _, _) = lua_over(offering_set());
    let result: (i64, String, String, String, String, String, bool, String) = lua
        .load(
            "local offered = tools.offered()\n\
             local first = offered[1]\n\
             local name = first.name\n\
             first.name = 'changed'\n\
             return #offered, first.id, name, first.plugin, first.description, \
               offered[2].name, getmetatable(first) == nil, tools.offered()[1].name",
        )
        .eval()
        .expect("tools.offered evaluates");
    assert_eq!(
        result,
        (
            2,
            "gh/issues.list".to_owned(),
            "gh_issues_list".to_owned(),
            "gh".to_owned(),
            "gh/issues.list tool".to_owned(),
            "gh_search".to_owned(),
            true,
            "gh_issues_list".to_owned(),
        ),
        "one plain record per offered tool, the declared slot left out, and a \
         write to a record never reaching the next call"
    );
}

#[test]
fn tools_add_takes_an_offered_name_a_record_and_the_whole_offering() {
    let (lua, set, runtime) = lua_over(offering_set());
    lua.load("tools.add(tools.offered()[2], 'search override')")
        .exec()
        .expect("a record adds with an override");
    lua.load("tools.add(tools.offered())")
        .exec()
        .expect("the offering adds as a list");
    lua.load("tools.add('gh_search')")
        .exec()
        .expect("an offered name adds");
    let error = lua
        .load("tools.add({ name = 'ghost' })")
        .exec()
        .expect_err("a record naming no tool is refused");
    assert!(
        error
            .to_string()
            .contains("tools.add alias \"ghost\" is neither a bound tool slot nor an offered tool"),
        "a record goes through the same check as a name: {error}"
    );
    let snapshot = set.lock().expect("the set mutex is not poisoned").clone();
    let scope = crate::current_tool_bindings(&snapshot, &runtime)
        .expect("the offered names resolve in the section's scope");
    let scoped: Vec<(&str, &str, Option<&str>)> = scope
        .iter()
        .map(|binding| {
            (
                binding.alias(),
                binding.description(),
                binding.model_description(),
            )
        })
        .collect();
    assert_eq!(
        scoped,
        vec![
            ("gh_search", "gh/search tool", Some("search override")),
            ("gh_issues_list", "gh/issues.list tool", None),
        ]
    );
}

#[test]
fn tools_always_and_add_local_stay_on_declared_slots_not_the_offering() {
    let (lua, _, _) = lua_over(offering_set());
    let error = lua
        .load("tools.always('gh_search')")
        .exec()
        .expect_err("an offered name is not a declared slot");
    assert!(
        error
            .to_string()
            .contains("tools.always alias \"gh_search\" is not a bound tool slot"),
        "tools.always reads the declared slots alone: {error}"
    );
    lua.load("tools.add_local('gh_search', 'a local search', {}, function() return '' end)")
        .exec()
        .expect("an offered name does not block a local tool");
}
