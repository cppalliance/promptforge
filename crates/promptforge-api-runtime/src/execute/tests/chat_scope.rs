//! Tests for the `Chat` arm's tool-scope resolution from a section VM: an
//! absent `tools` list is the section's current effective scope plus every
//! local Lua tool; an explicit list names its members (a local tool, an
//! effective binding, or a bound catalog slot outside the section's
//! scope), an empty list advertises nothing, and an alias bound nowhere
//! fails the call as `unbound_tool` before any request leaves.

use super::chat_arm::chat_context;
use super::models_loop::{echo_tools, loop_prompt};
use super::*;
use crate::test_support::tokio_driver::TokioDriver;

/// The function names one request advertised, in wire order.
fn advertised_names(body: &serde_json::Value) -> Vec<&str> {
    body["tools"]
        .as_array()
        .expect("tools is an array")
        .iter()
        .map(|tool| {
            tool["function"]["name"]
                .as_str()
                .expect("a tool schema names its function")
        })
        .collect()
}

/// The tool set with `echo` always in scope and `spare` bound in the
/// catalog but never scoped into the section.
fn echo_and_spare_tools() -> FixtureTools {
    FixtureTools::new(
        vec![
            fixture_binding("echo", "echo capability", Arc::new(EchoTool)),
            fixture_binding("spare", "a bound but unscoped echo", Arc::new(EchoTool)),
        ],
        vec!["echo".to_owned()],
    )
}

#[tokio::test(flavor = "current_thread")]
async fn an_absent_tool_list_advertises_the_section_scope_with_local_tools() {
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_1", "grab", "{\"value\":\"x\"}")]).await;
    let md = loop_prompt(
        "tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use the local tool')\n\
         local r = models.chat(msgs)\n\
         assert(r.tool_calls[1].name == 'grab', 'a local tool is in the advertised scope')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = chat_context(&prompt, echo_tools(), Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the section scope includes local tools");
    assert_eq!(out, "ok");
    let body = gateway.last_request().expect("one request left");
    assert_eq!(
        advertised_names(&body),
        ["echo", "grab"],
        "the bound scope then the local tools: {body:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_tool_list_advertises_exactly_its_members() {
    // Each alias resolves in turn: `grab` is a local tool, `spare` is a
    // bound catalog slot outside the section's scope. `echo` (effective)
    // and `skip` (local) are omitted from the list and so from the wire,
    // and the round's scope gate is the explicit set.
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_1", "grab", "{\"value\":\"x\"}")]).await;
    let md = loop_prompt(
        "tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         tools.add_local('skip', 'Local skip', { value = 'string' }, function(args)\n\
           return 'skipped'\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use the named tools')\n\
         local r = models.chat(msgs, { tools = { 'grab', 'spare' } })\n\
         assert(r.tool_calls[1].name == 'grab', 'a listed local tool is in scope')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = chat_context(
        &prompt,
        echo_and_spare_tools(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an explicit list resolves each member");
    assert_eq!(out, "ok");
    let body = gateway.last_request().expect("one request left");
    assert_eq!(
        advertised_names(&body),
        ["spare", "grab"],
        "the listed bound slot then the listed local tool: {body:?}"
    );

    // An unlisted effective binding is outside the round's scope gate.
    let gateway = ScriptedGateway::start(vec![resp_tool_call("call_1", "echo", "{}")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('call the unlisted tool')\n\
         local ok, err = pcall(models.chat, msgs, { tools = { 'spare' } })\n\
         assert(not ok, 'an unlisted call raises')\n\
         return err.kind .. '|' .. err.name",
    );
    let prompt = parse(&md);
    let ctx = chat_context(
        &prompt,
        echo_and_spare_tools(),
        Arc::new(NullObserver::default()),
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the scope refusal is pcall-able");
    assert_eq!(out, "out_of_scope_tool|echo");
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_tool_list_advertises_nothing() {
    let gateway = ScriptedGateway::start(vec![resp_text("no tools")]).await;
    let md = loop_prompt(
        "tools.add_local('grab', 'Local grab', { value = 'string' }, function(args)\n\
           return 'grabbed ' .. args.value\n\
         end)\n\
         local msgs = messages.new()\n\
         msgs:user('use no tools')\n\
         local r = models.chat(msgs, { tools = {} })\n\
         assert(r.reply == 'no tools', 'the reply resumes')\n\
         return 'ok'",
    );
    let prompt = parse(&md);
    let ctx = chat_context(&prompt, echo_tools(), Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an empty list runs a tool-free round");
    assert_eq!(out, "ok");
    let body = gateway.last_request().expect("one request left");
    assert!(
        body.get("tools").is_none() || body["tools"].is_null(),
        "an empty list puts no tools on the wire, not even the section's scope: {body:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_unbound_alias_in_the_tool_list_fails_the_call_as_unbound_tool() {
    // Readable at the call site as the `unbound_tool` kind naming the
    // run's whole bound catalog, and typed when it escapes the section; no
    // request leaves either way.
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('use a ghost')\n\
         local ok, err = pcall(models.chat, msgs, { tools = { 'echo', 'ghost' } })\n\
         assert(not ok, 'an unbound alias raises')\n\
         return err.kind .. '|' .. err.name .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = chat_context(&prompt, echo_tools(), Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(
        out,
        "unbound_tool|ghost|tool \"ghost\" is not bound in this run; bound aliases: [\"echo\"]"
    );
    assert_eq!(gateway.call_count(), 0, "the refusal fires before dispatch");

    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('use a ghost')\n\
         models.chat(msgs, { tools = { 'ghost' } })\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = chat_context(&prompt, echo_tools(), Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("an uncaught unbound alias fails the section");
    match error {
        Error::UnboundToolCall { name, bound } => {
            assert_eq!(name, "ghost");
            assert_eq!(bound, vec!["echo".to_owned()]);
        }
        other => panic!("expected UnboundToolCall, got {other:?}"),
    }
    assert_eq!(gateway.call_count(), 0, "no request left on either path");
}
