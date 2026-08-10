use super::super::*;
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_infer_runs_once() {
    async fn completions(
        State(calls): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        calls.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "h1 answer" }
            }]
        }))
    }

    let calls = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&calls));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let source = "---\nname: live-h1\ndescription: d\npromptforge: 1\n---\n\n\
        # Live H1\n\n\
        ```lua\n\
        local writer = models.always('writer', 'A general model for tests')\n\
        var.answer = writer:infer('answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(source);
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let models = test_model_catalog();
    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &[],
        &StoreRef::memory(),
        to_config(RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        }),
    )
    .await
    .expect("live H1 path must run");

    assert_eq!(out, "h1 answer");
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_library_loads_before_host_and_resolves_host_when_called() {
    let source = "---\nname: shared-host\ndescription: d\npromptforge: 1\n---\n\n\
        # Shared Host\n\n\
        ```lua shared\n\
        function read_args() return args end\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn read_args()\n```\n";
    let prompt = parse(source);
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let models = test_model_catalog();
    let out = super::super::run(
        &prompt,
        "later host value",
        ResolutionContext::new(&picker, &models),
        &[],
        &StoreRef::memory(),
        to_config(silent()),
    )
    .await
    .expect("shared function must resolve host globals when called");

    assert_eq!(out, "later host value");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_library_cannot_call_host_at_load_time() {
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let models = test_model_catalog();
    for (host, call) in [
        ("store", "store.write('forbidden.txt', 'not written')"),
        ("log", "log('forbidden')"),
    ] {
        let source = format!(
            "---\nname: shared-host-error\ndescription: d\npromptforge: 1\n---\n\n\
             # Shared Host Error\n\n\
             ```lua shared\n\
             {call}\n\
             ```\n\n\
             ## Result\n\n\
             ```lua\nreturn 'unreachable'\n```\n"
        );
        let prompt = parse(&source);
        let error = super::super::run(
            &prompt,
            "",
            ResolutionContext::new(&picker, &models),
            &[],
            &StoreRef::memory(),
            to_config(silent()),
        )
        .await
        .expect_err("top-level shared host call must fail");

        assert!(
            error.to_string().contains(host),
            "failure must identify unavailable host global {host}: {error}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_bindings_reach_section_execute_and_fanout_vms() {
    let echo = Arc::new(EchoTool);
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        echo.description(),
        echo.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize tool capability");
    let source = format!(
        "---\nname: captured-bindings\ndescription: d\npromptforge: 1\n---\n\n\
         # Captured Bindings\n\n\
         ```lua\n\
         echo = tools.need('echo', {capability})\n\
         writer = models.need('writer', 'A general model for tests')\n\
         ```\n\n\
         ```lua shared\n\
         function binding_names() return echo.name .. ':' .. writer.name end\n\
         ```\n\n\
         ## Parent\n\n\
         ```lua\n\
         local direct = binding_names()\n\
         local called = execute('## Called')\n\
         local arms = fanout('### Worker', '### Items')\n\
         return direct .. '|' .. called .. '|' .. table.concat(arms, ',')\n\
         ```\n\n\
         ### Worker\n\n\
         ```lua\nreturn binding_names() .. ':' .. item\n```\n\n\
         ### Items\n\n\
         - one\n\
         - two\n\n\
         ## Called\n\n\
         ```lua\nreturn binding_names()\n```\n"
    );
    let prompt = parse(&source);
    let picker = ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
        .expect("tool picker must build");
    let models = test_model_catalog();
    let tools: [Arc<dyn Tool>; 1] = [echo];
    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &tools,
        &StoreRef::memory(),
        to_config(silent()),
    )
    .await
    .expect("captured bindings must be installed in every section VM");

    assert_eq!(
        out,
        "echo:writer|echo:writer|echo:writer:one,echo:writer:two"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_infer_sees_tools_resolved_in_the_same_block() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let echo = Arc::new(EchoTool);
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        echo.description(),
        echo.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize tool capability");
    let source = format!(
        "---\nname: live-h1-tools\ndescription: d\npromptforge: 1\n---\n\n\
         # Live H1 Tools\n\n\
         ```lua\n\
         local echo = tools.need('echo', {capability})\n\
         tools.always(echo.name)\n\
         local writer = models.always('writer', 'A general model for tests')\n\
         var.answer = writer:infer('use echo')\n\
         ```\n\n\
         ## Result\n\n\
         ```lua\nreturn var.answer\n```\n"
    );
    let prompt = parse(&source);
    let picker = ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
        .expect("tool picker must build");
    let models = test_model_catalog();
    let tools: [Arc<dyn Tool>; 1] = [echo];

    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &tools,
        &StoreRef::memory(),
        to_config(RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        }),
    )
    .await
    .expect("live H1 infer must use its resolved always tool");

    assert_eq!(out, "final answer");
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_lua_infer_emits_model_and_tool_observations() {
    // observe.rs F1: a nested Lua `model:infer` that drives a tool round trip
    // must surface BOTH model-turn and tool-call observations to the run's
    // observer, proving owned-observer propagation reaches the inner tool loop.
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let echo = Arc::new(EchoTool);
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        echo.description(),
        echo.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize tool capability");
    let source = format!(
        "---\nname: nested-infer-observations\ndescription: d\npromptforge: 1\n---\n\n\
         # Nested Infer Observations\n\n\
         ```lua\n\
         local echo = tools.need('echo', {capability})\n\
         tools.always(echo.name)\n\
         local writer = models.always('writer', 'A general model for tests')\n\
         var.answer = writer:infer('use echo')\n\
         ```\n\n\
         ## Result\n\n\
         ```lua\nreturn var.answer\n```\n"
    );
    let prompt = parse(&source);
    let picker = ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
        .expect("tool picker must build");
    let models = test_model_catalog();
    let tools: [Arc<dyn Tool>; 1] = [echo];
    let recorder = Arc::new(Recorder::default());

    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &tools,
        &StoreRef::memory(),
        to_config(RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        }),
    )
    .await
    .expect("nested infer must run its tool loop");

    assert_eq!(out, "final answer");
    let details: Vec<String> = recorder
        .records()
        .into_iter()
        .map(|(_, _, detail)| detail)
        .collect();
    let model_turns = details
        .iter()
        .filter(|d| d.as_str() == "Model turn completed")
        .count();
    let tool_calls = details
        .iter()
        .filter(|d| d.as_str() == "Tool call succeeded")
        .count();
    assert!(
        model_turns >= 2,
        "nested infer drives two model round trips; saw {model_turns}: {details:?}"
    );
    assert_eq!(
        tool_calls, 1,
        "the single echo dispatch must emit exactly one tool-call observation: {details:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_prose_preserves_non_final_and_final_semantics_and_captures_var() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
    let addr = gateway.addr();
    let echo = Arc::new(EchoTool);
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        echo.description(),
        echo.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize tool capability");
    let source = format!(
        "---\nname: live-h1-prose\ndescription: d\npromptforge: 1\n---\n\n\
         # Live H1 Prose\n\n\
         ```lua\n\
         tools.need('echo', {capability})\n\
         tools.always('echo')\n\
         models.always('writer', 'A general model for tests')\n\
         var.executions = (var.executions or 0) + 1\n\
         ```\n\n\
         Ask for one tool call.\n\n\
         ```lua\n\
         var.non_final_had_text = reply ~= nil\n\
         var.executions = var.executions + 1\n\
         ```\n\n\
         Finish now.\n\n\
         ```lua\n\
         var.final_reply = reply\n\
         var.executions = var.executions + 1\n\
         ```\n\n\
         ## Result\n\n\
         ```lua\n\
         return tostring(var.non_final_had_text) .. ':' .. var.final_reply .. ':' .. var.executions\n\
         ```\n"
    );
    let prompt = parse(&source);
    let picker = ToolPicker::build(Catalog::new(vec![descriptor]), PickerConfig::default())
        .expect("tool picker must build");
    let models = test_model_catalog();
    let tools: [Arc<dyn Tool>; 1] = [echo];

    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &tools,
        &StoreRef::memory(),
        to_config(RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        }),
    )
    .await
    .expect("live H1 prose must preserve block semantics");

    assert_eq!(out, "false:final answer:3");
}
