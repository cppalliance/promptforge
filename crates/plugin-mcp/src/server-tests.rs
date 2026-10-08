//! Tests for the tool-name rule and the descriptors `catalog` builds.

use promptforge_plugin::PluginId;
use rmcp::model::Tool;
use serde_json::{Value, json};

use super::catalog;

fn tool(name: &str, extra: &Value) -> Tool {
    let mut value = json!({ "name": name, "inputSchema": { "type": "object" } });
    for (key, member) in extra.as_object().expect("extra is an object") {
        value[key] = member.clone();
    }
    serde_json::from_value(value).expect("a tool parses")
}

fn plain(name: &str) -> Tool {
    tool(name, &json!({}))
}

fn ids(plugin: &str, tools: Vec<Tool>) -> Vec<String> {
    let plugin = PluginId::parse(plugin).expect("a Plugin name parses");
    catalog(&plugin, tools)
        .iter()
        .map(|listed| listed.descriptor.id.to_string())
        .collect()
}

#[test]
fn a_tool_name_is_lowercased_under_the_plugin_name() {
    assert_eq!(
        ids(
            "papers",
            vec![plain("Get_Paper"), plain("list-mailings"), plain("v1.2")]
        ),
        ["papers/get_paper", "papers/list-mailings", "papers/v1.2"]
    );
}

#[test]
fn the_original_name_is_kept_for_the_call() {
    let plugin = PluginId::parse("papers").expect("parses");
    let listed = catalog(&plugin, vec![plain("Get_Paper")]);
    assert_eq!(listed[0].mcp_name, "Get_Paper");
}

#[test]
fn a_name_that_is_still_invalid_after_lowercasing_is_dropped() {
    assert_eq!(
        ids(
            "papers",
            vec![
                plain("has space"),
                plain("keep"),
                plain("sym@bol"),
                plain(""),
                plain("café")
            ]
        ),
        ["papers/keep"]
    );
}

#[test]
fn two_names_that_collide_after_lowercasing_keep_the_first() {
    let plugin = PluginId::parse("papers").expect("parses");
    let listed = catalog(
        &plugin,
        vec![plain("Search"), plain("other"), plain("SEARCH")],
    );
    let kept: Vec<_> = listed.iter().map(|l| l.mcp_name.as_str()).collect();
    assert_eq!(kept, ["Search", "other"]);
}

#[test]
fn the_description_is_the_description_else_the_title_else_the_name() {
    let plugin = PluginId::parse("papers").expect("parses");
    let listed = catalog(
        &plugin,
        vec![
            tool("a", &json!({ "description": "does a", "title": "A" })),
            tool("b", &json!({ "title": "Title B" })),
            plain("c"),
        ],
    );
    let described: Vec<_> = listed
        .iter()
        .map(|l| l.descriptor.description.as_str())
        .collect();
    assert_eq!(described, ["does a", "Title B", "c"]);
}

#[test]
fn the_parameters_are_the_input_schema_and_the_tool_is_plain_and_stoppable() {
    let plugin = PluginId::parse("papers").expect("parses");
    let schema = json!({ "type": "object", "properties": { "id": { "type": "string" } }, "required": ["id"] });
    let listed = catalog(
        &plugin,
        vec![tool("get", &json!({ "inputSchema": schema }))],
    );
    let descriptor = &listed[0].descriptor;
    assert_eq!(descriptor.parameters_schema, schema);
    assert!(!descriptor.structured_output);
    assert!(!descriptor.survives_stop);
}
