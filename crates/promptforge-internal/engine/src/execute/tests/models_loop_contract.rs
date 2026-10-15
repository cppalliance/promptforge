//! Contract tests for `models.loop`: the behaviors the loop's earlier
//! suites leave unpinned, each driven through the public Lua surface over
//! `messages.new()` lists and asserting only what an author or the caller
//! sees. The default and argument compactor, the list checks, an uncaught
//! argument error, the drain's order, the single return value, and the
//! compactor call sit here; the exit rules, the batch rule, tool results,
//! and the event trace sit in `rounds`.

use promptforge_parser::test_support::{one_section, prompt};

use super::model_task_notices::loop_owner;
use super::model_tasks::{CHILD_PARKED_ON_SLOW_TOOL, model_task_scenario};
use super::models_loop::loop_scenario;
use super::*;
use crate::test_support::scenario::Trace;

#[path = "models_loop_contract-rounds.rs"]
mod rounds;

/// Runs `source` against a model whose every round is a provider
/// overflow, returning the run's trace and the rounds the model answered.
fn run_overflowing(source: String) -> (Trace, usize) {
    let rounds = Arc::new(AtomicUsize::new(0));
    let trace = loop_scenario(source).chat_with(overflowing(&rounds)).run();
    (trace, rounds.load(Ordering::SeqCst))
}

#[test]
fn the_default_compactor_is_whatever_compactors_fail_holds_at_the_call() {
    // The default is read from `compactors.fail` at each call and never
    // checked: an author's replacement becomes the default, and a
    // non-function there lets an ordinary round run and fails only at an
    // overflow round, with Lua's own call error as a string.
    let gateway = ScriptedChat::new(vec![text("fine")]);
    let source = one_section(
        "local big = messages.new()
         big:user(string.rep('x', 100000))
         compactors.fail = function(reason) error('replaced: ' .. reason, 0) end
         local _, replaced = pcall(models.loop, big)
         compactors.fail = 42
         local msgs = messages.new()
         msgs:user('hello')
         models.loop(msgs)
         local _, called = pcall(models.loop, big)
         return replaced .. '|' .. #msgs .. '|' .. type(called) .. '|' .. called .. '|' .. #big",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "replaced: precheck|2|string|attempt to call a number value|1"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "only the ordinary round sends a request"
    );
}

#[test]
fn a_light_userdata_compactor_is_named_userdata_as_lua_names_it() {
    // A JSON null in a structured tool's result is a null light userdata,
    // which Lua's `type()` calls `userdata`.
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "tools.offer('tools/structured')
         local null = tools.call('tools/structured', {}).none
         local other = models.get('other')
         local msgs = messages.new()
         msgs:user('hello')
         local ok, err = pcall(other.loop, other, msgs, null)
         assert(not ok, 'a light userdata compactor is refused')
         return type(null) .. '|' .. err.kind .. '|' .. tostring(err) .. '|' .. #msgs",
    );
    let trace = loop_scenario(source)
        .offer("structured", structured_tool("{\"none\":null}", true))
        .chat(&gateway)
        .run();
    assert_eq!(
        trace.text(),
        "userdata|lua|compactor must be a function, got userdata|1"
    );
    assert_eq!(gateway.call_count(), 0, "no round runs");
}

#[test]
fn a_number_no_argument_or_a_handle_in_the_lists_place_is_the_list_error() {
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local other = models.get('other')
         local msgs = messages.new()
         msgs:user('hello')
         local out = {}
         for _, call in ipairs({
           function() return models.loop(42) end,
           function() return other:loop(42) end,
           function() return models.loop() end,
           function() return other:loop() end,
           function() return other:loop(other) end,
         }) do
           local ok, err = pcall(call)
           assert(not ok, 'the call is refused')
           out[#out + 1] = err.kind .. ':' .. tostring(err)
         end
         return table.concat(out, '|') .. '|' .. #msgs",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    let refusal = "lua:models.loop needs a messages.new() list; build one with \
                   messages.new() and :user, :append, or :replace";
    assert_eq!(trace.text(), format!("{}|1", [refusal; 5].join("|")));
    assert_eq!(gateway.call_count(), 0, "no refused call sends a request");
}

#[test]
fn the_list_error_still_raises_when_the_author_has_rebound_error() {
    // A shim raise that read the global `error` would call the author's
    // no-op instead of raising. Had that sent a round, the run would wait
    // on nothing but the held round, and the cancel ends it `Cancelled`;
    // had it spun in the block, nextest's timeout ends the test.
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "error = function() end
         local ok, err = pcall(models.loop, 42)
         return tostring(err)",
    );
    let mut driving = loop_scenario(source)
        .chat(&gateway)
        .hold(|effect| matches!(effect, Effect::Chat { .. }))
        .start();
    while driving.step() {}
    driving.cancel();
    let trace = driving.finish();
    assert!(
        !matches!(trace.result(), RunResult::Cancelled),
        "the raise ends the call before the cancel: {:?}",
        trace.result()
    );
    assert_eq!(
        trace.text(),
        "models.loop needs a messages.new() list; build one with \
         messages.new() and :user, :append, or :replace"
    );
    assert!(trace.effects().is_empty(), "no round runs");
}

#[test]
fn an_empty_list_raises_unless_a_pending_notice_fills_its_round() {
    // The empty check runs at each round, after that round's drain: with
    // nothing pending the call raises before any request, and once the
    // author's cancel queues a notice, an empty list runs one round over it.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        text("started"),
        text("read"),
    ]);
    let owner = loop_owner(
        "local ok, err = pcall(models.loop, messages.new())\n\
         assert(not ok, 'an empty list with nothing pending raises')\n\
         local refused = err.kind .. ':' .. tostring(err)\n\
         tasks.cancel(tasks.pending({ origin = 'model' })[1])\n\
         local fresh = messages.new()\n\
         models.loop(fresh)\n\
         return refused .. '|' .. #fresh .. '|' .. fresh[1].role .. ':' .. fresh[1].content \
         .. '|' .. fresh[2].content",
    );
    let source = prompt(
        "",
        &[("Only", &owner), ("Child", CHILD_PARKED_ON_SLOW_TOOL)],
    );
    let trace = model_task_scenario(source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "lua:messages must not be empty|2\
         |user:Task id=0.0 (## Child) was canceled: the author cancelled it|read"
    );
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 3, "the refused call sends no request");
    assert_eq!(
        bodies[2].messages.len(),
        1,
        "the round over the empty list holds only the notice"
    );
}

#[test]
fn an_uncaught_loop_arity_error_fails_the_run_naming_the_authors_prompt_line() {
    // The loop block opens at prompt line 12, so its third line is 14.
    let gateway = ScriptedChat::new(vec![text("unreachable")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('hello')
         models.loop(msgs, nil, nil)
         return 'unreachable'",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    match trace.error().inner() {
        Error::LuaRuntime { message, .. } => {
            assert!(
                message.contains("models.loop takes (messages, compactor?)"),
                "the runtime error keeps the shim's message: {message}"
            );
            assert!(
                message.contains("section `Only` prologue:14: in"),
                "the runtime error names the author's prompt line: {message}"
            );
        }
        other => panic!("expected the mapped runtime error, got {other:?}"),
    }
    assert_eq!(gateway.call_count(), 0, "no round runs");
}

#[test]
fn two_notices_drained_into_one_round_land_in_the_order_their_tasks_ended() {
    // The author cancels the model's second task before its first, so the
    // queue holds the second's notice first.
    let gateway = ScriptedChat::new(vec![
        calls(&[("call_1", "task", "{\"target\":\"## Child\"}")]),
        calls(&[("call_2", "task", "{\"target\":\"## Child\"}")]),
        text("started"),
        text("read"),
    ]);
    let owner = loop_owner(
        "local live = {}\n\
         for _, t in ipairs(tasks.pending({ origin = 'model' })) do live[t.task] = t end\n\
         tasks.cancel(live['0.1'])\n\
         tasks.cancel(live['0.0'])\n\
         models.loop(msgs)\n\
         return #msgs .. '|' .. msgs[7].role .. ':' .. msgs[7].content \
         .. '|' .. msgs[8].role .. ':' .. msgs[8].content",
    );
    let source = prompt(
        "",
        &[("Only", &owner), ("Child", CHILD_PARKED_ON_SLOW_TOOL)],
    );
    let trace = model_task_scenario(source).chat(&gateway).run();
    assert_eq!(
        trace.text(),
        "9|user:Task id=0.1 (## Child) was canceled: the author cancelled it\
         |user:Task id=0.0 (## Child) was canceled: the author cancelled it"
    );
    // The wire coalesces the two consecutive user records into one message.
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 4, "both notices ride one round");
    let last = bodies[3].messages.last().expect("the round sends messages");
    let ended_first = last
        .content()
        .find("Task id=0.1")
        .expect("0.1's notice is sent");
    let ended_second = last
        .content()
        .find("Task id=0.0")
        .expect("0.0's notice is sent");
    assert!(
        last.role() == "user" && ended_first < ended_second,
        "the round's last message holds both notices in queue order: {last:?}"
    );
}

#[test]
fn each_loop_entry_returns_exactly_one_value() {
    let gateway = ScriptedChat::new(vec![text("one"), text("two")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user('first')
         local n = select('#', models.loop(msgs))
         msgs:user('second')
         local other = models.get('other')
         local m = select('#', other:loop(msgs))
         return n .. '|' .. m .. '|' .. #msgs",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "1|1|4");
}

#[test]
fn the_compactor_gets_only_the_reason_and_may_suspend_before_raising() {
    // The compactor runs inside the block, so its own `models.infer`
    // round suspends and resumes like any other.
    let gateway = ScriptedChat::new(vec![text("summary")]);
    let source = one_section(
        "local msgs = messages.new()
         msgs:user(string.rep('x', 100000))
         local count, summary
         local ok, err = pcall(models.loop, msgs, function(...)
           count = select('#', ...)
           summary = models.infer('summarize')
           error('compacted after ' .. summary, 0)
         end)
         return tostring(ok) .. '|' .. count .. '|' .. err .. '|' .. #msgs",
    );
    let trace = loop_scenario(source).chat(&gateway).run();
    assert_eq!(trace.text(), "false|1|compacted after summary|1");
    assert_eq!(
        gateway.call_count(),
        1,
        "the compactor's infer round is the only request"
    );
}

#[test]
fn a_returning_compactor_raises_the_exact_deferred_replacement_text() {
    let (trace, rounds) = run_overflowing(one_section(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         local ok, err = pcall(models.loop, msgs, function() return 'summary' end)
         assert(not ok, 'a returning compactor raises')
         return err.kind .. '|' .. tostring(err) .. '|' .. #msgs",
    ));
    assert_eq!(
        trace.text(),
        "lua|the selected compactor returned without raising: replacement compactors are \
         deferred; compactors.fail is the only shipped policy|1"
    );
    assert_eq!(rounds, 1, "the one round left and overflowed");
}

#[test]
fn a_compactors_own_table_raise_reaches_the_caller_as_the_same_table() {
    let (trace, rounds) = run_overflowing(one_section(
        "local msgs = messages.new()
         msgs:user('a small prompt')
         local own = { reason = 'mine' }
         local ok, err = pcall(models.loop, msgs, function() error(own) end)
         return tostring(ok) .. '|' .. tostring(err == own) .. '|' \
         .. tostring(getmetatable(err) == nil) .. '|' .. #msgs",
    ));
    assert_eq!(trace.text(), "false|true|true|1");
    assert_eq!(rounds, 1, "the one round left and overflowed");
}
