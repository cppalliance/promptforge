use super::super::*;
use super::*;
use crate::lua::OverflowReason;
use promptforge_lua::Compactor;

/// Runs the standard echo fixture with the requested loop cap.
async fn run_echo_loop(addr: SocketAddr, max_iterations: usize) -> Result<String> {
    let client = gateway_client(addr);
    let tools: Vec<Arc<dyn Tool>> = vec![Arc::new(EchoTool)];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "loop forever".to_string(),
        max_iterations,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .map(|(text, _)| text)
}

#[tokio::test]
async fn precheck_overflow_invokes_the_default_compactor_before_any_request() {
    // A 16-token window against a prose payload far past it: the precheck
    // fires, the omitted compactor defaults to `compactors.fail`, and no
    // request ever leaves.
    let gateway = ScriptedGateway::start(vec![resp_text("unreachable")]).await;
    let client = gateway_client(gateway.addr());
    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let mut conversation = Vec::new();
    let err = run_prose_inference(
        &client,
        &[],
        &BTreeMap::new(),
        &mut conversation,
        "x".repeat(4096),
        DEFAULT_MAX_TOOL_ITERATIONS,
        NonZeroU32::new(16).expect("16 is non-zero"),
        None,
        EXECUTION,
        recorder.as_ref(),
        "Only",
        &turns,
        None,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .expect_err("an over-window request must exhaust the context");
    assert!(
        matches!(
            err,
            Error::ContextExhausted {
                reason: OverflowReason::Precheck
            }
        ),
        "the default compactor raises typed precheck exhaustion, got {err:?}"
    );
    assert_eq!(
        gateway.call_count(),
        0,
        "the precheck fires before any request leaves"
    );
    assert_eq!(
        recorder.events(),
        vec![("Only".to_string(), detail::MODEL_TURN_FAILED.to_string())],
        "the refused dispatch is an operator-visible failed turn"
    );
}

#[tokio::test]
async fn provider_overflow_invokes_the_compactor_with_the_provider_reason() {
    let gateway = ScriptedGateway::start(vec![resp_status(
        400,
        "This model's maximum context length is 4096 tokens.",
    )])
    .await;
    let client = gateway_client(gateway.addr());
    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let mut conversation = Vec::new();
    let err = run_prose_inference(
        &client,
        &[],
        &BTreeMap::new(),
        &mut conversation,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        NonZeroU32::new(131_072).expect("non-zero"),
        Some(Compactor::Fail),
        EXECUTION,
        recorder.as_ref(),
        "Only",
        &turns,
        None,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .expect_err("a provider context rejection must exhaust the context");
    assert!(
        matches!(
            err,
            Error::ContextExhausted {
                reason: OverflowReason::Provider
            }
        ),
        "the compactor raises typed provider exhaustion, got {err:?}"
    );
    assert_eq!(gateway.call_count(), 1, "the request left and was rejected");
    assert_eq!(
        recorder.events(),
        vec![("Only".to_string(), detail::MODEL_TURN_FAILED.to_string())]
    );
}

#[tokio::test]
async fn a_client_rejection_without_overflow_signatures_stays_a_backend_error() {
    // Same status class, unrelated body: not context overflow, so the bare
    // backend failure propagates and no compactor is invoked.
    let gateway =
        ScriptedGateway::start(vec![resp_status(400, "invalid request: unknown field")]).await;
    let client = gateway_client(gateway.addr());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let mut conversation = Vec::new();
    let err = run_prose_inference(
        &client,
        &[],
        &BTreeMap::new(),
        &mut conversation,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        NonZeroU32::new(131_072).expect("non-zero"),
        None,
        EXECUTION,
        &NullObserver::default(),
        "Only",
        &turns,
        None,
        &options,
        &nonce,
        None,
        None,
        None,
    )
    .await
    .expect_err("an ordinary backend rejection must propagate unchanged");
    assert!(
        matches!(err, Error::Backend { status: 400, .. }),
        "a non-overflow 400 stays a backend error, got {err:?}"
    );
}

#[tokio::test]
async fn tool_loop_gives_up_after_exactly_the_configured_cap() {
    // A small explicit cap: the loop must make exactly that many round
    // trips against a never-converging model, then exhaust.
    let cap = 3;
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let err = run_echo_loop(gateway.addr(), cap)
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
    let err = run_echo_loop(gateway.addr(), DEFAULT_MAX_TOOL_ITERATIONS)
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
    let p = Prompt::parse(declared, EXECUTION, &NullObserver::default()).unwrap();
    assert_eq!(
        p.frontmatter()
            .max_tool_iterations()
            .resolve(DEFAULT_MAX_TOOL_ITERATIONS),
        5
    );

    let absent = "---\nname: t\ndescription: d\n---\n\n# T\n\n## S\n\np\n";
    let p = Prompt::parse(absent, EXECUTION, &NullObserver::default()).unwrap();
    assert_eq!(
        p.frontmatter()
            .max_tool_iterations()
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
    let client = gateway_client(addr);

    // Advertise schemas so the request carries tools, but pass no dispatch
    // targets, so the returned call resolves to no tool.
    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let schemas = schemas_for(&[echo]);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let err = run_tool_loop(
        &client,
        &schemas,
        &BTreeMap::new(),
        "call unknown".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
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
async fn a_failing_tool_becomes_an_untrusted_error_result_and_the_loop_continues() {
    // A bound tool's own failure is the call's result record - the ToolError
    // message, nonce-wrapped as untrusted - with TOOL_CALL_FAILED firing
    // alongside it, and the loop continues to the terminal reply.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_x", "echo", "{\"value\":\"x\"}"),
        resp_text("final answer"),
    ])
    .await;
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
    let (out, _) = run_tool_loop(
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
        None,
        None,
        None,
    )
    .await
    .expect("a tool's own failure becomes the call's result, not the loop's");
    assert_eq!(out, "final answer", "the loop continues to the terminal reply");

    // The result record carries the tool's error message, guard-wrapped as
    // untrusted content.
    let content = last_tool_turn_content(&gateway.requests());
    assert!(
        content.contains("the tool's own backend failed"),
        "the result record must carry the tool's error message, got: {content}"
    );
    assert!(
        content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
        "the error result must be nonce-wrapped as untrusted, got: {content}"
    );

    // TOOL_CALL_FAILED fires alongside the error result, and the loop runs a
    // second completed turn to the terminal reply.
    assert_eq!(
        recorder.events(),
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            ("Gather".to_string(), detail::TOOL_CALL_FAILED.to_string(),),
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
        ],
        "the failed dispatch is reported and the loop continues"
    );
    assert!(
        recorder
            .records()
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION)
    );
}

#[tokio::test]
async fn a_counts_failure_still_aborts_the_loop() {
    // The counts increment runs before the tool call; an alias that was
    // never seeded fails there, and that quota-layer failure still aborts
    // the loop rather than becoming a tool result.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_x", "echo", "{\"value\":\"x\"}"),
        resp_text("unreachable"),
    ])
    .await;
    let client = gateway_client(gateway.addr());

    let echo: Arc<dyn Tool> = Arc::new(EchoTool);
    let tools: Vec<Arc<dyn Tool>> = vec![echo];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    // Seeded with a different alias: the increment for "echo" fails.
    let counts = ToolCallCounts::new(["other".to_string()]);
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask the model".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
        Some(&counts),
        None,
        None,
    )
    .await
    .expect_err("a counts failure must abort the loop");
    assert!(
        err.to_string().contains("was not pre-seeded"),
        "the counts failure propagates unchanged, got: {err}"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the loop aborted on the first dispatch"
    );
}

#[tokio::test]
async fn repeated_calls_to_a_failing_tool_exit_at_the_iteration_cap() {
    // Every round's failing call becomes an error result, so a model that
    // keeps calling the failing tool never converges: the loop exits at
    // exactly `max_tool_iterations`.
    let cap = 3;
    let gateway =
        ScriptedGateway::start(vec![resp_tool_call("call_x", "echo", "{\"value\":\"x\"}")]).await;
    let addr = gateway.addr();
    let client = gateway_client(addr);

    let failing: Arc<dyn Tool> = Arc::new(FailingTool);
    let tools: Vec<Arc<dyn Tool>> = vec![failing];
    let schemas = schemas_for(&tools);
    let dispatch = dispatch_for(&tools);

    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let nonce = GuardNonce::fresh();
    let err = run_tool_loop(
        &client,
        &schemas,
        &dispatch,
        "ask the model".to_string(),
        cap,
        &NullObserver::default(),
        "Only",
        &turns,
        &options,
        &nonce,
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
        "each round answers the failing call and loops, exiting at the cap"
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
    let nonce = GuardNonce::fresh();
    let error = run_tool_loop(
        &client,
        &[],
        &BTreeMap::new(),
        "private model input".to_string(),
        DEFAULT_MAX_TOOL_ITERATIONS,
        recorder.as_ref(),
        "Gather",
        &turns,
        &options,
        &nonce,
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
