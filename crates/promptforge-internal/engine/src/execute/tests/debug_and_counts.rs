//! Tests for debug capture delivery and the `tools.calls` counters. The
//! infer-round reporting cases sit in `infer_rounds`.

use super::run;
use super::*;

#[tokio::test]
async fn debug_capture_receives_request_and_response_when_set() {
    let gateway = ScriptedChat::new(vec![resp_text("hello from the mock")]);
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nAsk the model.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &TestStore::new(),
        gatewayed_with_debug(&gateway, Arc::clone(&capture) as Arc<dyn DebugCapture>),
    )
    .await
    .unwrap();

    assert_eq!(out, "hello from the mock");
    let events = capture.events();
    assert_eq!(events.len(), 2, "one request and one response: {events:#?}");
    assert_eq!(events[0].0, EXECUTION);
    assert_eq!(events[0].1, "Only");
    assert_eq!(events[0].2, 1);
    let round = gateway.last_request().expect("infer must reach the model");
    assert_eq!(round.options.model(), "claude-sonnet-4-6");
    assert!(!round.messages.is_empty());
    // A scripted completion carries no raw exchange, so both bodies are
    // the `null` a broker without one reports.
    match &events[0].3 {
        crate::test_support::recording::DebugEvent::Request { body } => {
            assert!(body.is_null(), "{body}");
        }
        other => panic!("expected request first, got {other:?}"),
    }
    match &events[1].3 {
        crate::test_support::recording::DebugEvent::Response {
            body,
            finish_reason,
            reasoning_content,
        } => {
            assert_eq!(finish_reason, &None);
            assert_eq!(reasoning_content, &None);
            assert!(body.is_null(), "{body}");
        }
        other => panic!("expected response second, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_model_infer_capture_reaches_the_debug_sink() {
    // A nested infer called from Lua must route its request/response
    // capture to the run's owned debug sink instead of dropping it.
    let gateway = ScriptedChat::new(vec![resp_text("final answer")]);
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = models.get('writer'):infer('say hello')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[],
        &TestStore::new(),
        gatewayed_with_debug(&gateway, Arc::clone(&capture) as Arc<dyn DebugCapture>),
    )
    .await
    .expect("handle-form infer must return text");
    assert_eq!(out, "final answer");

    let events = capture.events();
    assert!(
        !events.is_empty(),
        "nested handle-form infer must reach the debug sink (F4), got no events"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.3,
            crate::test_support::recording::DebugEvent::Request { .. }
        )),
        "nested inference must capture at least one request: {events:#?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event.3,
            crate::test_support::recording::DebugEvent::Response { .. }
        )),
        "nested inference must capture at least one response: {events:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_debug_events_reach_the_run_sink() {
    // The arm's debug side channel: the fanout's run-context fork keeps
    // the run's own debug sink, so an arm's model-turn events land on the
    // run's sink under the worker's section name.
    let gateway = ScriptedChat::new(vec![resp_text("arm reply")]);
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        Reply about {{ item }}.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[],
        &TestStore::new(),
        gatewayed_with_debug(&gateway, Arc::clone(&capture) as Arc<dyn DebugCapture>),
    )
    .await
    .expect("the fanout must succeed");
    assert_eq!(out, "arm reply");

    let events = capture.events();
    let worker_events: Vec<_> = events
        .iter()
        .filter(|(_, section, _, _)| section == "Worker")
        .collect();
    assert!(
        worker_events.iter().any(|event| matches!(
            event.3,
            crate::test_support::recording::DebugEvent::Request { .. }
        )),
        "the arm's request must forward to the run's sink: {events:#?}"
    );
    assert!(
        worker_events.iter().any(|event| matches!(
            event.3,
            crate::test_support::recording::DebugEvent::Response { .. }
        )),
        "the arm's response must forward to the run's sink: {events:#?}"
    );
}

#[tokio::test]
async fn debug_capture_none_changes_nothing() {
    let gateway = ScriptedChat::new(vec![resp_text("hello from the mock")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nAsk the model.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &TestStore::new(),
        gatewayed(&gateway),
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");
}

// --- Per-VM tools.calls count tests ---

#[tokio::test]
async fn tool_calls_count_increments_on_successful_dispatch() {
    let tool = Arc::new(ScopedFixtureTool::new("echo", "Echo a test value."));
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.call('tools/echo', { value = 'x' })\n\
        assert(tools.calls['tools/echo'] == 1, \
        'expected 1 call, got ' .. tostring(tools.calls['tools/echo']))\n\
        return 'ok'\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "ok");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn tool_calls_count_increments_even_when_tool_errors() {
    // Drive a real `FailingTool` through a `models.loop` round and prove
    // `tools.calls` records exactly one call even though the tool errors
    // (the count is incremented before dispatch). The tool's own failure is
    // the call's error result, so the loop continues to the terminal reply.
    use super::models_loop::{always_tool, loop_context, loop_prompt};
    use crate::test_support::tokio_driver::TokioDriver;

    let gateway = ScriptedChat::new(vec![
        resp_tool_call("call_x", "echo", "{\"value\":\"x\"}"),
        resp_text("final answer"),
    ]);
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('ask the model')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content .. '|' .. tostring(tools.calls['tools/failing'])",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, always_tool("echo", Arc::new(FailingTool)));
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("a tool's own failure becomes the call's result, not the loop's");
    assert_eq!(
        out, "final answer|1",
        "the counter must record exactly one call even though the tool errored"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_model_call_and_a_script_call_count_under_the_id_and_a_local_tool_under_its_alias() {
    use super::models_loop::{echo_tools, loop_context, loop_prompt};
    use crate::test_support::tokio_driver::TokioDriver;

    let gateway = ScriptedChat::new(vec![
        ScriptedReply::ToolCalls {
            model: MOCK_MODEL.to_owned(),
            calls: vec![
                scripted_call("call_1", "echo", r#"{"value":"model"}"#),
                scripted_call("call_2", "grab", r#"{"value":"local"}"#),
            ],
        },
        resp_text("final answer"),
    ]);
    let md = loop_prompt(
        "tools.offer_local('grab', 'Grab a value', { value = 'string' }, \
           function(args) return 'got ' .. args.value end)\n\
         local msgs = messages.new()\n\
         msgs:user('ask the model')\n\
         models.loop(msgs)\n\
         tools.call('tools/echo', { value = 'script' })\n\
         local ok = pcall(function() return tools.calls.echo end)\n\
         return tostring(tools.calls['tools/echo']) .. '|' .. tostring(tools.calls.grab) \
           .. '|' .. tostring(ok)",
    );
    let prompt = parse(&md);
    let (ctx, fixture) = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the loop and the script call complete");
    assert_eq!(
        out, "2|1|false",
        "the model's wire-name call and the script's id call count once each under \
         the id, the local tool under its alias, and the wire name is no key"
    );
}

#[tokio::test]
async fn tool_calls_count_zero_for_uncalled_alias_fails_epilog_assert() {
    // The first script dispatch installs the counts seeded from the
    // effective scope, so an offered but uncalled tool reads as 0 and an
    // author assert on it fails the run with its own message.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.offer('tools/search')\n\
        local _ = tools.call('tools/other', { value = 'x' })\n\
        ```\n\n\
        ```lua\nassert(tools.calls['tools/search'] > 0, 'search was never called')\n\
        return 'unreached'\n```\n";
    let search = ScopedFixtureTool::new("search", "Search for things.");
    let other = ScopedFixtureTool::new("other", "Other things.");
    let prompt = bound_with_tools(md);
    let error = run(
        &prompt,
        "",
        &[
            Arc::new(search) as Arc<dyn TestTool>,
            Arc::new(other) as Arc<dyn TestTool>,
        ],
        &TestStore::new(),
        silent(),
    )
    .await
    .expect_err("epilog assert on zero count must fail the run");
    assert!(
        error.to_string().contains("search was never called"),
        "error must include the assert message: {error}"
    );
}

#[tokio::test]
async fn tool_calls_typo_alias_is_a_hard_error_with_seeded_set() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nplugins:\n  - tools\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.offer('tools/search')\n\
        local _ = tools.call('tools/search', { value = 'x' })\n\
        ```\n\n\
        ```lua\nlocal _ = tools.calls['tools/serach']\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "Search for things.");
    let prompt = bound_with_tools(md);
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .expect_err("accessing a typo id in tools.calls must hard error");
    let msg = error.to_string();
    assert!(
        msg.contains("tools/serach") && msg.contains("has no seeded count"),
        "error must name the bad key and state it was never seeded: {msg}"
    );
    assert!(
        msg.contains("seeded names: [\"tools/search\"]")
            && msg.contains(" - check for typos or offer it with tools.offer"),
        "error must list the seeded ids and the remedy: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_infer_returns_text_without_touching_reply_or_sys() {
    // The one infer shape: `handle:infer(...)` returns the round's text and never
    // sets `reply` or `sys.reply_finish_reason`.
    let gateway = ScriptedChat::new(vec![resp_text("pong")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = models.get('writer'):infer('say hello')\n\
        assert(type(text) == 'string', 'infer must return text')\n\
        assert(text == 'pong')\n\
        assert(reply == nil, 'infer must not set reply')\n\
        assert(not pcall(function() return sys.reply_finish_reason end),\n\
            'infer must not touch sys')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(&prompt, "", &[], &TestStore::new(), gatewayed(&gateway))
        .await
        .expect("handle-form infer must return text");
    assert_eq!(out, "pong");
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert!(
        body.tools.is_empty(),
        "handle-form infer advertises no tools: {body:?}"
    );
}

#[path = "debug_and_counts-infer-rounds.rs"]
mod infer_rounds;
