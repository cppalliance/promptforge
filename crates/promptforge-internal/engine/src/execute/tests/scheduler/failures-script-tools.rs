//! Script-initiated `tools.call` dispatch on the scheduler: string and
//! table results, tool objects, unbound aliases, bound tools outside the
//! section scope, cancellation of a slow call, untrusted output wrapping,
//! and the model install around a call.

use super::*;

/// Arms the run's shared tool set with `bindings`, every alias in the
/// prompt-wide `always` scope, so a section's effective scope includes them
/// without an H1 pass; the implementations go to the test driver's tool table.
fn arm_tool_set(
    ctx: &RunState,
    fixture: RunFixture,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
) -> RunFixture {
    arm_tools(ctx, fixture, bindings)
}

/// Arms the run's shared tool set with `bindings` and exactly `always` as
/// the prompt-wide scope, so a binding can sit in the document catalog
/// without entering any section's effective scope.
fn arm_tool_set_scoped(
    ctx: &RunState,
    fixture: RunFixture,
    bindings: Vec<(crate::lua::ToolBinding, Arc<dyn TestTool>)>,
    always: Vec<String>,
) -> RunFixture {
    arm_tools_scoped(ctx, fixture, bindings, always)
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_dispatches_and_resumes_as_a_string() {
    // The whole script path in one pass: the shim yields, the scheduler
    // dispatches the bound tool, the plain binding resumes as a Lua
    // string, and the counts land in the same `tools.calls` table the
    // prose loop feeds.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local out = tools.call('echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls.echo)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the script dispatch succeeds");
    assert_eq!(out, "echoed: hi|1");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_with_a_tool_object_dispatches_its_binding() {
    // The handle form: the captured alias global is an inspectable Tool
    // object, and passing it as the leading argument dispatches the binding
    // it names, identically to the bare alias string.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        assert(type(echo) == 'userdata', 'the captured alias is a Tool object')\n\
        return tools.call(echo, { value = 'hi' })\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the handle-form dispatch succeeds");
    assert_eq!(out, "echoed: hi");
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_with_an_unbound_alias_names_the_bound_set() {
    // Script-initiated resolution runs against the run's full bound
    // catalog, so the unknown-alias error names that whole set, not the
    // section's effective scope.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('missing', {})\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("an unbound alias fails the block");
    match &error {
        Error::UnboundToolCall { name, bound } => {
            assert_eq!(name, "missing");
            assert_eq!(bound, &["echo".to_owned()]);
        }
        other => panic!("expected the typed unbound-tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_reaches_a_bound_tool_outside_the_section_scope() {
    // A tool bound in the document catalog but never scoped into the
    // section (no `always`, no `tools.add`) still dispatches for a script:
    // the scope shapes what the model is offered, and the author's own
    // code is not the model. The count lands in the same shared map
    // `tools.calls` reads.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local out = tools.call('echo', { value = 'hi' })\n\
        return out .. '|' .. tostring(tools.calls.echo)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set_scoped(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
        Vec::new(),
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a bound but unscoped alias dispatches for a script");
    assert_eq!(out, "echoed: hi|1");
}

/// A tool that signals its start and then never completes, so the
/// cancellation test fires only once the dispatch is in flight.
struct SignallingSlowTool {
    started: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TestTool for SignallingSlowTool {
    fn id(&self) -> ToolId {
        ToolId::parse("tools/slow").expect("valid id")
    }

    #[expect(
        clippy::unnecessary_literal_bound,
        reason = "the TestTool trait fixes this return type to &str, so the &'static str suggestion cannot be applied"
    )]
    fn description(&self) -> &str {
        "a deliberately slow tool"
    }

    fn parameters_schema(&self) -> serde_json::Value {
        json!({ "type": "object", "properties": {} })
    }

    async fn call(
        &self,
        _args: serde_json::Value,
    ) -> std::result::Result<crate::tools::ToolOutput, crate::tools::ToolError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        std::future::pending().await
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn cancellation_interrupts_a_slow_script_tools_call() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('slow', {})\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let started = Arc::new(AtomicUsize::new(0));
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding(
            "slow",
            "slow tool",
            Arc::new(SignallingSlowTool {
                started: Arc::clone(&started),
            }),
        )],
    );
    let mut driver = TokioDriver::new(&ctx, fixture, None);
    let canceller = driver.cancel_handle();
    let observed = Arc::clone(&started);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while observed.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let start = std::time::Instant::now();
    let result = driver.drive().await;

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling a suspended tools.call must interrupt the run, got {result:?}"
    );
    assert_eq!(
        started.load(Ordering::SeqCst),
        1,
        "the cancellation must land after the tool call was in flight"
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "the slow tool must not hold the run, took {:?}",
        start.elapsed()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_untrusted_script_tools_call_result_is_nonce_wrapped() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('fetch', { value = 'hi' })\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding(
            "fetch",
            "untrusted echo tool",
            Arc::new(UntrustedEchoTool),
        )],
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the untrusted dispatch succeeds");
    assert!(
        out.contains("<untrusted_input_") && out.contains("</untrusted_input_"),
        "the script must receive the nonce-wrapped envelope, got: {out}"
    );
    assert!(
        out.contains("echoed: hi"),
        "the wrapped block must still include the tool output, got: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_structured_binding_resumes_as_a_lua_table() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\n\
        local r = tools.call('form', {})\n\
        return r.text .. '|' .. tostring(#r.images)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "{\"text\":\"typed\",\"images\":[]}",
            trusted: true,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let fixture = arm_tool_set(&ctx, fixture, vec![binding]);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the structured dispatch succeeds");
    assert_eq!(out, "typed|0");
}

#[tokio::test(flavor = "current_thread")]
async fn invalid_json_from_a_structured_tool_is_a_tool_error() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('form', {})\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "not json",
            trusted: true,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let fixture = arm_tool_set(&ctx, fixture, vec![binding]);
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("invalid structured output fails the call");
    match &error {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the tool error names the invalid JSON, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn an_untrusted_structured_output_is_wrapped_before_classification() {
    // The untrusted nonce wrap precedes the structured JSON parse, so an
    // untrusted binding's valid JSON still fails the call: this ordering is
    // what restricts structured output to trusted tools. If classification
    // ever ran on the raw output, this test would resume a table and fail.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn tools.call('form', {})\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let mut binding = fixture_binding(
        "form",
        "structured fixture",
        Arc::new(StructuredFixtureTool {
            body: "{\"text\":\"typed\"}",
            trusted: false,
        }),
    );
    binding.0.output_kind = promptforge_lua::ToolOutputKind::Structured;
    let fixture = arm_tool_set(&ctx, fixture, vec![binding]);
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("untrusted structured output fails the call");
    match &error {
        Error::Tool { message, .. } => {
            assert!(
                message.contains("returned invalid JSON"),
                "the wrap must precede the parse, got: {message}"
            );
        }
        other => panic!("expected the typed tool error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_script_tools_call_before_infer_keeps_the_model_install() {
    // The one-time section scope install is shared between the first script
    // dispatch and the model resolution: a script `tools.call` that runs
    // first must not swallow the install a later `models.infer` relies on.
    let gateway = ScriptedChat::new(vec![resp_text("prose answer")]);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\ntools.call('echo', { value = 'x' })\n```\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("infer after a script dispatch still resolves the model");
    assert_eq!(out, "prose answer");
}

#[tokio::test(flavor = "current_thread")]
async fn a_document_prompt_without_tools_call_is_unaffected() {
    // Bindings installed, shim present, `tools.call` never called: the
    // section runs exactly as before the dispatch arm existed.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ToolCall\n\n\
        ## Only\n\n\
        ```lua\nreturn 'plain'\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let fixture = arm_tool_set(
        &ctx,
        fixture,
        vec![fixture_binding("echo", "echo tool", Arc::new(EchoTool))],
    );
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a prompt that never calls tools.call is unchanged");
    assert_eq!(out, "plain");
}
