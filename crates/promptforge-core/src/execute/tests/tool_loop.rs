use super::super::*;
use super::*;

#[tokio::test]
async fn tool_loop_gives_up_after_exactly_the_configured_cap() {
    // A small explicit cap: the loop must make exactly that many round
    // trips against a never-converging model, then exhaust.
    let cap = 3;
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "loop forever".to_string(),
        cap,
        silent_progress(&turns, &options),
        None,
        None,
        None,
    )
    .await
    .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(err, Error::ToolLoopExhausted));
    assert_eq!(
        gateway.call_count(),
        cap,
        "the loop must make exactly `cap` round trips before giving up"
    );
}

#[tokio::test]
async fn tool_loop_uses_the_default_cap_when_unspecified() {
    // Threading `DEFAULT_MAX_TOOL_ITERATIONS` (what `run` passes when a
    // prompt declares no budget) makes exactly that many round trips.
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    let echo = EchoTool;
    let tools: &[&dyn Tool] = &[&echo];
    let schemas = schemas_for(tools);
    let dispatch = dispatch_for(tools);
    let registry = ToolRegistry::new(tools.iter().copied()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        &registry,
        "loop forever".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
        None,
    )
    .await
    .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(err, Error::ToolLoopExhausted));
    assert_eq!(gateway.call_count(), DEFAULT_MAX_TOOL_ITERATIONS);
    assert_eq!(DEFAULT_MAX_TOOL_ITERATIONS, 24);
}

#[test]
fn run_resolves_cap_from_frontmatter_else_default() {
    // Mirrors the resolution in `run`: a declared budget wins, an absent
    // one falls back to the raised default.
    let declared =
        "---\nname: t\ndescription: d\nmax_tool_iterations: 5\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(declared, EXECUTION, &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter
            .max_tool_iterations
            .resolve(DEFAULT_MAX_TOOL_ITERATIONS),
        5
    );

    let absent = "---\nname: t\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(absent, EXECUTION, &NullObserver).unwrap();
    assert_eq!(
        p.frontmatter
            .max_tool_iterations
            .resolve(DEFAULT_MAX_TOOL_ITERATIONS),
        DEFAULT_MAX_TOOL_ITERATIONS
    );
}

#[tokio::test]
async fn tool_loop_errors_on_unknown_tool() {
    // The model asks for "echo" but no tools are provided to the loop.
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("test").expect("non-empty test key"),
    );

    // Advertise schemas so the request carries tools, but pass no dispatch
    // targets, so the returned call resolves to no tool.
    let echo = EchoTool;
    let schemas = schemas_for(&[&echo]);
    let registry = ToolRegistry::new(std::iter::empty()).expect("unique test registry");

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let err = run_tool_loop(
        &client,
        &schemas,
        &BTreeMap::new(),
        &registry,
        "call unknown".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        silent_progress(&turns, &options),
        None,
        None,
        None,
    )
    .await
    .expect_err("an unprovided tool should be rejected");
    match err {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "echo");
            assert!(!global_exists);
            assert!(in_scope.is_empty());
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
}

#[tokio::test]
async fn a_failing_tool_is_reported_before_the_error_propagates() {
    // The dispatch is split from the `?` precisely so a tool that fails is
    // still reported: the recorder must see `ToolCalled { ok: false }` and
    // the tool's own error must still end the loop.
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
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
        None,
        None,
        None,
    )
    .await
    .expect_err("a tool whose call fails must fail the loop");
    match &err {
        Error::Tool { message, .. } => assert!(
            message.contains("the tool's own backend failed"),
            "the tool's own error propagates: {message}"
        ),
        other => panic!("expected the tool's own error, got {other:?}"),
    }
    // The tool's error (and its own inner cause) must be preserved as the
    // error's source chain, not discarded when bridged into the run error.
    let source = std::error::Error::source(&err).expect("tool error is kept as the source");
    let chain = std::iter::successors(Some(source), |error| error.source())
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" -> ");
    assert!(
        chain.contains("upstream socket reset"),
        "the tool's inner cause must survive in the source chain, got: {chain}"
    );

    assert_eq!(
        recorder.events(),
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            ("Gather".to_string(), detail::TOOL_CALL_FAILED.to_string(),),
        ],
        "the failed dispatch must be reported before the error propagates"
    );
    assert!(
        recorder
            .records()
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION)
    );
}

#[tokio::test]
async fn a_failing_model_turn_is_reported_before_the_error_propagates() {
    let gateway = ScriptedGateway::start(vec![resp_status(500, "private backend response")]).await;
    let addr = gateway.addr();

    let client = GatewayClient::new(
        GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
        SecretString::new("secret token").expect("non-empty test key"),
    );
    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let error = run_tool_loop(
        &client,
        &[],
        &BTreeMap::new(),
        &ToolRegistry::new(std::iter::empty()).expect("unique test registry"),
        "private model input".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        SectionProgress {
            execution: EXECUTION,
            observer: recorder.as_ref(),
            section: "Gather",
            turns: &turns,
            debug: None,
            completion_options: &options,
        },
        None,
        None,
        None,
    )
    .await
    .expect_err("the backend failure must propagate");

    assert!(matches!(error, Error::Backend { status: 500, .. }));
    assert_eq!(
        recorder.events(),
        vec![("Gather".to_string(), detail::MODEL_TURN_FAILED.to_string(),)]
    );
    let trace = format!("{:?}", recorder.events());
    for payload in [
        "private backend response",
        "private model input",
        "secret token",
    ] {
        assert!(!trace.contains(payload), "observation leaked {payload:?}");
    }
}
