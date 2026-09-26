//! Tests for `tools.add_local`: the registration rules run end to end, and
//! the `models.loop` shim's local-tool rounds are driven at prompt level,
//! so a model-issued call to a local tool runs its handler inside the
//! block coroutine - store calls included - and its trusted result is sent
//! back to the model verbatim. `jump` is withheld only while a handler
//! runs.

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

#[tokio::test(flavor = "current_thread")]
async fn a_loop_handler_writes_and_reads_the_store_and_the_model_gets_the_text() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let prompt = parse(&grab_loop(
        "store.write('grab.txt', 'kept ' .. args.value)\n\
           return store.read('grab.txt')",
    ));
    let (ctx, host) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the handler's store calls suspend and resume inside the loop");
    assert_eq!(out, "final answer");
    assert_eq!(
        last_tool_turn_content(&gateway.requests()),
        "kept hi",
        "the text the handler read back from the store reaches the model"
    );
}

/// The two-section shell the jump tests drive: `lua` runs in `## Only`,
/// and `## Other` returns a fixed marker when a jump lands there.
fn jump_prompt(lua: &str) -> String {
    format!(
        "---\nname: loop\ndescription: d\npromptforge: 0\n---\n\n# Loop\n\n## Only\n\n```lua\n{lua}\n```\n\n\
         ## Other\n\n```lua\nreturn 'jumped'\n```\n"
    )
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_that_calls_jump_fails_the_run() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let prompt = parse(&jump_prompt(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           jump('## Other')\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tool.')\n\
         models.loop(msgs)\n\
         return 'no jump'",
    ));
    let (ctx, host) = loop_context(&prompt, ToolSet::default());
    let error = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("jump is withheld while the handler runs");
    let message = error.to_string();
    assert!(
        message.contains("jump") && message.contains("nil value"),
        "the handler's call reaches a nil `jump`: {message}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_works_in_the_same_block_after_the_loop_returns() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let prompt = parse(&jump_prompt(
        "tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)\n\
           return 'got ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('Use the tool.')\n\
         models.loop(msgs)\n\
         jump('## Other')",
    ));
    let (ctx, host) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("jump is restored once the handler returns");
    assert_eq!(out, "jumped");
}

#[tokio::test(flavor = "current_thread")]
async fn a_handler_returning_a_table_raises_and_is_observed_as_a_failure() {
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "grab", "{\"value\":\"hi\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let prompt = parse(&grab_loop("return { args.value }"));
    let recorder = Arc::new(Recorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let error = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a table return has no text form");
    assert!(
        error
            .to_string()
            .contains("cannot return a table as a result"),
        "the scalar-return rule names the rejected type: {error}"
    );
    assert!(
        recorder
            .events()
            .contains(&("Only".to_string(), detail::TOOL_CALL_FAILED.to_string())),
        "the rejected return is observed as a tool-call failure"
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
