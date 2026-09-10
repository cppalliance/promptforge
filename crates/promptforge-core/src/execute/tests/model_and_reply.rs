use super::super::*;
use super::run;
use super::*;

#[tokio::test]
async fn models_use_forwards_binding_completion_options_to_the_gateway() {
    // models.use -> completion_options -> GatewayClient::complete must carry
    // the binding's model and sampling fields on the chat body.
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let catalog = ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("analyst").expect("the test model alias is valid"),
        "A careful analysis model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Switchable,
    )])
    .expect("the test catalog has a single unique model");
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\n\
models.bind('analyst', 'careful analysis', { temperature = 0.25, max_tokens = 64, thinking = false })\n\
```\n\n\
## Only\n\n\
```lua\nmodels.use('analyst')\n```\n\n\
Ask the model.\n\n\
```lua\nreturn models.infer(prose)\n```\n";
    let prompt =
        Prompt::parse(md, EXECUTION, &NullObserver::default()).expect("fixture must parse");
    let prompt = TestPrompt {
        prompt,
        models: catalog,
        picker_catalog: None,
    };

    let out = run(&prompt, "", &[], &StoreRef::memory(), gatewayed(addr))
        .await
        .unwrap();
    assert_eq!(out, "hello from the mock");

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body["model"], "analyst");
    assert_eq!(body["temperature"], 0.25);
    assert_eq!(body["max_tokens"], 64);
    assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
}

#[tokio::test]
async fn an_explicit_client_is_used_instead_of_the_environment() {
    // `client: Some(..)` is what a caller configured from a file passes;
    // nothing here reads `PROMPTFORGE_*`, and the run still reaches a
    // gateway and reports its model turn.
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nSay something.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let recorder = Arc::new(Recorder::default());
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(gateway_client(addr)),
            debug: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");

    assert_eq!(
        recorder.events(),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
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
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string(),),
            ("Only".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string()),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn epilog_runs_after_prose_and_can_return() {
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nSay something.\n\n```lua\n\
local text = models.infer(prose)\n\
assert(text == 'hello from the mock')\n\
store.write('epilog-ran.txt', 'yes')\n\
return 'epilog result'\n\
```\n";
    let prompt = bound_for_model(md);
    let entry = prompt.prompt().entry().expect("fixture has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_some());

    let recorder = Arc::new(Recorder::default());
    let store = StoreRef::memory();
    let out = run(
        &prompt,
        "",
        &[],
        &store,
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(gateway_client(addr)),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "epilog result");
    assert_eq!(store.read("epilog-ran.txt").unwrap(), "yes");
    assert_eq!(
        recorder.events(),
        vec![
            ("Test prompt".to_string(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
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
            ("Only".to_string(), detail::MODEL_TURN_COMPLETED.to_string()),
            (
                "Only".to_string(),
                detail::STORE_WRITE_SUCCEEDED.to_string()
            ),
            ("Only".to_string(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_string(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_string(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_string(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_string(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn add_without_h1_bindings_fails_the_run_loudly() {
    // Input with no shared library goes through the same validated VM with
    // empty frozen bindings, so the alias is rejected.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\ntools.add('web_search')\n```\n\nThis prose must not reach a model.\n";
    let prompt = fixture(md);
    let error = run(&prompt, "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("not declared by tools.bind"),
        "the error must report the missing declaration: {error}"
    );
}

#[tokio::test]
async fn add_with_an_empty_shared_library_fails_the_run_loudly() {
    // A prompt whose shared library declares nothing closes over empty frozen
    // bindings, so tools.add in a prologue is rejected the same way.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
```lua\nfunction helper() return 'no declarations' end\n```\n\n\
## Only\n\n```lua\ntools.add('web_search')\n```\n\nThis prose must not reach a model.\n";
    let error = run(&fixture(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("not declared by tools.bind"),
        "the error must report the missing declaration: {error}"
    );
}

#[tokio::test]
async fn prologue_return_skips_model_and_epilog() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nreturn 'early'\n```\n\n\
This prose must not reach a model.\n\n\
```lua\nstore.write('epilog-ran.txt', 'yes')\nreturn 'late'\n```\n";
    let store = StoreRef::memory();
    let out = run(&fixture(md), "", &[], &store, silent()).await.unwrap();

    assert_eq!(out, "early");
    assert!(store.read("epilog-ran.txt").is_err());
}

#[tokio::test]
async fn shared_helper_survives_prologue_model_and_epilog() {
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
```lua shared\nfunction decorate(value) return '<' .. value .. '>' end\n```\n\n\
## Only\n\n```lua\nvar.question = decorate(args)\n```\n\n\
Ask using {{ var.question }}.\n\n\
```lua\nreturn decorate(models.infer(prose))\n```\n";
    let recorder = Arc::new(Recorder::default());
    let out = run(
        &bound_for_model(md),
        "input",
        &[],
        &StoreRef::memory(),
        RunOptions {
            execution: EXECUTION,
            observer: Arc::clone(&recorder) as Arc<dyn Observer>,
            client: Some(gateway_client(addr)),
            debug: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(out, "<hello from the mock>");
    assert_eq!(
        recorder.events(),
        [
            ("Test prompt".to_owned(), detail::RUN_STARTED.to_string()),
            (
                "Test prompt".to_owned(),
                detail::LUA_CHUNK_STARTED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_CHUNK_SUCCEEDED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_TEARDOWN_STARTED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_owned(), detail::SECTION_STARTED.to_string()),
            (
                "Only".to_owned(),
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Only".to_owned(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
            ("Only".to_owned(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_owned(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_owned(), detail::LUA_CHUNK_STARTED.to_string()),
            ("Only".to_owned(), detail::MODEL_TURN_COMPLETED.to_string(),),
            ("Only".to_owned(), detail::LUA_CHUNK_SUCCEEDED.to_string(),),
            ("Only".to_owned(), detail::LUA_TEARDOWN_STARTED.to_string()),
            (
                "Only".to_owned(),
                detail::LUA_TEARDOWN_SUCCEEDED.to_string(),
            ),
            ("Only".to_owned(), detail::SECTION_FINISHED.to_string()),
            ("Test prompt".to_owned(), detail::RUN_SUCCEEDED.to_string()),
        ]
    );
}

#[tokio::test]
async fn empty_prose_skips_model_but_runs_epilog_with_nil_reply() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nvar.phase = 'prologue'\n```\n\n\
```lua\nif reply ~= nil then error('empty prose must not bind a reply') end\nreturn var.phase .. '-epilog'\n```\n";

    assert_eq!(
        run(&fixture(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "prologue-epilog"
    );
}

#[tokio::test]
async fn whitespace_only_prose_skips_model_without_binding() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\n-- prologue\n```\n\n   \n\t\n\n\
```lua\nif reply ~= nil then error('whitespace prose must not bind a reply') end\nreturn 'ok'\n```\n";
    assert_eq!(
        run(&fixture(md), "", &[], &StoreRef::memory(), silent())
            .await
            .unwrap(),
        "ok"
    );
}

#[tokio::test]
async fn model_required_when_infer_has_no_binding() {
    // Prose itself never requires a model; only an explicit `models.infer`
    // of it does.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\nAsk the model.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let error = run(&fixture(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("an explicit infer without a model binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model binding required for section Only"),
        "error must name the section: {error}"
    );
}

#[tokio::test]
async fn shared_function_sees_sys_model_unknown_before_scope_close() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
```lua\nmodels.default('writer', 'A general model for tests')\n```\n\n\
```lua shared\nfunction read_sys_model()\n  return sys.model\nend\n```\n\n\
## Only\n\n```lua\nreturn read_sys_model()\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("shared function must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn prologue_sys_model_unknown_before_scope_close() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn sys.model\n```\n\nprose\n";
    let error = run(&bound_for_model(md), "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("prologue must not read sys.model before scope close");
    assert!(
        error.to_string().contains("unknown sys field 'model'"),
        "error must name the missing field: {error}"
    );
}

#[tokio::test]
async fn prose_substitution_sees_sys_model_catalog_id() {
    // The first script dispatch runs the one-time scope install, which
    // enriches `sys.model` with the bound catalog id; a prose read after it
    // substitutes the catalog id, not the alias.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('echo', 'echo tool')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.call('echo', { value = 'x' })\n```\n\nModel id is {{ sys.model }}.\n\n\
```lua\nreturn prose\n```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "Model id is claude-sonnet-4-6.");
}

#[tokio::test]
async fn epilog_sees_model_catalog_id_not_alias_after_the_scope_install() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('echo', 'echo tool')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Only\n\n```lua\ntools.call('echo', { value = 'x' })\n```\n\n```lua\nreturn sys.model\n```\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "claude-sonnet-4-6");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_sees_sys_model_catalog_id_after_the_scope_install() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n```lua shared\n\
tools.bind('echo', 'echo tool')\n\
models.default('writer', 'A general model for tests')\n```\n\n\
## Parent\n\n```lua\nlocal r = fanout('### Worker', list_from_section('### Items'))\nreturn table.concat(r, ',')\n```\n\n\
### Worker\n\n```lua\ntools.call('echo', { value = item })\n```\n\n\
```lua\nreturn sys.model .. ':' .. item\n```\n\n\
### Items\n\n- a\n";
    let prompt = bound_with_tools(md, Vec::new());
    let out = run(
        &prompt,
        "",
        &[Arc::new(EchoTool) as Arc<dyn Tool>],
        &StoreRef::memory(),
        silent(),
    )
    .await
    .unwrap();
    assert_eq!(out, "claude-sonnet-4-6:a");
}

/// `{{ item }}` renders a non-string member per its type: here a table
/// member reaches the model as compact JSON.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_item_substitution_renders_a_table_member_as_compact_json() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Parent\n\n```lua\nlocal r = fanout('### Worker', {{7, 'x'}})\nreturn r[1].text\n```\n\n\
### Worker\n\n```lua\n-- prologue\n```\n\nItem: {{ item }}.\n\n```lua\nreturn models.infer(prose)\n```\n";
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let out = run(
        &bound_for_model(md),
        "",
        &[],
        &StoreRef::memory(),
        gatewayed(addr),
    )
    .await
    .unwrap();
    assert_eq!(out, "hello from the mock");

    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    let user_content = body["messages"]
        .as_array()
        .and_then(|messages| messages.first())
        .and_then(|message| message["content"].as_str())
        .expect("first message must carry substituted prose");
    assert!(
        user_content.contains("Item: [7,\"x\"]."),
        "a table member must render as compact JSON, got: {user_content}"
    );
}

// --- Reply removal ---

#[tokio::test]
async fn reply_is_nil_in_first_section() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n```lua\nreturn tostring(reply)\n```\n";
    let out = run_offline(md).await.unwrap();
    assert_eq!(out, "nil");
}

#[tokio::test]
async fn reply_substitution_is_an_unknown_global_error() {
    // The reply register is gone: `{{ reply }}` names no namespace and no
    // bare global, so reading the prose fails at the read site.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
## Only\n\n{{ reply }}\n\n```lua\nreturn prose\n```\n";
    let err = run_offline(md)
        .await
        .expect_err("{{ reply }} names no global and must error");
    assert!(
        err.to_string().contains("reply"),
        "error must mention reply, got: {err}"
    );
}

// --- models.get / models.infer with a leading handle ---

/// A two-model catalog: `writer` resolves to `writer-model`, `analyst` to
/// `analyst-model`, so a test can tell which model a request used.
fn writer_and_analyst_catalog() -> ModelCatalog {
    let context = NonZeroU32::new(131_072).expect("131072 is non-zero");
    ModelCatalog::new([
        ModelDescriptor::new(
            ModelId::gateway("writer-model").expect("the writer model id is valid"),
            "A general model for tests",
            context,
            ThinkingMode::Switchable,
        ),
        ModelDescriptor::new(
            ModelId::gateway("analyst-model").expect("the analyst model id is valid"),
            "A careful analysis model",
            context,
            ThinkingMode::Switchable,
        ),
    ])
    .expect("the test catalog has two unique models")
}

fn analyst_only_catalog() -> ModelCatalog {
    ModelCatalog::new([ModelDescriptor::new(
        ModelId::gateway("analyst-model").expect("the analyst model id is valid"),
        "A careful analysis model",
        NonZeroU32::new(131_072).expect("131072 is non-zero"),
        ThinkingMode::Switchable,
    )])
    .expect("the test catalog has a single unique model")
}

/// Run a parsed prompt against a scripted gateway with no external tools.
async fn run_with_gateway(test: &TestPrompt, addr: SocketAddr, store: &StoreRef) -> Result<String> {
    run(test, "", &[], store, gatewayed(addr)).await
}

#[tokio::test]
async fn models_get_returns_a_handle_without_changing_the_section_model() {
    let gateway = ScriptedGateway::start(vec![resp_text("hello from the mock")]).await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer', 'A general model for tests')\n\
models.bind('analyst', 'A careful analysis model')\n\
```\n\n\
## Only\n\n\
```lua\nstore.write('handle.txt', models.get('analyst').name)\n```\n\n\
Ask the model.\n\n\
```lua\nreturn models.infer(prose)\n```\n";
    let prompt = TestPrompt {
        prompt: parse(md),
        models: writer_and_analyst_catalog(),
        picker_catalog: None,
    };
    let store = StoreRef::memory();
    let out = run_with_gateway(&prompt, addr, &store).await.unwrap();

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
    let out = run_with_gateway(&bound_for_model(md), addr, &StoreRef::memory())
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer', 'A general model for tests')\n\
models.bind('analyst', 'A careful analysis model')\n\
```\n\n\
## Only\n\n\
```lua\nreturn models.infer(models.get('analyst'), 'ping')\n```\n";
    let prompt = TestPrompt {
        prompt: parse(md),
        models: writer_and_analyst_catalog(),
        picker_catalog: None,
    };
    let out = run_with_gateway(&prompt, addr, &StoreRef::memory())
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\n\
models.default('writer', 'A general model for tests')\n\
models.bind('analyst', 'A careful analysis model')\n\
```\n\n\
## Only\n\n\
```lua\n\
models.use('writer')\n\
models.infer('ping')\n\
models.use('analyst')\n\
return models.infer('ping')\n\
```\n";
    let prompt = TestPrompt {
        prompt: parse(md),
        models: writer_and_analyst_catalog(),
        picker_catalog: None,
    };
    let out = run_with_gateway(&prompt, addr, &StoreRef::memory())
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\nmodels.bind('analyst', 'A careful analysis model')\n```\n\n\
## Only\n\n\
```lua\nreturn models.infer('ping')\n```\n";
    let prompt = TestPrompt {
        prompt: parse(md),
        models: analyst_only_catalog(),
        picker_catalog: None,
    };
    let error = run(&prompt, "", &[], &StoreRef::memory(), silent())
        .await
        .expect_err("models.infer with no current model must fail");
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
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# T\n\n\
```lua\nmodels.bind('analyst', 'A careful analysis model')\n```\n\n\
## Only\n\n\
```lua\nreturn models.infer(models.get('analyst'), 'ping')\n```\n";
    let prompt = TestPrompt {
        prompt: parse(md),
        models: analyst_only_catalog(),
        picker_catalog: None,
    };
    let out = run_with_gateway(&prompt, addr, &StoreRef::memory())
        .await
        .unwrap();
    assert_eq!(out, "pong");
    let body = gateway
        .last_request()
        .expect("complete must reach the gateway");
    assert_eq!(body["model"], "analyst-model");
}
