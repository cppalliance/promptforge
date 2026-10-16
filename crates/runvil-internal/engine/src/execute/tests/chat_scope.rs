//! Tests for the `Chat` arm's tool scope from a section VM: every round
//! advertises the section's current effective scope plus every local Lua
//! tool.

use runvil_parser::test_support::one_section;

use super::fixtures::echo_tool;
use super::models_loop::loop_scenario;
use super::*;

#[test]
fn a_round_advertises_the_section_scope_with_local_tools() {
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "grab", "{\"value\":\"x\"}")]),
        text("done"),
    ]);
    let source = one_section(
        "tools.offer('tools/echo')
         tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)
           return 'grabbed ' .. args.value
         end)
         local msgs = messages.new()
         msgs:user('use the local tool')
         models.loop(msgs)
         assert(msgs[2].tool_calls[1].name == 'grab', 'a local tool is in the advertised scope')
         assert(msgs[3].role == 'tool', 'the shim dispatched the local tool')
         return 'ok'",
    );
    let trace = loop_scenario(source)
        .offer("echo", echo_tool())
        .chat(&gateway)
        .run();
    assert_eq!(trace.text(), "ok");
    let body = &gateway.requests()[0];
    assert_eq!(
        body.tool_names(),
        ["echo", "grab"],
        "the bound scope then the local tools: {body:?}"
    );
}
