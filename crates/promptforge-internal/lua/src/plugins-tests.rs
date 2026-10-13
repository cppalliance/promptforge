//! Tests for the `plugins` table: the `required` and `extras` lists,
//! `plugins.get`, the shared Plugin objects and their `tools`, and a tool
//! object's `plugin` getter.

use std::sync::{Arc, Mutex};

use mlua::Lua;
use promptforge_types::plugins::PluginId;
use promptforge_types::tools::{ToolDescriptor, ToolId};
use serde_json::json;

use super::install_plugins;
use crate::ToolBinding;
use crate::handles::ToolSet;
use crate::scope::ToolRuntime;
use crate::tools::install_tools;

/// A tool as data under `id`.
fn tool_at(id: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        ToolId::parse(id).expect("valid id"),
        format!("{id} tool"),
        json!({ "type": "object" }),
    )
}

/// A set declaring `web` and then `mail`, which offers no tool, beside the
/// undeclared Plugins `alpha` and `gh`, every tool in id order under its
/// wire name.
fn plugin_set() -> ToolSet {
    let bind = |id: &str| ToolBinding::from_descriptor(&id.replace(['/', '.'], "_"), &tool_at(id));
    ToolSet {
        declared: vec![
            PluginId::parse("web").expect("valid Plugin"),
            PluginId::parse("mail").expect("valid Plugin"),
        ],
        offered: vec![
            bind("alpha/zed"),
            bind("gh/issues.list"),
            bind("gh/search"),
            bind("web/fetch"),
            bind("web/search"),
        ],
    }
}

/// Installs `tools` and then `plugins` over `set` on a fresh VM, the
/// section setup order, and returns it with the runtime `tools.offer` and
/// `plugins.offer` record into and the shared set.
fn lua_over(set: ToolSet) -> (Lua, Arc<ToolSet>, Arc<Mutex<ToolRuntime>>) {
    let lua = Lua::new();
    let globals = lua.globals();
    let set = Arc::new(set);
    let runtime = Arc::new(Mutex::new(ToolRuntime::default()));
    install_tools(
        &lua,
        &globals,
        &set,
        &runtime,
        &crate::vm::LocalTools::default(),
    )
    .expect("the tools install on a fresh VM");
    install_plugins(&lua, &globals, &set, &runtime).expect("the plugins install on a fresh VM");
    (lua, set, runtime)
}

/// The Plugins `plugins.offer` recorded in `runtime`, by name.
fn offered_plugins(runtime: &Mutex<ToolRuntime>) -> Vec<String> {
    runtime
        .lock()
        .expect("the runtime mutex is not poisoned")
        .plugins
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// The wire names a round over `set` and `runtime` advertises, in order.
fn scoped(set: &ToolSet, runtime: &Mutex<ToolRuntime>) -> Vec<String> {
    crate::current_tool_bindings(set, runtime)
        .expect("the scope resolves")
        .iter()
        .map(|binding| binding.alias().to_owned())
        .collect()
}

/// A Lua helper joining the `name` of each Plugin object in a list.
const NAMES: &str = "local function names(list)\n\
       local out = {}\n\
       for i, plugin in ipairs(list) do out[i] = plugin.name end\n\
       return table.concat(out, ',')\n\
     end\n";

#[test]
fn required_lists_every_declared_plugin_in_name_order_and_extras_the_rest() {
    let (lua, _, _) = lua_over(plugin_set());
    let result: (String, String, i64, String, bool, bool, String) = lua
        .load(format!(
            "{NAMES}\
             local ok, err = pcall(plugins.get, 42)\n\
             assert(not ok, 'a non-string is refused')\n\
             return names(plugins.required()), names(plugins.extras()), \
               #plugins.get('mail').tools, plugins.get('gh').name, \
               plugins.get('missing') == nil, plugins.get('web/fetch') == nil, \
               tostring(err)"
        ))
        .eval()
        .expect("the probe evaluates");
    assert_eq!(
        result.0, "mail,web",
        "every declared Plugin is required, in name order"
    );
    assert_eq!(
        result.1, "alpha,gh",
        "every other Plugin with offered tools is an extra, in name order"
    );
    assert_eq!(result.2, 0, "a declared Plugin with no tool has an object");
    assert_eq!(result.3, "gh", "plugins.get reads an extra by name");
    assert!(
        result.4 && result.5,
        "an unknown name and a tool id read nil: {result:?}"
    );
    assert!(
        result
            .6
            .contains("plugins.get takes a Plugin name, got integer"),
        "the refusal names the type: {}",
        result.6
    );
}

#[test]
fn plugins_get_returns_the_one_object_required_and_extras_hand_out() {
    let (lua, _, _) = lua_over(plugin_set());
    let result: (bool, bool, bool, bool, bool) = lua
        .load(
            "local required = plugins.required()\n\
             return plugins.get('web') == plugins.get('web'), \
               required[2] == plugins.get('web'), \
               plugins.extras()[2] == plugins.get('gh'), \
               plugins.required() ~= required, \
               plugins.extras() ~= plugins.extras()",
        )
        .eval()
        .expect("the probe evaluates");
    assert_eq!(
        result,
        (true, true, true, true, true),
        "one object per Plugin, handed out in a fresh array per list call"
    );
}

#[test]
fn a_plugins_tools_are_the_shared_tool_objects_in_a_fresh_array_that_offer_takes() {
    let (lua, set, runtime) = lua_over(plugin_set());
    let result: (i64, bool, bool, bool) = lua
        .load(
            "local web = plugins.get('web')\n\
             local tools_list = web.tools\n\
             tools.offer(web.tools)\n\
             return #tools_list, tools_list[1] == tools.get('web/fetch'), \
               tools_list[2] == tools.get('web/search'), web.tools ~= tools_list",
        )
        .eval()
        .expect("the probe evaluates");
    assert_eq!(
        result,
        (2, true, true, true),
        "a Plugin's tools are its shared tool objects, in id order, in a new array per read"
    );
    let scoped: Vec<String> = crate::current_tool_bindings(&set, &runtime)
        .expect("the scope resolves")
        .iter()
        .map(|binding| binding.alias().to_owned())
        .collect();
    assert_eq!(
        scoped,
        ["web_fetch", "web_search"],
        "tools.offer takes a Plugin's tools array"
    );
}

#[test]
fn assigning_a_field_on_a_plugin_object_is_refused() {
    let (lua, _, _) = lua_over(plugin_set());
    let (name, extra, read): (String, String, String) = lua
        .load(
            "local web = plugins.get('web')\n\
             local _, name = pcall(function() web.name = 'x' end)\n\
             local _, extra = pcall(function() web.extra = 1 end)\n\
             return tostring(name), tostring(extra), web.name",
        )
        .eval()
        .expect("the refusals are caught");
    assert!(
        name.contains("Plugin objects are frozen: cannot assign field \"name\""),
        "the refusal names the field: {name}"
    );
    assert!(
        extra.contains("Plugin objects are frozen: cannot assign field \"extra\""),
        "a new field is refused too: {extra}"
    );
    assert_eq!(read, "web", "the object keeps its name");
}

#[test]
fn every_tool_objects_plugin_is_its_plugins_object() {
    let (lua, _, _) = lua_over(plugin_set());
    let mismatched: String = lua
        .load(
            "local bad = {}\n\
             for _, list in ipairs({ tools.required(), tools.extras() }) do\n\
               for _, tool in ipairs(list) do\n\
                 local name = tool.id:match('^[^/]+')\n\
                 if tool.plugin ~= plugins.get(name) or tool.plugin.name ~= name then\n\
                   bad[#bad + 1] = tool.id\n\
                 end\n\
               end\n\
             end\n\
             assert(#tools.required() + #tools.extras() == 5, 'every tool is walked')\n\
             return table.concat(bad, ',')",
        )
        .eval()
        .expect("the probe evaluates");
    assert_eq!(
        mismatched, "",
        "each tool object's plugin is == to its Plugin's object"
    );
}

#[test]
fn offer_records_each_plugin_once_from_a_name_an_object_or_a_list() {
    let (lua, _, runtime) = lua_over(plugin_set());
    lua.load(
        "plugins.offer('gh')\n\
         plugins.offer(plugins.get('alpha'))\n\
         plugins.offer({ 'gh', plugins.get('web') })\n\
         plugins.offer(plugins.required())\n\
         plugins.offer({})\n\
         assert(select('#', plugins.offer('gh')) == 0, 'the verb returns nothing')",
    )
    .exec()
    .expect("every accepted shape offers");
    assert_eq!(
        offered_plugins(&runtime),
        ["gh", "alpha", "web", "mail"],
        "each Plugin is recorded once, in first-offer order"
    );
    assert!(
        runtime
            .lock()
            .expect("the runtime mutex is not poisoned")
            .added
            .is_empty(),
        "a Plugin offer never records a wire name"
    );
}

#[test]
fn offer_refuses_a_wrong_or_extra_argument_and_an_absent_plugin_and_records_nothing() {
    let (lua, _, runtime) = lua_over(plugin_set());
    let shape = "plugins.offer takes a Plugin name, a Plugin object, or a list of them, got";
    let absent = "plugins.offer: no Plugin named \"mcp\" in this run; declare it under \
                  plugins: to require it, or offer it only when plugins.get(\"mcp\") returns it";
    for (call, expected) in [
        ("plugins.offer(nil)", format!("{shape} nil")),
        ("plugins.offer()", format!("{shape} nil")),
        ("plugins.offer(42)", format!("{shape} integer")),
        (
            "plugins.offer(tools.get('web/fetch'))",
            format!("{shape} userdata"),
        ),
        ("plugins.offer({ 'web', 42 })", format!("{shape} integer")),
        (
            "plugins.offer('web/fetch')",
            format!("{shape} \"web/fetch\""),
        ),
        (
            "plugins.offer('web', 'gh')",
            "plugins.offer takes one Plugin or a list of them, got extra string".to_owned(),
        ),
        ("plugins.offer('mcp')", absent.to_owned()),
        ("plugins.offer({ 'web', 'mcp' })", absent.to_owned()),
    ] {
        let error: String = lua
            .load(format!(
                "local ok, err = pcall(function() {call} end)\n\
                 assert(not ok, 'the call is refused')\n\
                 return tostring(err)"
            ))
            .eval()
            .unwrap_or_else(|error| panic!("{call}: {error}"));
        assert!(error.contains(&expected), "{call}: {error}");
    }
    assert!(
        offered_plugins(&runtime).is_empty(),
        "a refused call records no Plugin"
    );
}

#[test]
fn the_scope_lists_section_tools_then_each_offered_plugins_tools_each_name_once() {
    let (lua, set, runtime) = lua_over(plugin_set());
    lua.load(
        "tools.offer('web/search')\n\
         plugins.offer({ 'gh', 'web', 'alpha' })",
    )
    .exec()
    .expect("the offers record");
    let before = scoped(&set, &runtime);
    assert_eq!(
        before,
        [
            "web_search",
            "gh_issues_list",
            "gh_search",
            "web_fetch",
            "alpha_zed"
        ],
        "section tools first, then Plugins in offer order with tools in id order, \
         each name at its first place"
    );
    lua.load("plugins.offer('mail')")
        .exec()
        .expect("a declared Plugin with no tools is accepted");
    assert_eq!(
        scoped(&set, &runtime),
        before,
        "a Plugin with no tools adds nothing"
    );
}
