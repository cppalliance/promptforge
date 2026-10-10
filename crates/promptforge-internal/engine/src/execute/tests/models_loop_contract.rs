//! Contract tests for `models.loop`: the behaviors the loop's earlier
//! suites leave unpinned, each driven through the public Lua surface over
//! `messages.new()` lists and asserting only what an author or the caller
//! sees. The default and argument compactor, the list checks, an uncaught
//! argument error, the drain's order, the single return value, and the
//! compactor call sit here; the exit rules, the batch rule, tool results,
//! and the event trace sit in `rounds`.

use super::model_task_notices::loop_owner;
use super::model_tasks::{PARKED_CHILD, model_task_context_with, owner_prompt};
use super::models_loop::{loop_context, loop_context_observed, loop_prompt};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

#[path = "models_loop_contract-rounds.rs"]
mod rounds;

/// Drives `md` with `tools` in scope against `gateway`, reporting to
/// `observer`, and returns the section's result.
async fn drive_observed(
    md: &str,
    tools: impl Into<FixtureTools>,
    gateway: &ScriptedChat,
    observer: Arc<dyn Observer>,
) -> Result<String> {
    let prompt = parse(md);
    let (ctx, fixture) = loop_context_observed(&prompt, tools, observer);
    TokioDriver::new(&ctx, fixture, Some(gateway_client(gateway)))
        .drive()
        .await
}

/// [`drive_observed`] under the null observer.
async fn drive(md: &str, tools: impl Into<FixtureTools>, gateway: &ScriptedChat) -> Result<String> {
    drive_observed(md, tools, gateway, Arc::new(NullObserver::default())).await
}

/// Drives `md` against a client whose every round is a provider overflow,
/// returning the section's result and the rounds the client was asked for.
async fn drive_overflowing(md: &str) -> (Result<String>, usize) {
    let client = OverflowClient::default();
    let prompt = parse(md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let out = TokioDriver::new(&ctx, fixture.client(client.clone()), None)
        .drive()
        .await;
    (out, client.calls.load(Ordering::SeqCst))
}

#[tokio::test(flavor = "current_thread")]
async fn the_default_compactor_is_whatever_compactors_fail_holds_at_the_call() {
    // The default is read from `compactors.fail` at each call and never
    // checked: an author's replacement becomes the default, and a
    // non-function there lets an ordinary round run and fails only at an
    // overflow round, with Lua's own call error as a string.
    let gateway = ScriptedChat::new(vec![resp_text("fine")]);
    let md = loop_prompt(
        "local big = messages.new()\n\
         big:user(string.rep('x', 100000))\n\
         compactors.fail = function(reason) error('replaced: ' .. reason, 0) end\n\
         local _, replaced = pcall(models.loop, big)\n\
         compactors.fail = 42\n\
         local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs)\n\
         local _, called = pcall(models.loop, big)\n\
         return replaced .. '|' .. #msgs .. '|' .. type(called) .. '|' .. called .. '|' .. #big",
    );
    let out = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect("every overflow raise is pcall-able");
    assert_eq!(
        out,
        "replaced: precheck|2|string|attempt to call a number value|1"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "only the ordinary round sends a request"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_light_userdata_compactor_is_named_userdata_as_lua_names_it() {
    // A JSON null in a structured tool's result is a null light userdata,
    // which Lua's `type()` calls `userdata`.
    let gateway = ScriptedChat::new(vec![resp_text("unreachable")]);
    let md = loop_prompt(
        "local null = tools.call('structured', {}).none\n\
         local other = models.get('other')\n\
         local msgs = messages.new()\n\
         msgs:user('hello')\n\
         local ok, err = pcall(other.loop, other, msgs, null)\n\
         assert(not ok, 'a light userdata compactor is refused')\n\
         return type(null) .. '|' .. err.kind .. '|' .. tostring(err) .. '|' .. #msgs",
    );
    let mut binding = fixture_binding(
        "structured",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "{\"none\":null}",
            trusted: true,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let tools = FixtureTools::new(vec![binding], vec!["structured".to_owned()]);
    let out = drive(&md, tools, &gateway)
        .await
        .expect("the refusal is pcall-able");
    assert_eq!(
        out,
        "userdata|lua|compactor must be a function, got userdata|1"
    );
    assert_eq!(gateway.call_count(), 0, "no round runs");
}

#[tokio::test(flavor = "current_thread")]
async fn a_number_no_argument_or_a_handle_in_the_lists_place_is_the_list_error() {
    let gateway = ScriptedChat::new(vec![resp_text("unreachable")]);
    let md = loop_prompt(
        "local other = models.get('other')\n\
         local msgs = messages.new()\n\
         msgs:user('hello')\n\
         local out = {}\n\
         for _, call in ipairs({\n\
           function() return models.loop(42) end,\n\
           function() return other:loop(42) end,\n\
           function() return models.loop() end,\n\
           function() return other:loop() end,\n\
           function() return other:loop(other) end,\n\
         }) do\n\
           local ok, err = pcall(call)\n\
           assert(not ok, 'the call is refused')\n\
           out[#out + 1] = err.kind .. ':' .. tostring(err)\n\
         end\n\
         return table.concat(out, '|') .. '|' .. #msgs",
    );
    let out = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect("every refusal is pcall-able");
    let refusal = "lua:models.loop needs a messages.new() list; build one with \
                   messages.new() and :user, :append, or :replace";
    assert_eq!(out, format!("{}|1", [refusal; 5].join("|")));
    assert_eq!(gateway.call_count(), 0, "no refused call sends a request");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_list_error_still_raises_when_the_author_has_rebound_error() {
    // A shim raise that reads the global `error` calls the author's no-op
    // and spins, so the timed cancel ends that case as `Interrupted`
    // instead of hanging the test.
    use std::time::Duration;

    let gateway = ScriptedChat::new(vec![resp_text("unreachable")]);
    let md = loop_prompt(
        "error = function() end\n\
         local ok, err = pcall(models.loop, 42)\n\
         return tostring(err)",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, ToolSet::default());
    let mut driver = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)));
    let canceller = driver.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(1)).await;
        canceller.cancel();
    });
    let result = driver.drive().await;
    assert!(
        !matches!(result, Err(Error::Interrupted)),
        "the raise ends the call before the cancel fires: {result:?}"
    );
    assert_eq!(
        result.expect("the list error is pcall-able"),
        "models.loop needs a messages.new() list; build one with \
         messages.new() and :user, :append, or :replace"
    );
    assert_eq!(gateway.call_count(), 0, "no round runs");
}

#[tokio::test(flavor = "current_thread")]
async fn an_empty_list_raises_unless_a_pending_notice_fills_its_round() {
    // The empty check runs at each round, after that round's drain: with
    // nothing pending the call raises before any request, and once the
    // author's cancel queues a notice, an empty list runs one round over it.
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_text("started"),
        resp_text("read"),
    ]);
    let md = owner_prompt(
        "",
        &loop_owner(
            "local ok, err = pcall(models.loop, messages.new())\n\
             assert(not ok, 'an empty list with nothing pending raises')\n\
             local refused = err.kind .. ':' .. tostring(err)\n\
             tasks.cancel(tasks.pending({ origin = 'model' })[1])\n\
             local fresh = messages.new()\n\
             models.loop(fresh)\n\
             return refused .. '|' .. #fresh .. '|' .. fresh[1].role .. ':' .. fresh[1].content \
             .. '|' .. fresh[2].content",
        ),
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let (ctx, fixture) = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(SlowTool),
    );
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the empty-list raise is pcall-able");
    assert_eq!(
        out,
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

#[tokio::test(flavor = "current_thread")]
async fn an_uncaught_loop_arity_error_fails_the_run_naming_the_authors_prompt_line() {
    // The loop block opens at prompt line 12, so its third line is 14.
    let gateway = ScriptedChat::new(vec![resp_text("unreachable")]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('hello')\n\
         models.loop(msgs, nil, nil)\n\
         return 'unreachable'",
    );
    let error = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect_err("an uncaught argument error fails the section");
    match &error {
        Error::LuaRuntime { message, .. } => {
            assert!(
                message.contains("models.loop takes (messages, compactor?)"),
                "the runtime error keeps the shim's message: {message}"
            );
            assert!(
                message.contains("[string \"section `Only` prologue\"]:14:"),
                "the runtime error names the author's prompt line: {message}"
            );
        }
        other => panic!("expected the mapped runtime error, got {other:?}"),
    }
    assert_eq!(gateway.call_count(), 0, "no round runs");
}

#[tokio::test(flavor = "current_thread")]
async fn two_notices_drained_into_one_round_land_in_the_order_their_tasks_ended() {
    // The author cancels the model's second task before its first, so the
    // queue holds the second's notice first.
    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_1", "task", "{\"target\":\"## Child\"}"),
        resp_tool_call("call_2", "task", "{\"target\":\"## Child\"}"),
        resp_text("started"),
        resp_text("read"),
    ]);
    let md = owner_prompt(
        "",
        &loop_owner(
            "local live = {}\n\
             for _, t in ipairs(tasks.pending({ origin = 'model' })) do live[t.task] = t end\n\
             tasks.cancel(live['0.1'])\n\
             tasks.cancel(live['0.0'])\n\
             models.loop(msgs)\n\
             return #msgs .. '|' .. msgs[7].role .. ':' .. msgs[7].content \
             .. '|' .. msgs[8].role .. ':' .. msgs[8].content",
        ),
        PARKED_CHILD,
    );
    let prompt = parse(&md);
    let (ctx, fixture) = model_task_context_with(
        &prompt,
        Arc::new(NullObserver::default()),
        Arc::new(SlowTool),
    );
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("both notices reach the next round");
    assert_eq!(
        out,
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

#[tokio::test(flavor = "current_thread")]
async fn each_loop_entry_returns_exactly_one_value() {
    let gateway = ScriptedChat::new(vec![resp_text("one"), resp_text("two")]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('first')\n\
         local n = select('#', models.loop(msgs))\n\
         msgs:user('second')\n\
         local other = models.get('other')\n\
         local m = select('#', other:loop(msgs))\n\
         return n .. '|' .. m .. '|' .. #msgs",
    );
    let out = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect("both replies end their loops");
    assert_eq!(out, "1|1|4");
}

#[tokio::test(flavor = "current_thread")]
async fn the_compactor_gets_only_the_reason_and_may_suspend_before_raising() {
    // The compactor runs inside the block, so its own `models.infer`
    // round suspends and resumes like any other.
    let gateway = ScriptedChat::new(vec![resp_text("summary")]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user(string.rep('x', 100000))\n\
         local count, summary\n\
         local ok, err = pcall(models.loop, msgs, function(...)\n\
           count = select('#', ...)\n\
           summary = models.infer('summarize')\n\
           error('compacted after ' .. summary, 0)\n\
         end)\n\
         return tostring(ok) .. '|' .. count .. '|' .. err .. '|' .. #msgs",
    );
    let out = drive(&md, ToolSet::default(), &gateway)
        .await
        .expect("the compactor's raise is pcall-able");
    assert_eq!(out, "false|1|compacted after summary|1");
    assert_eq!(
        gateway.call_count(),
        1,
        "the compactor's infer round is the only request"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_returning_compactor_raises_the_exact_deferred_replacement_text() {
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         local ok, err = pcall(models.loop, msgs, function() return 'summary' end)\n\
         assert(not ok, 'a returning compactor raises')\n\
         return err.kind .. '|' .. tostring(err) .. '|' .. #msgs",
    );
    let (out, rounds) = drive_overflowing(&md).await;
    assert_eq!(
        out.expect("the raise is pcall-able"),
        "lua|the selected compactor returned without raising: replacement compactors are \
         deferred; compactors.fail is the only shipped policy|1"
    );
    assert_eq!(rounds, 1, "the one round left and overflowed");
}

#[tokio::test(flavor = "current_thread")]
async fn a_compactors_own_table_raise_reaches_the_caller_as_the_same_table() {
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('a small prompt')\n\
         local own = { reason = 'mine' }\n\
         local ok, err = pcall(models.loop, msgs, function() error(own) end)\n\
         return tostring(ok) .. '|' .. tostring(err == own) .. '|' \
         .. tostring(getmetatable(err) == nil) .. '|' .. #msgs",
    );
    let (out, rounds) = drive_overflowing(&md).await;
    assert_eq!(out.expect("the raise is pcall-able"), "false|true|true|1");
    assert_eq!(rounds, 1, "the one round left and overflowed");
}
