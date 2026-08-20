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
        local writer = models.default('writer', 'A general model for tests')\n\
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
async fn shared_function_resolves_host_globals_when_called() {
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
async fn shared_library_calls_host_apis_at_load_time() {
    // The shared library replays as each section's first chunk with the full
    // host environment installed, so top-level shared code may use `store`,
    // `log`, and `args` at load.
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let models = test_model_catalog();
    let store = StoreRef::memory();
    let source = "---\nname: shared-host-load\ndescription: d\npromptforge: 1\n---\n\n\
        # Shared Host Load\n\n\
        ```lua shared\n\
        store.write('loaded.txt', args)\n\
        log('shared loaded')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn store.read('loaded.txt')\n```\n";
    let prompt = parse(source);
    let out = super::super::run(
        &prompt,
        "load-time args",
        ResolutionContext::new(&picker, &models),
        &[],
        &store,
        to_config(silent()),
    )
    .await
    .expect("top-level shared host calls must succeed");

    assert_eq!(out, "load-time args");
    assert_eq!(
        store
            .read("loaded.txt")
            .expect("the load-time write persists"),
        "load-time args"
    );
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
         local arms = fanout('### Worker', list_from_section('### Items'))\n\
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
async fn live_h1_models_infer_resolves_the_default_model_without_touching_sys() {
    // The live H1 `models.infer` resolves the current model from the
    // producer's bindings-so-far and runs the one infer shape: a single
    // tool-free round on a fresh conversation that leaves `sys` untouched.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
    let source = "---\nname: live-h1-models-infer\ndescription: d\npromptforge: 1\n---\n\n\
        # Live H1 Models Infer\n\n\
        ```lua\n\
        models.default('writer', 'A general model for tests')\n\
        var.answer = models.infer('answer once')\n\
        var.sys_untouched = not pcall(function() return sys.reply_finish_reason end)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer .. ':' .. tostring(var.sys_untouched)\n```\n";
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
        to_config(gatewayed(gateway.addr())),
    )
    .await
    .expect("live H1 models.infer must run");

    assert_eq!(out, "h1 answer:true");
    assert_eq!(gateway.call_count(), 1);
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(
        body["model"], "claude-sonnet-4-6",
        "models.infer must use the section's current model"
    );
    assert!(
        body.get("tools").is_none(),
        "models.infer advertises no tools: {body}"
    );
    assert_eq!(
        body["messages"].as_array().expect("messages array").len(),
        1,
        "models.infer runs on a fresh context: {body}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn nested_lua_infer_emits_a_model_turn_observation() {
    // observe.rs F1: a nested Lua infer must surface its model-turn
    // observation to the run's observer, proving owned-observer propagation
    // reaches the nested inference path.
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let source = "---\nname: nested-infer-observations\ndescription: d\npromptforge: 1\n---\n\n\
        # Nested Infer Observations\n\n\
        ```lua\n\
        local writer = models.default('writer', 'A general model for tests')\n\
        var.answer = writer:infer('ping')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(source);
    let picker = ToolPicker::build(Catalog::default(), PickerConfig::default())
        .expect("empty tool picker must build");
    let models = test_model_catalog();
    let recorder = Arc::new(Recorder::default());

    let out = super::super::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models),
        &[],
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
    .expect("nested infer must run");

    assert_eq!(out, "pong");
    let details: Vec<String> = recorder
        .records()
        .into_iter()
        .map(|(_, _, detail)| detail)
        .collect();
    let model_turns = details
        .iter()
        .filter(|d| d.as_str() == "Model turn completed")
        .count();
    assert_eq!(
        model_turns, 1,
        "the nested infer drives exactly one model round trip: {details:?}"
    );
    assert!(
        details.iter().all(|d| d.as_str() != "Tool call succeeded"),
        "infer advertises no tools, so no tool call can be observed: {details:?}"
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
         models.default('writer', 'A general model for tests')\n\
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

#[tokio::test(flavor = "multi_thread")]
async fn h1_and_h2_prose_both_run_through_the_shared_block_loop() {
    // One block loop serves both drivers: the live H1 prose and the H2
    // section prose each reach the gateway exactly once, in source order.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply"), resp_text("h2 reply")]).await;
    let source = "---\nname: shared-loop\ndescription: d\npromptforge: 1\n---\n\n\
        # Shared Loop\n\n\
        ```lua\n\
        models.default('writer', 'A general model for tests')\n\
        ```\n\n\
        h1 prose turn\n\n\
        ## Section Two\n\n\
        h2 prose turn\n\n\
        ```lua\n\
        return reply\n\
        ```\n";
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
        to_config(gatewayed(gateway.addr())),
    )
    .await
    .expect("H1 prose and H2 prose both run through the shared block loop");

    assert_eq!(out, "h2 reply");
    assert_eq!(
        gateway.call_count(),
        2,
        "the H1 prose and the H2 prose each drive exactly one completion"
    );
    let requests = gateway.requests();
    let first_prose = requests[0]["messages"][0]["content"]
        .as_str()
        .expect("the first request carries a user message");
    let second_prose = requests[1]["messages"][0]["content"]
        .as_str()
        .expect("the second request carries a user message");
    assert!(
        first_prose.contains("h1 prose turn"),
        "the first completion is the H1 prose: {first_prose}"
    );
    assert!(
        second_prose.contains("h2 prose turn"),
        "the second completion is the H2 prose: {second_prose}"
    );
}
