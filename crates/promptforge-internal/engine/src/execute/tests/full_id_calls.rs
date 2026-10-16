//! Tests for script tool calls by canonical id: a script's `tools.call`
//! names an offered tool by its id and records the tool's wire name, and
//! reaches by id a catalog tool the run could not offer, recording the id.
//! A call binds no global and advertises nothing; a wire name is the
//! model's alone, and a model-issued call never resolves an id.

use promptforge_parser::test_support::one_section;

use super::models_loop::catalog_scenario;
use super::*;
use crate::execute::run::{EffectRecord, ToolCallOrigin, ToolCaller};
use crate::test_support::rig::Rig;

/// The echo fixture's full id.
const ECHO_ID: &str = "tools/echo";

/// The echo fixture's wire name.
const ECHO_WIRE: &str = "tools_echo";

/// The echo fixture's full id as a [`ToolId`].
fn echo_id() -> ToolId {
    ToolId::parse(ECHO_ID).expect("valid id")
}

/// The origin of a script call made in the fixture prompt's one section.
fn script_origin() -> ToolCallOrigin {
    ToolCallOrigin::new(EXECUTION, "Only", ToolCaller::Script)
}

/// A run of the one section `lua` prepared against a catalog of the echo
/// fixture and a second tool the prompts never offer.
#[track_caller]
fn echo_and_concrete(lua: &str) -> Scenario {
    catalog_scenario(one_section(lua))
        .tool(echo_tool())
        .tool(scoped_tool("concrete", "Concrete description.").0)
}

/// A catalog tool whose wire name, `task_cancel`, is a task built-in's, so
/// the run cannot offer it; it echoes its `value` argument.
#[track_caller]
fn unofferable_tool() -> FnTool {
    let schema = json!({ "type": "object", "properties": { "value": { "type": "string" } } });
    let tool = FnTool::new("task/cancel", schema, |args| {
        let value = args["value"].as_str().unwrap_or_default();
        Ok(ToolOutput::trusted(format!("unoffered: {value}")))
    });
    tool.description("A tool the run cannot offer.")
}

#[test]
fn a_script_call_by_id_records_the_offered_tools_wire_name() {
    let source = one_section("return tools.call('tools/echo', { value = 'hi' })");
    let rig = Rig::run(catalog_scenario(source).tool(echo_tool()));
    assert_eq!(rig.trace().text(), "echoed: hi");
    assert_eq!(
        rig.issued(),
        vec![EffectRecord::ToolCall {
            tool: echo_id(),
            alias: ECHO_WIRE.to_owned(),
            args: json!({ "value": "hi" }),
            origin: script_origin(),
        }]
    );
}

#[test]
fn a_script_reaches_a_catalog_tool_left_out_of_the_offering_by_id() {
    let source = one_section(
        "assert(tools.get('task/cancel') == nil, 'the tool is not offered')\n\
         return tools.call('task/cancel', { value = 'hi' })",
    );
    let mut rig = Rig::start(catalog_scenario(source).tool(unofferable_tool()));
    assert!(
        rig.scheduler_for_test()
            .tool_set_for_test()
            .offered()
            .is_empty(),
        "a tool whose wire name is a task built-in's is left out of the offering"
    );
    let rig = rig.finish();
    assert_eq!(rig.trace().text(), "unoffered: hi");
    assert_eq!(
        rig.issued(),
        vec![EffectRecord::ToolCall {
            tool: ToolId::parse("task/cancel").expect("valid id"),
            alias: "task/cancel".to_owned(),
            args: json!({ "value": "hi" }),
            origin: script_origin(),
        }],
        "the id stands as the alias of a tool the run could not offer"
    );
}

#[test]
fn a_script_call_advertises_nothing_until_the_tool_is_offered() {
    let gateway = ScriptedChat::new(vec![text("before"), text("after")]);
    let rig = Rig::run(
        echo_and_concrete(
            "tools.call('tools/echo', { value = 'a' })\n\
             tools.call('tools/concrete', { value = 'b' })\n\
             local before = messages.new()\n\
             before:user('before')\n\
             models.loop(before)\n\
             tools.offer('tools/echo')\n\
             local after = messages.new()\n\
             after:user('after')\n\
             models.loop(after)\n\
             return 'ok'",
        )
        .chat(&gateway),
    );
    assert_eq!(rig.trace().text(), "ok");
    let advertised: Vec<Vec<String>> = rig
        .issued()
        .iter()
        .filter_map(|record| match record {
            EffectRecord::Chat { tools, .. } => Some(tools.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        advertised,
        vec![Vec::new(), vec![ECHO_WIRE.to_owned()]],
        "a script call scopes nothing; only the offered tool is advertised"
    );
}

#[test]
fn a_script_call_by_wire_name_or_local_looking_name_is_unbound() {
    let source = one_section(
        "local wire_ok, wire_err = pcall(tools.call, 'tools_echo', {})\n\
         local bare_ok, bare_err = pcall(tools.call, 'echo', {})\n\
         assert(not wire_ok and not bare_ok, 'neither name is a catalog id')\n\
         return wire_err.kind .. ':' .. wire_err.name .. '|' .. bare_err.kind .. ':' .. bare_err.name",
    );
    let rig = Rig::run(catalog_scenario(source).tool(echo_tool()));
    assert_eq!(
        rig.trace().text(),
        "unbound_tool:tools_echo|unbound_tool:echo"
    );
    assert!(rig.issued().is_empty(), "nothing was issued");
}

#[test]
fn no_global_is_bound_under_a_wire_name_or_an_id() {
    let rig = Rig::run(echo_and_concrete(
        "tools.offer('tools/echo')\n\
         tools.call('tools/concrete', { value = 'x' })\n\
         local named = {}\n\
         for name in next, _G do\n\
           if type(name) == 'string' and (name:find('/', 1, true) or name:find('tools_', 1, true)) then\n\
             named[#named + 1] = name\n\
           end\n\
         end\n\
         table.sort(named)\n\
         return type(tools_echo) .. '|' .. table.concat(named, ',')",
    ));
    assert_eq!(
        rig.trace().text(),
        "nil|",
        "no tool is a global under any name"
    );
}

#[test]
fn a_script_call_by_id_and_by_tool_object_issue_the_same_effect() {
    let source = one_section(
        "return tools.call('tools/echo', { value = 'hi' }) .. '|' .. \
         tools.call(tools.get('tools/echo'), { value = 'hi' })",
    );
    let rig = Rig::run(catalog_scenario(source).tool(echo_tool()));
    assert_eq!(rig.trace().text(), "echoed: hi|echoed: hi");
    let call = EffectRecord::ToolCall {
        tool: echo_id(),
        alias: ECHO_WIRE.to_owned(),
        args: json!({ "value": "hi" }),
        origin: script_origin(),
    };
    assert_eq!(rig.issued(), vec![call.clone(), call]);
}

#[test]
fn a_name_that_is_no_catalog_id_is_unbound_and_lists_the_offered_ids() {
    let rig = Rig::run(echo_and_concrete("return tools.call('tools/missing', {})"));
    let error = rig.trace().error().inner();
    match error {
        Error::UnboundToolCall { name, ids } => {
            assert_eq!(name, "tools/missing");
            assert_eq!(
                ids,
                &["tools/concrete".to_owned(), ECHO_ID.to_owned()],
                "every offered tool's id is listed"
            );
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
    assert_eq!(
        error.to_string(),
        "tool \"tools/missing\" is not a tool in this run; \
         catalog tools: [\"tools/concrete\", \"tools/echo\"]",
        "the message lists the offered ids"
    );
    assert!(rig.issued().is_empty(), "nothing was issued");
}

#[test]
fn a_model_issued_call_never_resolves_an_id() {
    let source = one_section(
        "local ok, err = pcall(tools.call_as_model, 'call_1', 'tools/echo', { value = 'hi' })\n\
         assert(not ok, 'a model-issued id call is refused')\n\
         return err.kind .. ':' .. err.name",
    );
    let rig = Rig::run(catalog_scenario(source).tool(echo_tool()).raw_shims());
    assert_eq!(rig.trace().text(), "unbound_tool:tools/echo");
    assert!(rig.issued().is_empty(), "nothing was issued");
}
