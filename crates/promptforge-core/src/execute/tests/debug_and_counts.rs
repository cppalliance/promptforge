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
        crate::debug::DebugEvent::Request { body } => {
            assert_eq!(body["model"], "claude-sonnet-4-6");
            assert!(body["messages"].as_array().is_some_and(|m| !m.is_empty()));
        }
        other => panic!("expected request first, got {other:?}"),
    }
    match &events[1].3 {
        crate::debug::DebugEvent::Response {
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

#[test]
fn gateway_source_resolves_ready_and_preserves_the_env_error() {
    // F5: lazy client acquisition is centralized. A ready source resolves to its
    // client; a missing client becomes an `Env` source whose resolution mirrors
    // `env_client_with_limits` (same Ok/Err disposition), so a construction
    // failure is preserved as an error rather than swallowed with `.ok()`.
    let limits = RunLimits::new();
    let client = GatewayClient::new(
        GatewayEndpoint::new("http://localhost/v1").expect("valid endpoint"),
        SecretString::new("k").expect("non-empty test key"),
    );
    let ready = GatewaySource::from_optional(Some(client), limits);
    assert!(
        ready.resolve().is_ok(),
        "a ready source must resolve to its client"
    );

    let env_source = GatewaySource::from_optional(None, limits);
    assert!(
        matches!(env_source, GatewaySource::Env(_)),
        "a missing client must defer to an environment source"
    );
    assert_eq!(
        env_source.resolve().is_err(),
        env_client_with_limits(limits).is_err(),
        "the env source must preserve the construction result, not swallow it"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nested_model_infer_capture_reaches_the_debug_sink() {
    // F4: a nested infer called from Lua must route its request/response
    // capture to the run's owned debug sink instead of dropping it (was
    // hard-coded to `None`).
    let gateway = ScriptedGateway::start(vec![resp_text("final answer")]).await;
    let addr = gateway.addr();
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        writer = models.default('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = models.infer(writer, 'say hello')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md, Vec::new());
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
        events
            .iter()
            .any(|event| matches!(event.3, crate::debug::DebugEvent::Request { .. })),
        "nested inference must capture at least one request: {events:#?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.3, crate::debug::DebugEvent::Response { .. })),
        "nested inference must capture at least one response: {events:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_debug_events_reach_the_run_sink() {
    // The arm's debug side channel: the fanout's run-context fork carries
    // the run's own debug sink, so an arm's model-turn events land on the
    // run's sink under the worker's section name.
    let gateway = ScriptedGateway::start(vec![resp_text("arm reply")]).await;
    let addr = gateway.addr();
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        models.default('writer', 'A general model for tests')\n```\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        Reply about {{ item }}.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = bound_with_tools(md, Vec::new());
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
        worker_events
            .iter()
            .any(|event| matches!(event.3, crate::debug::DebugEvent::Request { .. })),
        "the arm's request must forward to the run's sink: {events:#?}"
    );
    assert!(
        worker_events
            .iter()
            .any(|event| matches!(event.3, crate::debug::DebugEvent::Response { .. })),
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.bind('echo', 'echo tool')\n\
        models.default('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        tools.call('echo', { value = 'x' })\n\
        assert(tools.calls['echo'] == 1, \
        'expected 1 call, got ' .. tostring(tools.calls['echo']))\n\
        return 'ok'\n\
        ```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &TestStore::new(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "ok");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn tool_calls_count_increments_even_when_tool_errors() {
    // TESTS-002: drive a real `FailingTool` through `run_tool_loop` and prove the
    // counter records exactly one call even though the tool errors (the count is
    // incremented before dispatch), and that the tool's backend error still ends
    // the loop. The old version poked `ToolCallCounts` directly and dispatched no
    // tool at all.
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let failing: Arc<dyn Tool> = Arc::new(FailingTool);
    let tools: Vec<Arc<dyn Tool>> = vec![failing];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    // The gateway always calls the tool wired as "echo".
    let counts = ToolCallCounts::new(["echo".to_string()]);

    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        recorder.as_ref(),
        "Gather",
        &turns,
        &options,
        &nonce,
        Some(&counts),
        None,
        None,
    )
    .await
    .expect_err("a tool whose call fails must fail the loop");

    match &err {
        Error::Tool { message, .. } => assert!(
            message.contains("the tool's own backend failed"),
            "the tool's own backend error must propagate: {message}"
        ),
        other => panic!("expected the tool's own backend error, got {other:?}"),
    }

    assert_eq!(
        counts.get("echo").expect("echo is a tracked alias"),
        Some(1),
        "the counter must record exactly one call even though the tool errored"
    );
}

#[tokio::test]
async fn tool_calls_count_zero_for_uncalled_alias_fails_epilog_assert() {
    // The first script dispatch installs the counts seeded from the
    // effective scope, so an added but uncalled alias reads as 0 and an
    // author assert on it fails the run with its own message.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.bind('search', 'search tool')\n\
        tools.bind('other', 'other tool')\n\
        models.default('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.add('search')\n\
        local _ = tools.call('other', { value = 'x' })\n\
        ```\n\n\
        ```lua\nassert(tools.calls['search'] > 0, 'search was never called')\n\
        return 'unreached'\n```\n";
    let search = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let other = ScopedFixtureTool::new("other", "canonical_other", "Other things.");
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[
            Arc::new(search) as Arc<dyn Tool>,
            Arc::new(other) as Arc<dyn Tool>,
        ],
        &TestStore::new(),
        silent(),
    )
    .await
    .expect_err("epilog assert on zero count must fail the run");
    assert!(
        error.to_string().contains("search was never called"),
        "error must carry the assert message: {error}"
    );
}

#[tokio::test]
async fn tool_calls_typo_alias_is_a_hard_error_with_seeded_set() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.bind('search', 'search tool')\n\
        models.default('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\n\
        tools.add('search')\n\
        local _ = tools.call('search', { value = 'x' })\n\
        ```\n\n\
        ```lua\nlocal _ = tools.calls['serach']\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
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

#[tokio::test]
async fn model_calling_global_but_unscoped_tool_is_a_hard_error() {
    // The loop's scope gate: a model call naming a declared-but-unscoped
    // alias fails with OutOfScopeToolCall carrying the
    // declared-but-unscoped hint. Driven at the loop directly; the
    // prompt-level wiring returns with the `models.loop` step.
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "global_tool",
        "{\"value\":\"x\"}",
    )])
    .await;
    let client = gateway_client(gateway.addr());
    let scoped: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "scoped",
        "canonical_scoped",
        "A scoped tool.",
    ));
    let global: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "global_tool",
        "canonical_global",
        "A global tool.",
    ));
    let schemas = vec![
        ToolSchema::new(
            "scoped".to_string(),
            "A scoped tool.".to_string(),
            scoped.parameters_schema(),
        )
        .expect("the scoped schema is valid"),
    ];
    let mut dispatch = BTreeMap::new();
    dispatch.insert(
        "scoped".to_string(),
        DispatchTarget::Bound(crate::lua::ToolBinding::for_test(
            "scoped",
            "A scoped tool.",
            Arc::clone(&scoped),
        )),
    );
    let mut global_aliases = BTreeMap::new();
    global_aliases.insert("scoped".to_string(), scoped.id());
    global_aliases.insert("global_tool".to_string(), global.id());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let error = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        Some(&global_aliases),
        None,
    )
    .await
    .expect_err("model calling a global-but-unscoped tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "global_tool");
            assert!(*global_exists, "the alias was declared by tools.bind");
            assert!(
                in_scope.contains(&"scoped".to_string()),
                "in_scope must list the scoped alias: {in_scope:?}"
            );
            assert!(
                !in_scope.contains(&"global_tool".to_string()),
                "global_tool must not be in scope: {in_scope:?}"
            );
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    let msg = error.to_string();
    assert!(
        msg.contains("declared by tools.bind but not added"),
        "error message must hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test]
async fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "nonexistent",
        "{\"value\":\"x\"}",
    )])
    .await;
    let client = gateway_client(gateway.addr());
    let echo: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let schemas = vec![
        ToolSchema::new(
            "echo".to_string(),
            "Echo a test value.".to_string(),
            echo.parameters_schema(),
        )
        .expect("the echo schema is valid"),
    ];
    let mut dispatch = BTreeMap::new();
    dispatch.insert(
        "echo".to_string(),
        DispatchTarget::Bound(crate::lua::ToolBinding::for_test(
            "echo",
            "Echo a test value.",
            Arc::clone(&echo),
        )),
    );
    let mut global_aliases = BTreeMap::new();
    global_aliases.insert("echo".to_string(), echo.id());

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let error = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "Use the tool.".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        Some(&global_aliases),
        None,
    )
    .await
    .expect_err("model calling a pure unknown tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "nonexistent");
            assert!(
                !*global_exists,
                "the alias was never declared by tools.bind"
            );
            assert!(
                in_scope.contains(&"echo".to_string()),
                "in_scope must list the scoped alias: {in_scope:?}"
            );
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    let msg = error.to_string();
    assert!(
        !msg.contains("declared by tools.bind but not added"),
        "pure unknown must not hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_infer_returns_text_without_touching_reply_or_sys() {
    // The one infer shape: `models.infer(handle, ...)` returns the round's text and never
    // sets `reply` or `sys.reply_finish_reason`.
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        writer = models.default('writer', 'A general model for tests')\n```\n\n\
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
    let prompt = bound_with_tools(md, Vec::new());
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
