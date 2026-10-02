//! `models.get` and `models.infer` with a leading handle: a handle
//! leaves the section model alone, `models.infer` without one uses the
//! section model and never binds `reply`, `models.use` re-selection
//! steers the next round, and a section with no current model fails
//! `models.infer` but still infers through a handle.

use super::*;

/// Runs a parsed prompt against a scripted gateway with no external tools.
async fn run_with_gateway(
    test: &TestPrompt,
    addr: SocketAddr,
    store: &TestStore,
) -> Result<String> {
    run(test, "", &[], store, gatewayed(addr)).await
}

/// Runs a prompt with hand-filled model bindings and no prepare pass: the
/// multi-model shape v1's trivial fill cannot produce (every role bound to
/// the one current model), exercising the runtime's label resolution
/// directly. `bindings` pairs a declared role label with the gateway model
/// id it resolves to.
async fn run_with_bindings(
    md: &str,
    bindings: &[(&str, &str)],
    addr: SocketAddr,
    store: &TestStore,
) -> Result<String> {
    let prompt = parse(md);
    let mut ctx = test_context(EXECUTION).vfs(store.vfs());
    for (label, model) in bindings {
        ctx.model_bindings.bind(
            label,
            ModelDescriptor::new(
                ModelId::gateway(*model).expect("the test model id is valid"),
                "A test model",
                NonZeroU32::new(131_072).expect("131072 is non-zero"),
                ThinkingMode::Switchable,
            ),
        );
    }
    let harness = RunHarness::new().client(gateway_client(addr));
    match crate::test_support::run_harness(&prompt, "", ctx, harness).await {
        RunResult::Ok(out) => Ok(out),
        RunResult::Cancelled => Err(Error::Interrupted),
        RunResult::Failure(error) => Err(Error::from(error)),
    }
}

#[tokio::test]
async fn models_get_returns_a_handle_without_changing_the_section_model() {
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n  analyst: {}\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer')\n\
```\n\n\
## Only\n\n\
```lua\nstore.write('handle.txt', models.get('analyst').name)\n```\n\n\
Ask the model.\n\n\
```lua\nreturn models.infer(prose)\n```\n";
    let store = TestStore::new();
    let out = run_with_bindings(
        md,
        &[("writer", "writer-model"), ("analyst", "analyst-model")],
        addr,
        &store,
    )
    .await
    .unwrap();

    assert_eq!(out, "hello from the mock");
    assert_eq!(
        store.read("handle.txt").unwrap(),
        "analyst",
        "models.get must return the analyst handle"
    );
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(
        body["model"], "writer-model",
        "models.get must not change the section's model"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_infer_uses_the_section_model_without_touching_reply() {
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n\
```lua\nvar.r = models.infer('ping')\n```\n\n\
```lua\nreturn var.r .. ':' .. tostring(reply)\n```\n";
    let out = run_with_gateway(&bound_for_model(md), addr, &TestStore::new())
        .await
        .unwrap();
    assert_eq!(
        out, "pong:nil",
        "models.infer must not bind the section's reply"
    );

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body["model"], "claude-sonnet-4-6");
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_infer_uses_that_model_regardless_of_the_section_model() {
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n  analyst: {}\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer')\n\
```\n\n\
## Only\n\n\
```lua\nreturn models.infer(models.get('analyst'), 'ping')\n```\n";
    let out = run_with_bindings(
        md,
        &[("writer", "writer-model"), ("analyst", "analyst-model")],
        addr,
        &TestStore::new(),
    )
    .await
    .unwrap();
    assert_eq!(out, "pong");
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(
        body["model"], "analyst-model",
        "a leading handle must use the handle's model, not the section default"
    );
}

#[tokio::test]
async fn models_use_reselection_steers_the_next_round() {
    let gateway = ScriptedGateway::start(vec![resp_text("first"), resp_text("second")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n  analyst: {}\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer')\n\
```\n\n\
## Only\n\n\
```lua\n\
models.use('writer')\n\
models.infer('ping')\n\
models.use('analyst')\n\
return models.infer('ping')\n\
```\n";
    let out = run_with_bindings(
        md,
        &[("writer", "writer-model"), ("analyst", "analyst-model")],
        addr,
        &TestStore::new(),
    )
    .await
    .expect("re-selection within a section must succeed");
    assert_eq!(out, "second");
    let requests = gateway.requests();
    assert_eq!(
        requests.len(),
        2,
        "both infer rounds must reach the gateway"
    );
    assert_eq!(
        requests[0]["model"], "writer-model",
        "the first round uses the initial selection"
    );
    assert_eq!(
        requests[1]["model"], "analyst-model",
        "the second round uses the re-selected model"
    );
}

#[tokio::test]
async fn models_infer_without_use_or_default_errors() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  analyst: {}\n---\n\n\
# T\n\n\
## Only\n\n\
```lua\nreturn models.infer('ping')\n```\n";
    let prompt = parse(md);
    let mut ctx = test_context(EXECUTION);
    ctx.model_bindings.bind(
        "analyst",
        ModelDescriptor::new(
            ModelId::gateway("analyst-model").expect("the analyst model id is valid"),
            "A careful analysis model",
            NonZeroU32::new(131_072).expect("131072 is non-zero"),
            ThinkingMode::Switchable,
        ),
    );
    let error = match crate::test_support::run_harness(&prompt, "", ctx, RunHarness::new()).await {
        RunResult::Failure(error) => error,
        other => panic!("models.infer with no current model must fail: {other:?}"),
    };
    assert!(
        error
            .to_string()
            .contains("model binding required for section Only"),
        "the error must name the section: {error}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn models_get_infer_works_without_any_section_model() {
    let gateway = ScriptedGateway::start(vec![resp_text("pong")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  analyst: {}\n---\n\n\
# T\n\n\
## Only\n\n\
```lua\nreturn models.infer(models.get('analyst'), 'ping')\n```\n";
    let out = run_with_bindings(md, &[("analyst", "analyst-model")], addr, &TestStore::new())
        .await
        .unwrap();
    assert_eq!(out, "pong");
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body["model"], "analyst-model");
}
