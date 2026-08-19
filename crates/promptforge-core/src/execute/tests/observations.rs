use super::super::*;
use super::run;
use super::*;

#[tokio::test]
async fn a_two_section_run_reports_the_exact_observation_sequence() {
    let (result, records) = run_recorded(TWO_SECTIONS).await;
    assert_eq!(result.unwrap(), "second");

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "First".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "First".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("First".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string()
            ),
            (
                "First".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("First".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Second".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Second".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
            ("Second".to_string(), detail::LUA_CHUNK_STARTED.to_string(),),
            (
                "Second".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Second".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Second".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn recording_and_null_observers_produce_the_same_result_and_store_state() {
    let prompt = fixture(STORE_SECTIONS);
    let recorded_store = StoreRef::memory();
    let sink = Arc::new(Recorder::default());
    let observed_result = run(
        &prompt,
        "",
        &[],
        &recorded_store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&sink) as Arc<dyn Observer>,
            client: None,
            debug: None,
        },
    )
    .await;
    let null_store = StoreRef::memory();
    let null_result = run(&prompt, "", &[], &null_store, silent()).await;

    assert_eq!(observed_result.unwrap(), null_result.unwrap());
    assert_eq!(
        recorded_store.glob("**").unwrap(),
        null_store.glob("**").unwrap(),
        "observer choice must not change store side effects"
    );
    assert_eq!(
        recorded_store.read("state.txt").unwrap(),
        null_store.read("state.txt").unwrap(),
        "observer choice must not change stored contents"
    );

    let failing = fixture(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
         ## Only\n\n```lua\nerror('expected failure')\n```\n",
    );
    let sink = Arc::new(Recorder::default());
    let observed_error = run(
        &failing,
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&sink) as Arc<dyn Observer>,
            client: None,
            debug: None,
        },
    )
    .await
    .expect_err("the prologue fails");
    let null_error = run(&failing, "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("the prologue fails");
    assert_eq!(
        observed_error.to_string(),
        null_error.to_string(),
        "observer choice must not change errors"
    );
}

#[tokio::test]
async fn a_run_refused_by_the_version_gate_reports_nothing() {
    // The gate is not a run that failed; it is a run that never started, so
    // there is no RunStarted to pair a RunFinished with.
    let md = "---\nname: t\ndescription: d\npromptforge: 2\n---\n\n\
## Only\n\n```lua\nreturn \"ran\"\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(result.is_err());
    assert!(
        records.is_empty(),
        "the gate must report nothing: {records:?}"
    );
}

#[tokio::test]
async fn a_failing_run_still_reports_run_finished() {
    // The prologue fails, so the walk tears down its VM and the final
    // observation must report the run failure.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nerror('expected failure')\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(matches!(
        result,
        Err(Error::Lua(_) | Error::LuaRuntime { .. })
    ));

    assert_eq!(
        events(&records),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Only".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_string(), detail::LUA_CHUNK_FAILED.to_string()),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Test prompt".to_string(), detail::RUN_FAILED.to_string()),
        ],
        "a section that errors reports no SectionFinished"
    );
}

#[tokio::test]
async fn an_erroring_section_reports_started_but_not_finished() {
    // The absence half of the section-boundary contract: a section that
    // errors mid-walk must emit SECTION_STARTED and never SECTION_FINISHED.
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
## Only\n\n```lua\nerror('expected failure')\n```\n";
    let (result, records) = run_recorded(md).await;
    assert!(result.is_err());

    let observed = events(&records);
    assert!(
        observed.contains(&("Only".to_string(), detail::SECTION_STARTED.to_string())),
        "the erroring section must report started: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(_, event)| event == &detail::SECTION_FINISHED.to_string()),
        "the erroring section must never report finished: {observed:?}"
    );
}

#[tokio::test]
async fn one_execution_id_spans_parse_and_the_complete_runtime_lifecycle() {
    let gateway = ScriptedGateway::start(aliased_tool_script("echo")).await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "echo",
        "canonical_echo",
        "Echo a test value.",
    ));
    let descriptor = ToolDescriptor::new(
        PickerToolId::new("tests", "echo"),
        tool.description(),
        tool.parameters_schema(),
    );
    let capability =
        serde_json::to_string(&capability_for(&descriptor)).expect("serialize fixture capability");
    let source = format!(
        "---\nname: lifecycle\ndescription: Correlated lifecycle fixture\npromptforge: 1\n---\n\n\
         # Lifecycle\n\n```lua\n\
         tools.need('echo', {capability})\n\
         tools.always('echo')\n\
         models.default('writer', 'A general model for tests')\n```\n\n\
         ## Gather\n\n```lua\nstore.write('state.txt', 'before')\n```\n\n\
         Use the echo tool.\n\n\
         ```lua\nstore.append('state.txt', '\\nafter')\nreturn reply\n```\n"
    );
    let recorder = Arc::new(Recorder::default());
    let prompt = Prompt::parse(&source, EXECUTION, recorder.as_ref())
        .expect("the lifecycle fixture must parse");
    let _picker = ToolPicker::build(
        Catalog::new(vec![descriptor.clone()]),
        PickerConfig::default(),
    )
    .expect("the lifecycle picker must build");
    let tools: [Arc<dyn Tool>; 1] = [Arc::clone(&tool) as Arc<dyn Tool>];
    let prompt = TestPrompt {
        prompt,
        models: test_model_catalog(),
        picker_catalog: Some(Catalog::new(vec![descriptor])),
    };
    let store = StoreRef::memory();

    let result = run(
        &prompt,
        "",
        &tools,
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test").expect("non-empty test key"),
            )),
            debug: None,
        },
    )
    .await
    .expect("the lifecycle fixture must run");

    assert_eq!(result, "aliased final");
    assert_eq!(store.read("state.txt").unwrap(), "before\nafter");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let records = recorder.records();
    assert!(!records.is_empty());
    assert!(
        records
            .iter()
            .all(|(execution, _, _)| execution == EXECUTION),
        "every lifecycle record must retain {EXECUTION}: {records:#?}"
    );
    let details = records
        .iter()
        .map(|(_, _, detail)| detail.clone())
        .collect::<Vec<_>>();
    for expected in [
        detail::PARSE_STARTED,
        detail::RUN_STARTED,
        detail::SECTION_STARTED,
        detail::LUA_CHUNK_STARTED,
        detail::STORE_WRITE_SUCCEEDED,
        detail::MODEL_TURN_COMPLETED,
        detail::TOOL_CALL_SUCCEEDED,
        detail::LUA_CHUNK_STARTED,
        detail::STORE_APPEND_SUCCEEDED,
        detail::RUN_SUCCEEDED,
    ] {
        assert!(
            details.contains(&expected.to_string()),
            "the complete lifecycle must include {expected:?}: {records:#?}"
        );
    }
}

#[tokio::test]
async fn the_tool_loop_reports_each_turn_and_each_tool_call() {
    let gateway = ScriptedGateway::start(echo_then_text_script()).await;
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

    let recorder = Arc::new(Recorder::default());
    let turns = AtomicU32::new(0);
    let options = test_completion_options();
    let (out, _) = run_tool_loop(
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
    .unwrap();
    assert_eq!(out, "final answer");

    assert_eq!(
        recorder.events(),
        vec![
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::TOOL_CALL_SUCCEEDED.to_string(),
            ),
            (
                "Gather".to_string(),
                detail::MODEL_TURN_COMPLETED.to_string(),
            ),
        ]
    );
}
