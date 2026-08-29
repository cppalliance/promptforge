use super::super::*;
use super::run;
use super::*;

#[tokio::test]
async fn declared_tools_are_not_injected_without_always_or_add() {
    let gateway = ScriptedGateway::start(vec![resp_text("plain reply")]).await;
    let addr = gateway.addr();

    let tool = ScopedFixtureTool::new("concrete", "canonical_wire", "Concrete description.");
    let md = "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\ntools.bind('local_alias', 'capability')\nmodels.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\nAsk without tools.\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .unwrap();

    assert_eq!(out, "plain reply");
    let bodies = gateway.requests();
    assert_eq!(bodies.len(), 1);
    assert!(
        bodies[0].get("tools").is_none(),
        "declaring a bind must not expose it without explicit scope"
    );
}

#[tokio::test]
async fn always_advertises_concrete_schema_under_local_alias_and_dispatches_by_id() {
    let gateway = ScriptedGateway::start(aliased_tool_script("local_alias")).await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Concrete description.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('local_alias', 'capability')\n\
tools.always('local_alias')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\nUse the tool.\n",
        Vec::new(),
    );

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    let bodies = gateway.requests();
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
    let gateway = ScriptedGateway::start(aliased_tool_script("section_tool")).await;
    let addr = gateway.addr();
    let tool = Arc::new(ScopedFixtureTool::new(
        "concrete",
        "canonical_wire",
        "Section concrete.",
    ));
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('section_tool', 'capability')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.add('section_tool')\n```\n\nUse the tool.\n",
        Vec::new(),
    );

    let out = run(
        &prompt,
        "",
        &[Arc::clone(&tool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .unwrap();

    assert_eq!(out, "aliased final");
    assert_eq!(tool.calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        gateway.requests()[0]["tools"][0]["function"]["name"],
        "section_tool"
    );
}

#[tokio::test]
async fn near_duplicate_tools_are_valid_when_isolated_in_separate_sections() {
    let gateway = ScriptedGateway::start(vec![resp_text("text")]).await;
    let addr = gateway.addr();

    let first = ScopedFixtureTool::new("first", "first_wire", "First concrete.");
    let second = ScopedFixtureTool::new("second", "second_wire", "Second concrete.");
    let first_descriptor = picker_descriptor("first", "Similar operation one.");
    let second_descriptor = picker_descriptor("second", "Similar operation two.");
    let prompt = bound_with_tools(
        "---\nname: t\ndescription: d\npromptforge: 1\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('first_local', 'first')\n\
tools.bind('second_local', 'second')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
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
        gatewayed(addr),
    )
    .await
    .unwrap();

    assert_eq!(out, "text");
    assert_eq!(gateway.call_count(), 2);
}

/// The always-scope path: both halves of a bind-time clash enter every
/// section's scope through the `always` list, and the per-block scope
/// rebuild fails on them. (An end-to-end twin-scope failure needs the real
/// model to score two descriptions at or above the 0.98 duplicate
/// threshold, which near-identical bind capabilities cannot reach
/// deterministically - so the always path is pinned here, one layer up
/// from the scope check.)
#[test]
fn near_duplicate_always_scope_fails_at_the_scope_rebuild() {
    let first: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "first",
        "first_wire",
        "First concrete.",
    ));
    let second: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "second",
        "second_wire",
        "Second concrete.",
    ));
    let mut first_binding =
        crate::lua::ToolBinding::for_test("first_local", "first", Arc::clone(&first));
    first_binding.conflicts.push(crate::lua::Conflict {
        alias: "second_local".to_owned(),
        similarity: 0.98,
    });
    let mut second_binding =
        crate::lua::ToolBinding::for_test("second_local", "second", Arc::clone(&second));
    second_binding.conflicts.push(crate::lua::Conflict {
        alias: "first_local".to_owned(),
        similarity: 0.98,
    });
    let tool_set = crate::lua::ToolSet::for_test(
        vec![first_binding, second_binding],
        vec!["first_local".to_owned(), "second_local".to_owned()],
    );
    let runtime = Mutex::new(crate::lua::ToolRuntime {
        added: Vec::new(),
        description_overrides: BTreeMap::new(),
    });

    let effective = current_tool_bindings(&tool_set, &runtime).expect("the always scope snapshots");
    let error =
        prepare_effective_scope(&effective, &[], EXECUTION, &NullObserver::default(), "Only")
            .unwrap_err();

    assert!(
        matches!(error, Error::NearDuplicateTools { .. }),
        "the always scope must fail on the recorded clash: {error}"
    );
}

#[test]
fn near_duplicate_effective_scope_fails_before_the_model_without_payload_reports() {
    let first: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "first",
        "first_wire",
        "First concrete.",
    ));
    let second: Arc<dyn Tool> = Arc::new(ScopedFixtureTool::new(
        "second",
        "second_wire",
        "Second concrete.",
    ));
    // Mirror the bind-time record: each half of the clash carries the other
    // half's alias and the picker's score.
    let mut first_binding =
        crate::lua::ToolBinding::for_test("first_local", "first", Arc::clone(&first));
    first_binding.conflicts.push(crate::lua::Conflict {
        alias: "second_local".to_owned(),
        similarity: 0.98,
    });
    let mut second_binding =
        crate::lua::ToolBinding::for_test("second_local", "second", Arc::clone(&second));
    second_binding.conflicts.push(crate::lua::Conflict {
        alias: "first_local".to_owned(),
        similarity: 0.98,
    });
    let bindings = vec![first_binding, second_binding];
    let recorder = Arc::new(Recorder::default());

    let error =
        prepare_effective_scope(&bindings, &[], EXECUTION, recorder.as_ref(), "Only").unwrap_err();

    assert!(matches!(
        error,
        Error::NearDuplicateTools {
            diagnostic,
        } if diagnostic.first_alias == "first_local"
            && diagnostic.first_id == ToolId::new("tests", "first").expect("valid id")
            && diagnostic.second_alias == "second_local"
            && diagnostic.second_id == ToolId::new("tests", "second").expect("valid id")
            && (diagnostic.similarity - 0.98).abs() < f64::EPSILON
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
