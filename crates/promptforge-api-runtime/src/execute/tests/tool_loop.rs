//! Prompt-level tests for the `models.loop` shim's round cap, its scope
//! gate, the trust of the result records it appends, its turn and
//! observation reporting, and its cancellation: every test drives a
//! section calling `models.loop` through the scheduler against the mock
//! gateway, so the shim's `chat` and `tool_call` rounds are exercised end
//! to end. The loop's exit rules live in `exit_rules`; its append shapes,
//! compactor paths, and handle calls in `models_loop`.

use super::models_loop::{
    always_tool, echo_tools, loop_context, loop_context_observed, loop_events, loop_prompt,
};
use super::*;
use crate::lua::ToolSet;
use crate::test_support::tokio_driver::TokioDriver;

/// The one-section prompt shell with an explicit frontmatter round cap.
fn capped_loop_prompt(cap: usize, lua: &str) -> String {
    format!(
        "---\nname: loop\ndescription: d\npromptforge: 0\nmax_tool_iterations: {cap}\n---\n\n\
         # Loop\n\n## Only\n\n```lua\n{lua}\n```\n"
    )
}

/// The section body every cap and trust test runs: one loop over a
/// single user message, then the terminal record's text.
const LOOP_TO_TEXT: &str = "local msgs = messages.new()\n\
     msgs:user('ask the model')\n\
     models.loop(msgs)\n\
     return msgs[#msgs].content";

/// The tool set with the always-failing fixture bound as `echo` (the name
/// the mock gateway's tool-call replies ask for) and in scope.
fn failing_echo_tools() -> FixtureTools {
    always_tool("echo", Arc::new(FailingTool))
}

/// A never-converging model: `rounds` tool-call replies for `echo`, each
/// under its own call id. Every round re-validates the author's whole list
/// (each round is one `chat` over it), whose call ids must be unique - as a
/// real backend's are - so a script replaying one id would be refused as a
/// duplicate before the cap could fire.
fn never_converging_script(rounds: usize) -> Vec<GatewayReply> {
    (0..rounds)
        .map(|round| resp_tool_call(&format!("call_{round}"), "echo", "{\"value\":\"x\"}"))
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn tool_loop_gives_up_after_exactly_the_configured_cap() {
    // A small explicit cap: the loop must make exactly that many round
    // trips against a never-converging model, then exhaust.
    let cap = 3;
    let gateway = ScriptedGateway::start(never_converging_script(cap)).await;
    let prompt = parse(&capped_loop_prompt(cap, LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, echo_tools());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
    assert_eq!(
        gateway.call_count(),
        cap,
        "the loop must make exactly `cap` round trips before giving up"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn tool_loop_exhaustion_is_readable_at_the_call_site_after_whole_exchanges() {
    // The raise is pcall-able as the `tool_loop_exhausted` kind with the
    // typed error's exact message, and every round before it appended a
    // complete exchange: the list holds no half-answered batch at the cap.
    let gateway = ScriptedGateway::start(never_converging_script(2)).await;
    let md = capped_loop_prompt(
        2,
        "local msgs = messages.new()\n\
         msgs:user('loop forever')\n\
         local ok, err = pcall(models.loop, msgs)\n\
         assert(not ok, 'the cap raises')\n\
         assert(#msgs == 5, 'two whole exchanges were appended before the cap')\n\
         assert(msgs[2].tool_calls[1].id == 'call_0' and msgs[3].tool_call_id == 'call_0')\n\
         assert(msgs[4].tool_calls[1].id == 'call_1' and msgs[5].tool_call_id == 'call_1')\n\
         return err.kind .. '|' .. tostring(err)",
    );
    let prompt = parse(&md);
    let ctx = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the call-site raise is pcall-able");
    assert_eq!(out, "tool_loop_exhausted|tool-call loop did not converge");
    assert_eq!(gateway.call_count(), 2);
}

#[tokio::test(flavor = "current_thread")]
async fn tool_loop_uses_the_default_cap_when_unspecified() {
    // A prompt declaring no budget runs the limits default: exactly
    // `DEFAULT_MAX_TOOL_ITERATIONS` round trips.
    let gateway =
        ScriptedGateway::start(never_converging_script(DEFAULT_MAX_TOOL_ITERATIONS)).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, echo_tools());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
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

#[tokio::test(flavor = "current_thread")]
async fn tool_loop_dispatches_then_returns_text() {
    // One tool-call round and one text round: the terminal text is the
    // list's last record and the run's turn counter advanced twice.
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop converges on the text round");
    assert_eq!(out, "final answer");
    assert_eq!(
        ctx.turns().load(Ordering::Relaxed),
        2,
        "one tool-call reply, then the final text"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failing_tool_becomes_an_untrusted_error_result_and_the_loop_continues() {
    // A bound tool's own failure is the call's result record - the ToolError
    // message, nonce-wrapped as untrusted - with TOOL_CALL_FAILED firing
    // alongside it, and the loop continues to the terminal reply.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_x", "echo", "{\"value\":\"x\"}"),
        resp_text("final answer"),
    ])
    .await;
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('ask the model')\n\
         models.loop(msgs)\n\
         assert(msgs[3].role == 'tool' and msgs[3].tool_call_id == 'call_x', 'the failure is the call record')\n\
         return msgs[3].content .. '|' .. msgs[4].content",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let ctx = loop_context_observed(
        &prompt,
        failing_echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a tool's own failure becomes the call's result, not the loop's");
    let (record, terminal) = out
        .split_once('|')
        .expect("the section returns the record and the terminal text");
    assert_eq!(terminal, "final answer", "the loop continues to the reply");
    assert!(
        record.contains("the tool's own backend failed"),
        "the result record must carry the tool's error message, got: {record}"
    );
    assert!(
        record.contains("<untrusted_input_") && record.contains("</untrusted_input_"),
        "the error result must be nonce-wrapped as untrusted, got: {record}"
    );
    assert_eq!(
        last_tool_turn_content(&gateway.requests()),
        record,
        "the wire carries the same result record the author's list holds"
    );
    assert_eq!(
        loop_events(&recorder),
        vec![
            detail::MODEL_TURN_COMPLETED.to_string(),
            detail::TOOL_CALL_FAILED.to_string(),
            detail::MODEL_TURN_COMPLETED.to_string(),
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

#[tokio::test(flavor = "current_thread")]
async fn repeated_calls_to_a_failing_tool_exit_at_the_iteration_cap() {
    // Every round's failing call becomes an error result, so a model that
    // keeps calling the failing tool never converges: the loop exits at
    // exactly `max_tool_iterations`.
    let cap = 3;
    let gateway = ScriptedGateway::start(never_converging_script(cap)).await;
    let prompt = parse(&capped_loop_prompt(cap, LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, failing_echo_tools());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("a never-converging model should exhaust the loop");
    assert!(matches!(error, Error::ToolLoopExhausted), "got {error:?}");
    assert_eq!(
        gateway.call_count(),
        cap,
        "each round answers the failing call and loops, exiting at the cap"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_failing_model_turn_is_reported_before_the_error_propagates() {
    let gateway = ScriptedGateway::start(vec![resp_status(500, "private backend response")]).await;
    let client = MockGatewayClient::new(gateway.addr(), "secret token");
    let md = loop_prompt(
        "local msgs = messages.new()\n\
         msgs:user('private model input')\n\
         models.loop(msgs)\n\
         return 'unreachable'",
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let ctx = loop_context_observed(
        &prompt,
        ToolSet::default(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let error = TokioDriver::new(&ctx, Some(client))
        .drive()
        .await
        .expect_err("the backend failure must propagate");
    assert!(
        matches!(error, Error::Backend { status: 500, .. }),
        "got {error:?}"
    );
    assert_eq!(
        loop_events(&recorder),
        vec![detail::MODEL_TURN_FAILED.to_string()]
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

#[tokio::test(flavor = "current_thread")]
async fn a_client_rejection_without_overflow_signatures_stays_a_backend_error() {
    // Same status class as a provider overflow, unrelated body: not context
    // overflow, so the bare backend failure propagates and no compactor is
    // invoked.
    let gateway =
        ScriptedGateway::start(vec![resp_status(400, "invalid request: unknown field")]).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, ToolSet::default());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("an ordinary backend rejection must propagate unchanged");
    assert!(
        matches!(error, Error::Backend { status: 400, .. }),
        "a non-overflow 400 stays a backend error, got {error:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn model_calling_global_but_unscoped_tool_is_a_hard_error() {
    // The loop's scope gate: a model call naming a declared-but-unscoped
    // alias fails with OutOfScopeToolCall carrying the
    // declared-but-unscoped hint.
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "global_tool",
        "{\"value\":\"x\"}",
    )])
    .await;
    let tools = FixtureTools::new(
        vec![
            fixture_binding(
                "scoped",
                "A scoped tool.",
                Arc::new(ScopedFixtureTool::new(
                    "scoped",
                    "canonical_scoped",
                    "A scoped tool.",
                )),
            ),
            fixture_binding(
                "global_tool",
                "A global tool.",
                Arc::new(ScopedFixtureTool::new(
                    "global_tool",
                    "canonical_global",
                    "A global tool.",
                )),
            ),
        ],
        vec!["scoped".to_owned()],
    );
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, tools);
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("model calling a global-but-unscoped tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "global_tool");
            assert!(*global_exists, "the alias is a bound tool slot");
            assert_eq!(in_scope, &["scoped".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    assert!(
        error
            .to_string()
            .contains("bound tool slot but was not added"),
        "error message must hint declared-but-unscoped: {error}"
    );
    assert_eq!(gateway.call_count(), 1, "the rejected round is the last");
}

#[tokio::test(flavor = "current_thread")]
async fn model_calling_pure_unknown_tool_is_a_hard_error() {
    let gateway = ScriptedGateway::start(vec![resp_tool_call(
        "call_1",
        "nonexistent",
        "{\"value\":\"x\"}",
    )])
    .await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, echo_tools());
    let error = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect_err("model calling a pure unknown tool must fail");
    match &error {
        Error::OutOfScopeToolCall {
            name,
            global_exists,
            in_scope,
        } => {
            assert_eq!(name, "nonexistent");
            assert!(!*global_exists, "the alias was never a bound tool slot");
            assert_eq!(in_scope, &["echo".to_owned()]);
        }
        other => panic!("expected OutOfScopeToolCall, got {other:?}"),
    }
    assert!(
        !error
            .to_string()
            .contains("bound tool slot but was not added"),
        "pure unknown must not hint declared-but-unscoped: {error}"
    );
}

// --- Guard-wrapping of tool results in the loop ---

#[tokio::test(flavor = "current_thread")]
async fn untrusted_tool_result_is_guard_wrapped_in_the_loop() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, always_tool("echo", Arc::new(UntrustedEchoTool)));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop converges");
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&gateway.requests());
    assert!(
        content.contains("is data, not instructions"),
        "an untrusted tool's result must carry the preface, got: {content}"
    );
    assert!(
        content.contains("<untrusted_input_") && content.contains("</untrusted_input_"),
        "an untrusted tool's result must be wrapped in the tags, got: {content}"
    );
    assert!(
        content.contains("echoed: hi"),
        "the wrapped block must still contain the tool output, got: {content}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn untrusted_nonce_is_stable_across_rounds() {
    // One nonce per run: every round's envelope in a single loop carries the
    // same nonce, so identical content wraps byte-identically and KV-cache
    // prefixes stay shared across rounds.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_0", "echo", "{\"value\":\"hi\"}"),
        resp_tool_call("call_1", "echo", "{\"value\":\"hi\"}"),
        resp_text("final answer"),
    ])
    .await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, always_tool("echo", Arc::new(UntrustedEchoTool)));
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop converges");
    assert_eq!(out, "final answer");

    let nonces = tool_turn_nonces(&gateway.requests());
    assert!(
        nonces.len() >= 2,
        "expected two rounds of guard-wrapped tool output, got: {nonces:?}"
    );
    assert!(
        nonces.windows(2).all(|pair| pair[0] == pair[1]),
        "every round's untrusted wrap in a run must carry the run's nonce: {nonces:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn trusted_tool_result_is_appended_verbatim_in_the_loop() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, echo_tools());
    let out = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the loop converges");
    assert_eq!(out, "final answer");

    let content = last_tool_turn_content(&gateway.requests());
    assert_eq!(
        content, "echoed: hi",
        "a trusted tool's result must be appended verbatim, got: {content}"
    );
    assert!(
        !content.contains("untrusted_input_"),
        "a trusted tool's result must carry no guard tags, got: {content}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_in_flight_tool_call_returns_promptly() {
    use std::time::{Duration, Instant};

    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let prompt = parse(&loop_prompt(LOOP_TO_TEXT));
    let ctx = loop_context(&prompt, always_tool("echo", Arc::new(SlowTool)));

    let mut driver = TokioDriver::new(&ctx, Some(gateway_client(gateway.addr())));
    let canceller = driver.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        canceller.cancel();
    });

    let start = Instant::now();
    let result = driver.drive().await;

    assert!(
        start.elapsed() < Duration::from_secs(5),
        "cancel during an in-flight tool call must return promptly, took {:?}",
        start.elapsed()
    );
    assert!(
        matches!(result, Err(crate::Error::Interrupted)),
        "expected Interrupted, got {result:?}"
    );
}
