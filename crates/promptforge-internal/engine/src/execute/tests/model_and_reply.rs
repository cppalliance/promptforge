//! Tests for section model selection and the prologue and epilog phases.
//! The `sys.model`, `item`, and `reply` reads sit in `globals`, and the
//! leading-handle `models.get` and `models.infer` cases in `handles`.

use super::run;
use super::*;

#[tokio::test]
async fn models_use_forwards_binding_completion_options_to_the_gateway() {
    // models.use -> completion_options -> GatewayClient::complete must set
    // the binding's model, the hard-keyword thinking switch, and the
    // section's `models.use` sampling options on the chat body. Roles
    // declare no sampling fields, so a section on the prompt-wide default
    // sends none.
    let gateway = ScriptedGateway::start(vec![
        resp_text("hello from the mock"),
        resp_text("hello again"),
    ])
    .await;
    let addr = gateway.addr();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  analyst:\n    keywords: [no-thinking]\n---\n\n\
# T\n\n\
```lua\nmodels.default('analyst')\n```\n\n\
## Only\n\n\
```lua\nmodels.use('analyst', { temperature = 0, max_tokens = 256 })\n```\n\n\
Ask the model.\n\n\
```lua\nmodels.infer(prose)\n```\n\n\
## Next\n\n\
Ask again.\n\n\
```lua\nreturn models.infer(prose)\n```\n";
    let prompt = Prompt::parse(md, EXECUTION).0.expect("fixture must parse");
    let mut ctx = test_context(EXECUTION);
    ctx.model_bindings.bind(
        "analyst",
        ModelDescriptor::new(
            ModelId::gateway("analyst").expect("the test model alias is valid"),
            "A careful analysis model",
            NonZeroU32::new(131_072).expect("131072 is non-zero"),
            ThinkingMode::Switchable,
        ),
    );
    let harness = RunHarness::new().client(gateway_client(addr));
    let out = match crate::test_support::run_harness(&prompt, "", ctx, harness).await {
        RunResult::Ok(out) => out,
        other => panic!("the run must succeed: {other:?}"),
    };
    assert_eq!(out, "hello again");

    let requests = gateway.requests();
    assert_eq!(requests.len(), 2, "one round per section: {requests:?}");
    let selected = &requests[0];
    assert_eq!(selected["model"], "analyst");
    assert_eq!(selected["chat_template_kwargs"]["enable_thinking"], false);
    assert_eq!(selected["temperature"], 0.0);
    assert_eq!(selected["max_tokens"], 256);
    let defaulted = &requests[1];
    assert_eq!(defaulted["model"], "analyst");
    assert_eq!(defaulted["chat_template_kwargs"]["enable_thinking"], false);
    assert!(
        defaulted.get("temperature").is_none() && defaulted.get("max_tokens").is_none(),
        "a section on the prompt-wide default sends neither option: {defaulted}"
    );
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
        &TestStore::new(),
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
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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
    let entry = promptforge_parser::detail::entry(prompt.prompt()).expect("fixture has sections");
    assert!(entry.prologue().is_none());
    assert!(entry.epilog().is_some());

    let recorder = Arc::new(Recorder::default());
    let store = TestStore::new();
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
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Test prompt".to_string(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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
    let error = run(&prompt, "", &[], &TestStore::new(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("is not a bound tool slot"),
        "the error must report the missing slot: {error}"
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
    let error = run(&fixture(md), "", &[], &TestStore::new(), silent())
        .await
        .expect_err("an undeclared alias must fail the run");
    assert!(
        error.to_string().contains("is not a bound tool slot"),
        "the error must report the missing slot: {error}"
    );
}

#[tokio::test]
async fn prologue_return_skips_model_and_epilog() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
# Test prompt\n\n\
## Only\n\n```lua\nreturn 'early'\n```\n\n\
This prose must not reach a model.\n\n\
```lua\nstore.write('epilog-ran.txt', 'yes')\nreturn 'late'\n```\n";
    let store = TestStore::new();
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
        &TestStore::new(),
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
                detail::LUA_SHARED_LOAD_STARTED.to_string(),
            ),
            (
                "Test prompt".to_owned(),
                detail::LUA_SHARED_LOAD_SUCCEEDED.to_string(),
            ),
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
        run(&fixture(md), "", &[], &TestStore::new(), silent())
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
        run(&fixture(md), "", &[], &TestStore::new(), silent())
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
    let error = run(&fixture(md), "", &[], &TestStore::new(), silent())
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

#[path = "model_and_reply-globals.rs"]
mod globals;
#[path = "model_and_reply-handles.rs"]
mod handles;
