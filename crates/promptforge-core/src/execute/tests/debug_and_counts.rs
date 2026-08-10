use super::super::*;
use super::run;
use super::*;

#[tokio::test]
async fn debug_capture_receives_request_and_response_when_set() {
    let addr = spawn_text_gateway().await;
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nAsk the model.\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: Some(Arc::clone(&capture) as Arc<dyn DebugCapture>),
        },
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
fn bridge_blocking_rejects_a_current_thread_runtime_instead_of_panicking() {
    // F3: the sync-to-async bridge must NOT panic on a current-thread runtime
    // (as raw `block_in_place` would); it returns a concrete error first.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime builds");
    let result: Result<()> = rt.block_on(async { bridge_blocking(async { Ok::<(), Error>(()) }) });
    match result {
        Err(Error::Internal(message)) => assert!(
            message.contains("multi-threaded"),
            "the error must explain the runtime requirement: {message}"
        ),
        other => panic!("expected a concrete Internal error, got {other:?}"),
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
    // F4: a nested `model:infer` called from Lua must route its request/response
    // capture to the run's owned debug sink instead of dropping it (was
    // hard-coded to `None`).
    let addr = spawn_mock_gateway().await;
    let echo = Arc::new(EchoTool);
    let capture = Arc::new(RecordingCapture::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        writer = models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = writer:infer('say hello')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&echo) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: Some(Arc::clone(&capture) as Arc<dyn DebugCapture>),
        },
    )
    .await
    .expect("model:infer must return text");
    assert_eq!(out, "final answer");

    let events = capture.events();
    assert!(
        !events.is_empty(),
        "nested model:infer must reach the debug sink (F4), got no events"
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

#[tokio::test]
async fn debug_capture_none_changes_nothing() {
    let addr = spawn_text_gateway().await;
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\nAsk the model.\n";
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");
}

// --- Per-VM tools.calls count tests ---

#[tokio::test]
async fn tool_calls_count_increments_on_successful_dispatch() {
    let (addr, _, _) = spawn_aliased_tool_gateway("echo").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n\n\
        ```lua\nassert(tools.calls['echo'] == 1, \
        'expected 1 call, got ' .. tostring(tools.calls['echo']))\n\
        return 'ok'\n```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
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
    let (addr, _calls) = spawn_always_tool_call().await;
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let failing = FailingTool;
    let tools: &[&dyn Tool] = &[&failing];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    // The gateway always calls the tool wired as "echo".
    let counts = ToolCallCounts::new(["echo".to_string()]);

    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: recorder.as_ref(),
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        Some(&counts),
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
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('search', 'search tool')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\ntools.add('search')\n```\n\n\
        ```lua\nassert(tools.calls['search'] > 0, 'search was never called')\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let addr = spawn_text_gateway().await;
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect_err("epilog assert on zero count must fail the run");
    assert!(
        error.to_string().contains("search was never called"),
        "error must carry the assert message: {error}"
    );
}

#[tokio::test]
async fn tool_calls_typo_alias_is_a_hard_error_with_in_scope_set() {
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('search', 'search tool')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n```lua\ntools.add('search')\n```\n\n\
        ```lua\nlocal _ = tools.calls['serach']\n\
        return 'unreached'\n```\n";
    let tool = ScopedFixtureTool::new("search", "canonical_search", "Search for things.");
    let addr = spawn_text_gateway().await;
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect_err("accessing a typo alias in tools.calls must hard error");
    let msg = error.to_string();
    assert!(
        msg.contains("serach") && msg.contains("not in this section's tool scope"),
        "error must name the bad key and state it's out of scope: {msg}"
    );
    assert!(
        msg.contains("search"),
        "error must list in-scope aliases: {msg}"
    );
}

#[tokio::test]
async fn model_calling_global_but_unscoped_tool_is_a_hard_error() {
    let (addr, _, _) = spawn_aliased_tool_gateway("global_tool").await;
    let scoped = ScopedFixtureTool::new("scoped", "canonical_scoped", "A scoped tool.");
    let global = ScopedFixtureTool::new("global_tool", "canonical_global", "A global tool.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('scoped', 'scoped tool')\n\
        tools.need('global_tool', 'global tool')\n\
        tools.always('scoped')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n";
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[
            Arc::new(scoped) as Arc<dyn Tool>,
            Arc::new(global) as Arc<dyn Tool>,
        ],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
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
            assert!(*global_exists, "the alias was declared by tools.need");
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
        msg.contains("declared by tools.need but not added"),
        "error message must hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test]
async fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let (addr, _, _) = spawn_aliased_tool_gateway("nonexistent").await;
    let tool = ScopedFixtureTool::new("echo", "canonical_echo", "Echo a test value.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\nUse the tool.\n";
    let prompt = bound_with_tools(md, Vec::new());
    let error = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
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
                "the alias was never declared by tools.need"
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
        !msg.contains("declared by tools.need but not added"),
        "pure unknown must not hint declared-but-unscoped: {msg}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn model_infer_single_shot_returns_text() {
    let addr = spawn_mock_gateway().await;
    let echo = Arc::new(EchoTool);
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
        # Test prompt\n\n```lua shared\n\
        tools.need('echo', 'echo tool')\n\
        tools.always('echo')\n\
        writer = models.always('writer', 'A general model for tests')\n```\n\n\
        ## Only\n\n\
        ```lua\n\
        local text = writer:infer('say hello')\n\
        assert(type(text) == 'string', 'infer must return text')\n\
        assert(text == 'final answer')\n\
        assert(reply == text, 'infer must set reply')\n\
        assert(tools.calls['echo'] == 1, 'infer tool loop must increment tools.calls')\n\
        return text\n\
        ```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::clone(&echo) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("model:infer single-shot must return text");
    assert_eq!(out, "final answer");
}
