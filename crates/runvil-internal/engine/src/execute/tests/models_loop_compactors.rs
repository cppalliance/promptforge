//! Tests for `models.loop`'s compactor argument: the omitted-compactor
//! default, `compactors.fail` invocation with the overflow reason, typed
//! context exhaustion raised at the call site, the non-function
//! compactor's argument error, and an author compactor that returns or
//! raises its own failure. The loop's round mechanics sit in
//! `models_loop`; its exit rules in `exit_rules`.

use runvil_parser::test_support::one_section;

use super::gateway::overflowing;
use super::models_loop::loop_scenario;
use super::*;
use crate::lua::OverflowReason;

#[test]
fn an_omitted_compactor_defaults_to_fail_with_typed_precheck_exhaustion() {
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user(string.rep('x', 100000))
         models.loop(msgs)
         return 'unreachable'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    let error = trace.error().inner();
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

#[test]
fn models_loop_raises_context_exhaustion_at_the_call_site() {
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user(string.rep('x', 100000))
         local ok, err = pcall(models.loop, msgs)
         assert(not ok, 'the overflow raises')
         assert(#msgs == 1, 'a refused dispatch appends nothing')
         return tostring(err)",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    let out = trace.text();
    assert!(
        out.starts_with("context exhausted: "),
        "the raised error is the typed exhaustion's message, got: {out}"
    );
}

#[test]
fn an_explicit_compactors_fail_invocation_reports_the_provider_reason() {
    let gateway = ScriptedChat::new(vec![fail(
        crate::model::CompletionErrorKind::ContextOverflow,
        400,
        "This model's maximum context length is 4096 tokens.",
    )]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         models.loop(msgs, compactors.fail)
         return 'unreachable'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    let error = trace.error().inner();
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

#[test]
fn a_non_function_compactor_is_the_calls_error_in_the_engines_type_names() {
    // The argument error is pcall-able at the call site and names the
    // value's type as the protocol parse does: an integer is "integer",
    // a float "number", anything else its Lua type name. No round runs.
    // Each value is passed to `models.loop` and to a handle's `loop`;
    // both entries reach the compactor check.
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('hello')
         local writer = models.get('writer')
         local function refusal(entry, ...)
           local ok, err = pcall(entry, ...)
           assert(not ok, 'a non-function compactor raises')
           assert(err.kind == 'lua', 'the argument error is the lua kind')
           return tostring(err)
         end
         local out = {}
         for _, bad in ipairs({ 42, 4.5, 'summarize', {} }) do
           out[#out + 1] = refusal(models.loop, msgs, bad)
           out[#out + 1] = refusal(writer.loop, writer, msgs, bad)
         end
         assert(#msgs == 1, 'a refused call appends nothing')
         return table.concat(out, '|')",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "compactor must be a function, got integer\
         |compactor must be a function, got integer\
         |compactor must be a function, got number\
         |compactor must be a function, got number\
         |compactor must be a function, got string\
         |compactor must be a function, got string\
         |compactor must be a function, got table\
         |compactor must be a function, got table"
    );
    assert_eq!(
        gateway.call_count(),
        0,
        "the argument check fires before any request leaves"
    );
}

#[test]
fn a_compactor_that_returns_is_the_deferred_replacement_error() {
    // A compactor that returns a replacement instead of raising is refused:
    // the loop raises a `lua`-kind error naming the one shipped policy, and
    // appends nothing.
    let rounds = Arc::new(AtomicUsize::new(0));
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         local seen
         local ok, err = pcall(models.loop, msgs, function(reason)
           seen = reason
           return { role = 'user', content = 'summary' }
         end)
         assert(not ok, 'a returning compactor raises')
         assert(seen == 'provider', 'the compactor ran with the reason tag')
         assert(#msgs == 1, 'the refused round appends nothing')
         return err.kind .. '|' .. tostring(err)",
    );
    let trace = loop_scenario(source).chat_with(overflowing(&rounds)).run();
    let (kind, message) = trace
        .text()
        .split_once('|')
        .expect("the section returns kind|message");
    assert_eq!(kind, "lua");
    assert!(
        message.contains("deferred") && message.contains("compactors.fail"),
        "a returned replacement names the deferred framework, got: {message}"
    );
    assert_eq!(
        rounds.load(Ordering::SeqCst),
        1,
        "the request left and was rejected"
    );
}

#[test]
fn cancel_during_a_looping_compactor_returns_promptly() {
    // The first round's answer hands control to the block, whose next call
    // overflows the precheck and enters the looping compactor in the same
    // step. A cancel landing before that step starts ends the run the same
    // way, and a hang fails through nextest's timeout.
    let gateway = ScriptedChat::new(vec![text("first"), text("unreachable")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('hello')
         models.loop(msgs)
         msgs:user(string.rep('x', 100000))
         models.loop(msgs, function() while true do end end)
         return 'unreachable'",
    );
    let trace = loop_scenario(source)
        .chat(&gateway)
        .cancel_when(|effect| matches!(effect, Effect::Chat { .. }))
        .run();
    assert!(
        matches!(trace.result(), RunResult::Cancelled),
        "expected the cancelled run, got {:?}",
        trace.result()
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the precheck refuses the overflowing round before it leaves"
    );
}

#[test]
fn a_compactors_own_string_raise_reaches_the_caller_with_the_reason_tag() {
    // An author compactor's own untyped raise is re-raised as the value it
    // raised: a bare string passes through the normalizer untouched and
    // fails the section as the ordinary Lua runtime error holding the
    // reason tag the compactor was invoked with.
    let rounds = Arc::new(AtomicUsize::new(0));
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         models.loop(msgs, function(reason) error('custom failure: ' .. reason, 0) end)
         return 'unreachable'",
    );
    let trace = loop_scenario(source).chat_with(overflowing(&rounds)).run();
    match trace.error().inner() {
        Error::LuaRuntime { message, .. } => assert!(
            message.contains("custom failure: provider"),
            "the compactor's own error survives with the reason tag, got: {message}"
        ),
        other => panic!("expected the compactor's own runtime error, got {other:?}"),
    }
    assert_eq!(
        rounds.load(Ordering::SeqCst),
        1,
        "the request left and was rejected"
    );
}
