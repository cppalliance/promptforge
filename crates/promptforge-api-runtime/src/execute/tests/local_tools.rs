//! Tests for `tools.add_local`: the registration rules run end to end, and
//! the `models.loop` shim's local-tool rounds are driven at prompt level,
//! so a model-issued call to a local tool is answered on the section VM
//! and its trusted result is sent back to the model verbatim.

use super::models_loop::{loop_context, loop_context_observed, loop_prompt};
use super::run;
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

/// The `grab` local tool registration the loop tests open with, followed
/// by one loop over a single user message; `handler` is the Lua body of
/// the handler function, given `args`.
fn grab_loop(handler: &str) -> String {
    loop_prompt(&format!(
        "tools.add_local('grab', 'Grab a value', {{ value = 'string' }}, function(args)\n\
           {handler}\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tool.')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content"
    ))
}

#[tokio::test(flavor = "current_thread")]
async fn local_tool_handler_result_returns_to_the_model() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let prompt = parse(&grab_loop("return 'got ' .. args.value"));
    let (ctx, host) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the local handler answers the model's call");
    assert_eq!(out, "final answer");

    let bodies = gateway.requests();
    let function = &bodies[0]["tools"][0]["function"];
    assert_eq!(function["name"], "grab");
    assert_eq!(function["description"], "Grab a value");
    assert_eq!(
        function["parameters"],
        json!({
            "type": "object",
            "properties": { "value": { "type": "string" } },
            "required": ["value"]
        })
    );
    // The handler's trusted return reaches the model verbatim (no guard wrap).
    assert_eq!(last_tool_turn_content(&bodies), "got hi");
}

#[tokio::test(flavor = "current_thread")]
async fn local_tool_multiple_calls_in_one_response_all_run() {
    let gateway = ScriptedGateway::start(vec![
        resp_two_tool_calls(
            "grab",
            ("c1", "{\"value\":\"a\"}"),
            ("c2", "{\"value\":\"b\"}"),
        ),
        resp_text("final answer"),
    ])
    .await;
    let md = loop_prompt(
        "local calls = {}\n\
         tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           calls[#calls + 1] = args.value\n\
           return 'ok ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tool.')\n\
         models.loop(msgs)\n\
         return table.concat(calls, ',') .. '|' .. msgs[#msgs].content",
    );
    let prompt = parse(&md);
    let (ctx, host) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("both calls in the one response run");
    assert_eq!(
        out, "a,b|final answer",
        "both calls in the one response must run, in order"
    );

    let bodies = gateway.requests();
    let tool_turns = bodies[1]["messages"]
        .as_array()
        .expect("a request body must include a messages array")
        .iter()
        .filter(|m| m["role"] == "tool")
        .count();
    assert_eq!(
        tool_turns, 2,
        "both handler results must go back: {bodies:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn local_tool_handler_error_surfaces_as_a_tool_failure() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let prompt = parse(&grab_loop("error('handler exploded')"));
    let recorder = Arc::new(Recorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let error = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a handler Lua error must fail the tool call");
    assert!(
        error.to_string().contains("handler exploded"),
        "the handler's error must surface: {error}"
    );
    assert!(
        recorder
            .events()
            .contains(&("Only".to_string(), detail::TOOL_CALL_FAILED.to_string())),
        "the failed handler must be observed as a tool-call failure"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the author's own program failing ends the loop before another round"
    );
}

#[tokio::test]
async fn local_tool_alias_cannot_shadow_a_declared_tool() {
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  grab: tests/tools/concrete\nmodels:\n  writer: {}\n---\n\n\
# Test prompt\n\n```lua\n\
models.default('writer')\n```\n\n\
## Only\n\n\
```lua\n\
tools.add_local('grab', 'Local grab', {}, function() return 'local' end)\n\
```\n",
    );

    let error = run(
        &prompt,
        "",
        &[tool as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .expect_err("a local alias must not shadow a declared tool");
    assert!(
        error.to_string().contains("duplicates a bound tool slot"),
        "the error must identify the bound-slot collision: {error}"
    );
}

#[tokio::test]
async fn local_tool_alias_cannot_be_registered_twice() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n\
```lua\n\
tools.add_local('grab', 'First grab', {}, function() return 'first' end)\n\
tools.add_local('grab', 'Second grab', {}, function() return 'second' end)\n\
```\n";
    let error = run_offline(md)
        .await
        .expect_err("a local alias must not be registered twice");
    assert!(
        error.to_string().contains("is already registered"),
        "the error must identify the duplicate local alias: {error}"
    );
}
