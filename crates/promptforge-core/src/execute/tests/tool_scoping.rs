use super::super::*;
use super::run;
use super::*;

#[tokio::test]
async fn declared_tools_are_not_injected_without_always_or_add() {
    async fn completions(
        State(bodies): State<Arc<Mutex<Vec<Value>>>>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        bodies.lock().unwrap().push(body);
        Json(json!({
            "choices": [{
                "message": { "role": "assistant", "content": "plain reply" }
            }]
        }))
    }

    let bodies = Arc::new(Mutex::new(Vec::new()));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&bodies));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let tool = ScopedFixtureTool::new("concrete", "canonical_wire", "Concrete description.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\ntools.need('local_alias', 'capability')\nmodels.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\nAsk without tools.\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "plain reply");
    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].get("tools").is_none(),
        "declaring a need must not expose it without explicit scope"
    );
}

#[tokio::test]
async fn always_advertises_concrete_schema_under_local_alias_and_dispatches_by_id() {
    let (addr, bodies, _) = spawn_aliased_tool_gateway("local_alias").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.need('local_alias', 'capability')\n\
tools.always('local_alias')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\nUse the tool.\n",
        Vec::new(),
    );

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
                SecretString::new("test"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = bodies.lock().unwrap();
    let function = &bodies[0]["tools"][0]["function"];
    assert_eq!(function["name"], "local_alias");
    assert_eq!(function["description"], "Concrete description.");
    assert_eq!(
        function["parameters"],
        json!({
            "type": "object",
            "properties": {"value": {"type": "string"}},
            "required": ["value"]
        })
    );
    assert_ne!(function["name"], "canonical_wire");
}

#[tokio::test]
async fn h2_add_scopes_an_alias_and_dispatches_the_concrete_tool() {
    let (addr, bodies, _) = spawn_aliased_tool_gateway("section_tool").await;
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.need('section_tool', 'capability')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.add('section_tool')\n```\n\nUse the tool.\n",
        Vec::new(),
    );

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
                SecretString::new("test"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        bodies.lock().unwrap()[0]["tools"][0]["function"]["name"],
        "section_tool"
    );
}

#[tokio::test]
async fn near_duplicate_tools_are_valid_when_isolated_in_separate_sections() {
    async fn completions(
        State(requests): State<Arc<AtomicUsize>>,
        Json(_body): Json<Value>,
    ) -> Json<Value> {
        requests.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "choices": [{"message": {"role": "assistant", "content": "text"}}]
        }))
    }

    let requests = Arc::new(AtomicUsize::new(0));
    let router = Router::new()
        .route("/v1/chat/completions", post(completions))
        .with_state(Arc::clone(&requests));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    let first = ScopedFixtureTool::new("first", "first_wire", "First concrete.");
    let second = ScopedFixtureTool::new("second", "second_wire", "Second concrete.");
    let first_descriptor = picker_descriptor("first", "Similar operation one.");
    let second_descriptor = picker_descriptor("second", "Similar operation two.");
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.need('first_local', 'first')\n\
tools.need('second_local', 'second')\n\
models.always('writer', 'A general model for tests')\n```\n\n\
## First\n\n```lua\ntools.add('first_local')\n```\n\nFirst model turn.\n\n\
## Second\n\n```lua\ntools.add('second_local')\n```\n\nSecond model turn.\n",
        vec![(first_descriptor, second_descriptor)],
    );

    let out = run(
        &prompt,
        "",
        &[
            Arc::new(first) as Arc<dyn Tool>,
            Arc::new(second) as Arc<dyn Tool>,
        ],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::new(NullObserver),
            client: Some(GatewayClient::new(
                GatewayEndpoint::new(&format!("http://{addr}/v1")).expect("valid test endpoint"),
                SecretString::new("test"),
            )),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "text");
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[test]
fn near_duplicate_effective_scope_fails_before_the_model_without_payload_reports() {
    let first = ScopedFixtureTool::new("first", "first_wire", "First concrete.");
    let second = ScopedFixtureTool::new("second", "second_wire", "Second concrete.");
    let first_id = ToolId::new("tests", "first").expect("valid id");
    let second_id = ToolId::new("tests", "second").expect("valid id");
    let scope = crate::lua::ToolScope::from_bindings(vec![
        crate::lua::ToolBinding::for_test("first_local", "first", first_id.clone()),
        crate::lua::ToolBinding::for_test("second_local", "second", second_id.clone()),
    ]);
    let analysis = ToolAnalysis {
        alias_to_id: BTreeMap::from([
            ("first_local".to_owned(), first_id.clone()),
            ("second_local".to_owned(), second_id.clone()),
        ]),
        id_to_alias: BTreeMap::from([
            (first_id.clone(), "first_local".to_owned()),
            (second_id.clone(), "second_local".to_owned()),
        ]),
        near_duplicates: vec![OwnedNearDuplicate {
            first_id: first_id.clone(),
            second_id: second_id.clone(),
            similarity: 0.98,
        }],
    };
    let registry = ToolRegistry::new([&first as &dyn Tool, &second as &dyn Tool])
        .expect("unique test registry");
    let recorder = Arc::new(Recorder::default());

    let error = prepare_effective_scope(
        &analysis,
        &scope,
        &registry,
        EXECUTION,
        recorder.as_ref(),
        "Only",
    )
    .unwrap_err();

    assert!(matches!(
        error,
        Error::NearDuplicateTools {
            diagnostic,
        } if diagnostic.first_alias == "first_local"
            && diagnostic.first_id == ToolId::new("tests", "first").expect("valid id")
            && diagnostic.second_alias == "second_local"
            && diagnostic.second_id == ToolId::new("tests", "second").expect("valid id")
            && (diagnostic.similarity - 0.98).abs() < f32::EPSILON
    ));
    let events = recorder.events();
    assert!(
        events
            .iter()
            .any(|(_, detail)| { *detail == detail::TOOL_SCOPE_VALIDATION_FAILED.to_string() })
    );
    assert!(!events.iter().any(|(_, detail)| {
        *detail == detail::MODEL_TURN_COMPLETED.to_string()
            || *detail == detail::MODEL_TURN_FAILED.to_string()
    }));
    let trace = format!("{events:?}");
    for payload in ["first_local", "second_local", "Private similar description"] {
        assert!(!trace.contains(payload), "observation leaked {payload:?}");
    }
}
