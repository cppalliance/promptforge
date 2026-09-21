//! Tests for `models.loop`'s compactor argument: the omitted-compactor
//! default, `compactors.fail` invocation with the overflow reason, typed
//! context exhaustion raised at the call site, the non-function
//! compactor's argument error, and an author compactor that returns or
//! raises its own failure. The loop's round mechanics sit in
//! `models_loop`; its exit rules in `exit_rules`.

use super::models_loop::{loop_context, loop_prompt};
use super::*;
use crate::lua::{OverflowReason, ToolSet};
use crate::test_support::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn an_omitted_compactor_defaults_to_fail_with_typed_precheck_exhaustion() {
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         models.loop(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("an over-window request must exhaust the context");
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: OverflowReason::Precheck
            }
        ),
        "the omitted compactor defaults to compactors.fail with the precheck reason, got {error:?}"
    );
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before any request leaves"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn models_loop_raises_context_exhaustion_at_the_call_site() {
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'the overflow raises')\n\
         assert(#msgs == 1, 'a refused dispatch appends nothing')\n\
         return tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert!(
        out.starts_with("context exhausted: "),
        "the raised error is the typed exhaustion's message, got: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_explicit_compactors_fail_invocation_reports_the_provider_reason() {
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         models.loop(msgs, compactors.fail)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a provider context rejection must exhaust the context");
    assert!(
        matches!(
            error,
            Error::ContextExhausted {
                reason: OverflowReason::Provider
            }
        ),
        "the explicit compactors.fail invocation reports the provider reason, got {error:?}"
    );
    assert_eq!(gateway.call_count(), 1, "the request left and was rejected");
}

#[tokio::test(flavor = "current_thread")]
async fn a_non_function_compactor_is_the_calls_error_in_the_hosts_type_names() {
    // The argument error is pcall-able at the call site and names the
    // value's type as the protocol parse does: an integer is "integer",
    // a float "number", anything else its Lua type name. No round runs.
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         local out = {}\n\
         for _, bad in ipairs({ 42, 4.5, 'summarize', {} }) do\n\
           local ok, err = pcall(models.loop, msgs, bad)\n\
           assert(not ok, 'a non-function compactor raises')\n\
           assert(err.kind == 'lua', 'the argument error is the lua kind')\n\
           out[#out + 1] = tostring(err)\n\
         end\n\
         assert(#msgs == 1, 'a refused call appends nothing')\n\
         return table.concat(out, '|')",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(
        out,
        "compactor must be a function, got integer\
         |compactor must be a function, got number\
         |compactor must be a function, got string\
         |compactor must be a function, got table"
    );
    assert_eq!(
        gateway.call_count(),
        0,
        "the argument check fires before any request leaves"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_compactor_that_returns_is_the_deferred_replacement_error() {
    // A compactor that returns a replacement instead of raising is the
    // deferred framework's shape: the loop refuses it as a `lua`-kind error
    // naming the deferral and the one shipped policy, and appends nothing.
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         local seen\n\
         local ok, err = pcall(models.loop, msgs, function(reason)\n\
           seen = reason\n\
           return { role = 'user', content = 'summary' }\n\
         end)\n\
         assert(not ok, 'a returning compactor raises')\n\
         assert(seen == 'provider', 'the compactor ran with the reason tag')\n\
         assert(#msgs == 1, 'the refused round appends nothing')\n\
         return err.kind .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    let (kind, message) = out
        .split_once('|')
        .expect("the section returns kind|message");
    assert_eq!(kind, "lua");
    assert!(
        message.contains("deferred") && message.contains("compactors.fail"),
        "a returned replacement names the deferred framework, got: {message}"
    );
    assert_eq!(gateway.call_count(), 1, "the request left and was rejected");
}

#[tokio::test(flavor = "current_thread")]
async fn a_compactors_own_string_raise_reaches_the_host_with_the_reason_tag() {
    // An author compactor's own untyped raise is re-raised as the value it
    // raised: a bare string passes through the normalizer untouched and
    // fails the section as the ordinary Lua runtime error holding the
    // reason tag the compactor was invoked with.
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         models.loop(msgs, function(reason) error('custom failure: ' .. reason, 0) end)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("the compactor's own raise fails the section");
    match &error {
        Error::LuaRuntime { message, .. } => assert!(
            message.contains("custom failure: provider"),
            "the compactor's own error survives with the reason tag, got: {message}"
        ),
        other => panic!("expected the compactor's own runtime error, got {other:?}"),
    }
    assert_eq!(gateway.call_count(), 1, "the request left and was rejected");
}
