//! Tests for the `Chat` arm's tool scope from a section VM: every round
//! advertises the section's current effective scope plus every local Lua
//! tool.

use super::models_loop::{echo_tools, loop_context, loop_prompt};
use super::*;
use crate::test_support::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn a_round_advertises_the_section_scope_with_local_tools() {
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"x\"}"),
        resp_text("done"),
    ]);
    let md = loop_prompt(
        "tools.offer_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use the local tool')\n\
         models.loop(msgs)\n\
         assert(msgs[2].tool_calls[1].name == 'grab', 'a local tool is in the advertised scope')\n\
         assert(msgs[3].role == 'tool', 'the shim dispatched the local tool')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the section scope includes local tools");
    assert_eq!(out, "ok");
    let body = &gateway.requests()[0];
    assert_eq!(
        body.tool_names(),
        ["echo", "grab"],
        "the bound scope then the local tools: {body:?}"
    );
}
