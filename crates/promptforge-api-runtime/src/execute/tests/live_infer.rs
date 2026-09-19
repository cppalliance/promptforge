use super::super::*;
use super::*;

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_infer_runs_once() {
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
    let addr = gateway.addr();

    let source = "---\nname: live-h1\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        var.answer = models.infer(writer, 'answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env.run(&prompt, "", to_context(gatewayed(addr))).await else {
        panic!("live H1 path must run");
    };

    assert_eq!(out, "h1 answer");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_environment_client_serves_a_run_when_the_context_carries_none() {
    // `Environment::run` defaults a client-less context to the environment's
    // client: the run's own client overrides it, and with none on the
    // context the environment's client must serve the run's completions.
    let gateway = ScriptedGateway::start(vec![resp_text("env answer")]).await;
    let addr = gateway.addr();

    let source = "---\nname: env-client\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Env Client\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        var.answer = models.infer(writer, 'answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(source);
    let env = Environment::new().client(gateway_client(addr));
    // The context deliberately carries no client: the defaulting in
    // `Environment::run` is the only path to the gateway.
    let RunResult::Ok(out) = env.run(&prompt, "", to_context(silent())).await else {
        panic!("the environment's client must serve a client-less context");
    };

    assert_eq!(out, "env answer");
    assert_eq!(
        gateway.call_count(),
        1,
        "the completion must have gone to the environment's client"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn unread_h1_prose_stays_inert_and_explicit_infer_requires_a_model() {
    // H1 prose no longer drives inference: an unread buffer - even one
    // whose substitution would fail or stay empty - discards at the pass's
    // end without requiring a model. Only an explicit `models.infer` of the
    // prose requires a binding.
    let unread = "---\nname: empty-h1\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Empty H1\n\n\
        ```lua\nvar.omit = ''\n```\n\n\
        {{ var.omit }}\n\n\
        ## Result\n\n\
        ```lua\nreturn 'ok'\n```\n";
    let out = super::run(&fixture(unread), "", &[], &TestStore::new(), silent())
        .await
        .expect("unread H1 prose must not require a model");
    assert_eq!(out, "ok");

    let reading = "---\nname: read-h1\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Read H1\n\n\
        ask\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let error = super::run(&fixture(reading), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("an explicit infer of H1 prose with no binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_function_resolves_host_globals_when_called() {
    let source = "---\nname: shared-host\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Shared Host\n\n\
        ```lua shared\n\
        function read_args() return args end\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn read_args()\n```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env
        .run(&prompt, "later host value", to_context(silent()))
        .await
    else {
        panic!("shared function must resolve host globals when called");
    };

    assert_eq!(out, "later host value");
}

#[tokio::test(flavor = "multi_thread")]
async fn shared_library_calls_host_apis_at_load_time() {
    // The shared library replays as each section's first chunk with the full
    // host environment installed, so top-level shared code may use `store`,
    // `log`, and `args` at load.
    let source = "---\nname: shared-host-load\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Shared Host Load\n\n\
        ```lua shared\n\
        store.write('loaded.txt', args)\n\
        log('shared loaded')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn store.read('loaded.txt')\n```\n";
    let prompt = parse(source);
    let env = Environment::new();
    // The multi-step path: prepare builds the run's own router, and the
    // test store wraps the prepared handle so the post-run assertion
    // reads what the run actually wrote.
    let (ctx, requirements) = env.prepare(&prompt, to_context(silent()));
    assert!(
        requirements.is_satisfied(),
        "the fixture declares nothing: {requirements:?}"
    );
    let store = TestStore::from_vfs(ctx.vfs_handle().clone());
    let RunResult::Ok(out) = crate::execute::run(&prompt, "load-time args", ctx).await else {
        panic!("top-level shared host calls must succeed");
    };

    assert_eq!(out, "load-time args");
    assert_eq!(
        store
            .read("loaded.txt")
            .expect("the load-time write persists"),
        "load-time args"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn captured_bindings_reach_section_call_and_fanout_vms() {
    let echo = Arc::new(EchoTool);
    // The bound slots arrive from the frontmatter: the capability installs
    // the tool, the exact slot binds the alias, and the captured alias
    // globals install in every section VM - H1 never runs a bind.
    let source = "---\nname: captured-bindings\ndescription: d\npromptforge: 0\ncapabilities:\n  - tests/tools\ntools:\n  echo: tests/tools/echo\nmodels:\n  writer: {}\n---\n\n\
         # Captured Bindings\n\n\
         ```lua shared\n\
         function binding_names() return echo.name .. ':' .. writer.name end\n\
         ```\n\n\
         ## Parent\n\n\
         ```lua\n\
         local direct = binding_names()\n\
         local called = call('## Called')\n\
         local arms = fanout('### Worker', list_from_section('### Items'))\n\
         return direct .. '|' .. called .. '|' .. table.concat(arms, ',')\n\
         ```\n\n\
         ### Worker\n\n\
         ```lua\nreturn binding_names() .. ':' .. item\n```\n\n\
         ### Items\n\n\
         - one\n\
         - two\n\n\
         ## Called\n\n\
         ```lua\nreturn binding_names()\n```\n";
    let prompt = parse(source);
    let tools: [Arc<dyn Tool>; 1] = [echo];
    let env = Environment::new().registry(tools_registry(&tools));
    let RunResult::Ok(out) = env.run(&prompt, "", to_context(silent())).await else {
        panic!("captured bindings must be installed in every section VM");
    };

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
    let source = "---\nname: live-h1-models-infer\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1 Models Infer\n\n\
        ```lua\n\
        models.default('writer')\n\
        var.answer = models.infer('answer once')\n\
        var.sys_untouched = not pcall(function() return sys.reply_finish_reason end)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer .. ':' .. tostring(var.sys_untouched)\n```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env
        .run(&prompt, "", to_context(gatewayed(gateway.addr())))
        .await
    else {
        panic!("live H1 models.infer must run");
    };

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
    let source = "---\nname: nested-infer-observations\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Nested Infer Observations\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        var.answer = models.infer(writer, 'ping')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(source);
    let recorder = Arc::new(Recorder::default());
    let env = Environment::new();

    let RunResult::Ok(out) = env
        .run(
            &prompt,
            "",
            to_context(RunOptions {
                execution: EXECUTION,
                observer: Arc::clone(&recorder) as Arc<dyn Observer>,
                client: Some(gateway_client(addr)),
                debug: None,
            }),
        )
        .await
    else {
        panic!("nested infer must run");
    };

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
async fn cancelled_nested_infer_does_not_report_model_turn_failed() {
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let source = "---\nname: cancelled-infer\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Cancelled Infer\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        return models.infer(writer, 'must cancel')\n\
        ```\n";
    let prompt = parse(source);
    let recorder = Arc::new(Recorder::default());
    let cancel = crate::cancel::CancelHandle::new();
    let canceller = cancel.clone();
    let gateway_calls = Arc::clone(&gateway.calls);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while gateway_calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });
    let env = Environment::new();
    let result = env
        .run(
            &prompt,
            "",
            test_context(EXECUTION)
                .observer(Arc::clone(&recorder) as Arc<dyn Observer>)
                .model(test_model_catalog().models()[0].clone())
                .client(gateway_client(gateway.addr()))
                .cancel(cancel),
        )
        .await;
    assert!(
        matches!(result, RunResult::Cancelled),
        "cancelling an in-flight infer must interrupt the run: {result:?}"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the cancellation must occur after infer reached the gateway"
    );
    assert!(
        recorder
            .events()
            .iter()
            .all(|(_, event)| event != &detail::MODEL_TURN_FAILED.to_string()),
        "cancellation must not report a model failure: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn handle_infer_tool_call_violation_uses_entry_point_neutral_wording() {
    let gateway = ScriptedGateway::start(vec![resp_tool_call("call_1", "ghost", "{}")]).await;
    let source = "---\nname: infer-tool-call\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Infer Tool Call\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        return models.infer(writer, 'answer without tools')\n\
        ```\n";
    let error = super::run(
        &bound_for_model(source),
        "",
        &[],
        &TestStore::new(),
        gatewayed(gateway.addr()),
    )
    .await
    .expect_err("a tool-call result from direct infer must be rejected");
    let rendered = error.to_string();
    assert!(
        rendered.contains("model inference received tool calls but no tools were advertised"),
        "the violation must use neutral wording: {rendered}"
    );
    assert!(
        !rendered.contains("models.infer received"),
        "handle-form infer must not be misreported as models.infer: {rendered}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_prose_infers_explicitly_and_var_accumulates_into_the_walk() {
    // The live H1 pass reads its pending buffer only through an explicit
    // infer, and `var` writes accumulate across the pass into the walk.
    let gateway = ScriptedGateway::start(vec![resp_text("final answer")]).await;
    let addr = gateway.addr();
    let source = "---\nname: live-h1-prose\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1 Prose\n\n\
        ```lua\n\
        models.default('writer')\n\
        var.executions = (var.executions or 0) + 1\n\
        ```\n\n\
        Ask for one round.\n\n\
        ```lua\n\
        var.first = models.infer(prose)\n\
        var.executions = var.executions + 1\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\n\
        return var.first .. ':' .. var.executions\n\
        ```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env.run(&prompt, "", to_context(gatewayed(addr))).await else {
        panic!("live H1 prose infers explicitly");
    };

    assert_eq!(out, "final answer:2");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn h1_and_h2_prose_each_infer_explicitly_in_source_order() {
    // The live H1 pass and the H2 section each read their own pending
    // buffer into an explicit infer: two completions, in source order.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply"), resp_text("h2 reply")]).await;
    let source = "---\nname: shared-loop\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Shared Loop\n\n\
        ```lua\n\
        models.default('writer')\n\
        ```\n\n\
        h1 prose turn\n\n\
        ```lua\n\
        var.h1 = models.infer(prose)\n\
        ```\n\n\
        ## Section Two\n\n\
        h2 prose turn\n\n\
        ```lua\n\
        return models.infer(prose)\n\
        ```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env
        .run(&prompt, "", to_context(gatewayed(gateway.addr())))
        .await
    else {
        panic!("H1 prose and H2 prose each infer explicitly");
    };

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

#[tokio::test(flavor = "multi_thread")]
async fn live_h1_chunk_takes_root_entry_zero_and_the_first_walked_section_takes_root_entry_one() {
    // The H1 pass is the root chain's entry 0, so the first walked section
    // takes entry 1 of the same chain.
    let source = "---\nname: live-h1-sys-id\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1 Sys Id\n\n\
        ```lua\n\
        assert(sys.id == '0.0', 'the live H1 chunk takes the root chain entry 0')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\n\
        assert(sys.id == '0.1', 'the first walked section takes entry 1')\n\
        return 'ok'\n\
        ```\n";
    let prompt = parse(source);
    let env = Environment::new();
    let RunResult::Ok(out) = env.run(&prompt, "", to_context(silent())).await else {
        panic!("the H1 chunk takes root entry 0 and the first walked section root entry 1");
    };

    assert_eq!(out, "ok");
}
