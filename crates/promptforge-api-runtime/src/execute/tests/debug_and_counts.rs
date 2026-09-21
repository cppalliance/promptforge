//! Tests for debug capture delivery and the `tools.calls` counters.

use super::run;
use super::*;

#[tokio::test]
async fn debug_capture_receives_request_and_response_when_set() {
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nAsk the model.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &TestStore::new(),
        gatewayed_with_debug(addr, Arc::clone(&capture) as Arc<dyn DebugCapture>),
    )
    .await
    .unwrap();

    assert_eq!(out, "hello from the mock");
    let events = capture.events();
    assert_eq!(events.len(), 2, "one request and one response: {events:#?}");
    assert_eq!(events[0].0, EXECUTION);
    assert_eq!(events[0].1, "Only");
    assert_eq!(events[0].2, 1);
    match &events[0].3 {
        crate::test_support::recording::DebugEvent::Request { body } => {
            assert_eq!(body["model"], "claude-sonnet-4-6");
            assert!(body["messages"].as_array().is_some_and(|m| !m.is_empty()));
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
            assert_eq!(
                body["choices"][0]["message"]["content"],
                "hello from the mock"
            );
        }
        other => panic!("expected response second, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_model_infer_capture_reaches_the_debug_sink() {
    // F4: a nested infer called from Lua must route its request/response
    // capture to the run's owned debug sink instead of dropping it (was
    // hard-coded to `None`).
    let gateway = ScriptedGateway::start(vec![resp_text("final answer")]).await;
    let addr = gateway.addr();
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        writer = models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = models.infer(writer, 'say hello')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(
        &prompt,
        "",
        &[],
        &TestStore::new(),
        gatewayed_with_debug(addr, Arc::clone(&capture) as Arc<dyn DebugCapture>),
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
    let gateway = ScriptedGateway::start(vec![resp_text("arm reply")]).await;
    let addr = gateway.addr();
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
        gatewayed_with_debug(addr, Arc::clone(&capture) as Arc<dyn DebugCapture>),
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
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nAsk the model.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &TestStore::new(),
        gatewayed(addr),
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");
}

// --- Per-VM tools.calls count tests ---

#[tokio::test]
async fn tool_calls_count_increments_on_successful_dispatch() {
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let md = "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.call('echo', { value = 'x' })\n\
        assert(tools.calls['echo'] == 1, \
        'expected 1 call, got ' .. tostring(tools.calls['echo']))\n\
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
    // TESTS-002: drive a real `FailingTool` through a `models.loop` round
    // and prove `tools.calls` records exactly one call even though the tool
    // errors (the count is incremented before dispatch). The tool's own
    // failure is the call's error result, so the loop continues to the
    // terminal reply.
    use super::models_loop::{always_tool, loop_context, loop_prompt};
    use crate::test_support::tokio_driver::TokioDriver;

    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_x", "echo", "{\"value\":\"x\"}"),
        resp_text("final answer"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('ask the model')\n\
         models.loop(msgs)\n\
         return msgs[#msgs].content .. '|' .. tostring(tools.calls.echo)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, always_tool("echo", Arc::new(FailingTool)));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool's own failure becomes the call's result, not the loop's");
    assert_eq!(
        out, "final answer|1",
        "the counter must record exactly one call even though the tool errored"
    );
}

#[tokio::test]
async fn tool_calls_count_zero_for_uncalled_alias_fails_epilog_assert() {
    // The first script dispatch installs the counts seeded from the
    // effective scope, so an added but uncalled alias reads as 0 and an
    // author assert on it fails the run with its own message.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  search: tests/tools/search\n  other: tests/tools/other\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.add('search')\n\
        local _ = tools.call('other', { value = 'x' })\n\
        ```\n\n\
        ```lua\nassert(tools.calls['search'] > 0, 'search was never called')\n\
        return 'unreached'\n```\n";
    let search = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let other = ScopedFixtureTool::new("other", "canonical_other", "Other things.");
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  search: tests/tools/search\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.add('search')\n\
        local _ = tools.call('search', { value = 'x' })\n\
        ```\n\n\
        ```lua\nlocal _ = tools.calls['serach']\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let prompt = bound_with_tools(md);
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn TestTool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .expect_err("accessing a typo alias in tools.calls must hard error");
    let msg = error.to_string();
    assert!(
        msg.contains("serach") && msg.contains("has no seeded count"),
        "error must name the bad key and state it was never seeded: {msg}"
    );
    assert!(
        msg.contains("search"),
        "error must list the seeded aliases: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_infer_returns_text_without_touching_reply_or_sys() {
    // The one infer shape: `models.infer(handle, ...)` returns the round's text and never
    // sets `reply` or `sys.reply_finish_reason`.
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        writer = models.default('writer')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = models.infer(writer, 'say hello')\n\
        assert(type(text) == 'string', 'infer must return text')\n\
        assert(text == 'pong')\n\
        assert(reply == nil, 'infer must not set reply')\n\
        assert(not pcall(function() return sys.reply_finish_reason end),\n\
            'infer must not touch sys')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md);
    let out = run(&prompt, "", &[], &TestStore::new(), gatewayed(addr))
        .await
        .expect("handle-form infer must return text");
    assert_eq!(out, "pong");
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert!(
        body.get("tools").is_none(),
        "handle-form infer advertises no tools: {body}"
    );
}
