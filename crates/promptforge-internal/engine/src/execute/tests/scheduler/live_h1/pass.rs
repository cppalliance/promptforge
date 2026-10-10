//! The live H1 pass itself: its infer and default-model selection, the
//! root entry ids of the chunk and the first walked section, H1 assertion
//! failures and their reports, `var` read back after a scalar return, a
//! shared-replay failure's Lua kind, and the section boundaries the pass
//! does not fire.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn live_h1_infer_runs_once() {
    // The H1 pass selects the default model by label, a handle's `infer`
    // yields through the shim, and the H1 `var` hand-off seeds the walk.
    let gateway = ScriptedChat::new(vec![resp_text("h1 answer")]);
    let md = "---\nname: live-h1\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        var.answer = writer:infer('answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("the H1 pass must run on the scheduler");

    assert_eq!(out, "h1 answer");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_models_infer_resolves_the_default_model_without_touching_sys() {
    // The H1 `models.infer` (no handle) resolves the current model from the
    // shared set and runs the one infer shape - a single tool-free
    // round on a fresh conversation that leaves `sys` untouched.
    let gateway = ScriptedChat::new(vec![resp_text("h1 answer")]);
    let md = "---\nname: live-h1-models-infer\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1 Models Infer\n\n\
        ```lua\n\
        models.default('writer')\n\
        var.answer = models.infer('answer once')\n\
        var.sys_untouched = not pcall(function() return sys.reply_finish_reason end)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer .. ':' .. tostring(var.sys_untouched)\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, Some(gateway_client(&gateway)))
        .drive()
        .await
        .expect("H1 models.infer must run on the scheduler");

    assert_eq!(out, "h1 answer:true");
    assert_eq!(gateway.call_count(), 1);
    let body = gateway
        .last_request()
        .expect("infer must reach the gateway");
    assert_eq!(
        body.options.model(),
        "claude-sonnet-4-6",
        "models.infer must use the section's current model"
    );
    assert!(
        body.tools.is_empty(),
        "models.infer advertises no tools: {body:?}"
    );
    assert_eq!(
        body.messages.len(),
        1,
        "models.infer runs on a fresh context: {body:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_chunk_takes_root_entry_zero_and_the_first_walked_section_takes_root_entry_one() {
    // The H1 pass is the root chain's entry 0, so the first walked section
    // takes entry 1 of the same chain.
    let md = "---\nname: live-h1-sys-id\ndescription: d\npromptforge: 0\n---\n\n\
        # Live H1 Sys Id\n\n\
        ```lua\n\
        assert(sys.id == '0.0', 'the H1 chunk takes the root chain entry 0')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\n\
        assert(sys.id == '0.1', 'the first walked section takes entry 1')\n\
        return 'ok'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the H1 chunk takes root entry 0 and the first walked section root entry 1");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_h1_assertion_ends_the_run_as_requirements_unmet() {
    // H1's remaining job is the prompt's hard gates: a failed `assert` is
    // the failed assertion, ending the run before the walk with the
    // RequirementsUnmet classification and the failure notice as content.
    let md = "---\nname: h1-gate\ndescription: d\npromptforge: 0\n---\n\n\
        # Gate\n\n\
        ```lua\n\
        assert(false, 'the gate cannot hold')\n\
        ```\n\n\
        ```lua\nstore.write('later.txt', 'ran')\n```\n\n\
        ## Result\n\n\
        ```lua\nreturn 'unexpected'\n```\n";
    let prompt = parse(md);
    let store = TestStore::new();
    let (ctx, fixture) = h1_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("a failed H1 assertion must fail the run");

    assert!(
        matches!(error, Error::RequirementsUnmet { .. }),
        "the failed assertion classifies as RequirementsUnmet: {error}"
    );
    assert!(
        error.to_string().contains("the gate cannot hold"),
        "the notice includes the assertion's message: {error}"
    );
    assert!(
        store.read("later.txt").is_err(),
        "the later H1 block must not run after the failed gate"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_uncaught_h1_assertion_reports_the_chunk_failed() {
    // The observation boundary of the H1 gate rule: an uncaught assertion
    // failure is the chunk's own failure, so the chunk reports
    // LUA_CHUNK_FAILED and the run ends as RequirementsUnmet.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: callback-drain\ndescription: d\npromptforge: 0\n---\n\n\
        # Callback Drain\n\n\
        ```lua\n\
        assert(false, 'the gate cannot hold')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context_on(&prompt, &TestStore::new(), recorder.clone());
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("the failed gate must fail the run");

    assert!(
        matches!(error, Error::RequirementsUnmet { .. }),
        "the failed gate classifies as RequirementsUnmet: {error}"
    );
    let observed = recorder.events();
    let title = "Callback Drain".to_string();
    assert!(
        observed.contains(&(title.clone(), detail::LUA_CHUNK_FAILED.to_string())),
        "the chunk with the failed gate reports failed: {observed:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_h1_scalar_return_still_reads_var_back() {
    // The read-back half of the H1 return rule: the pass reads the
    // final `var` back on every exit, so a reassigned `var` global fails
    // the run even when the block's scalar return would short-circuit it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Reassigned Var\n\n\
        ```lua\n\
        var = 5\n\
        return 'early'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect_err("a reassigned `var` global must fail the run");

    assert!(
        matches!(error, Error::Lua(_)),
        "the read-back failure is machinery around the prompt's chunk, \
         not the failed gate, so it keeps the Lua kind: {error}"
    );
    assert!(
        error.to_string().contains("global was reassigned"),
        "the read-back failure must name the cause: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_shared_replay_failure_in_h1_keeps_its_lua_kind() {
    // The other half of the remap's boundary: the shared replay is
    // machinery around the prompt's chunk, not the chunk itself, so its
    // failure is a prompt bug under the Lua kind, never the H1 gate's
    // RequirementsUnmet. The context holds the prompt's real compiled
    // shared library, not the empty stand-in the other H1 tests use.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Gate\n\n\
        ```lua shared\n\
        error('shared boom')\n\
        ```\n\n\
        ```lua\n\
        var.ok = true\n\
        ```\n";
    let prompt = parse(md);
    let shared = promptforge_parser::detail::replay(&prompt)
        .cloned()
        .expect("the prompt's shared chunk compiles at parse");
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        shared,
        &test_context(EXECUTION),
    );
    let error = TokioDriver::new(&ctx, RunFixture::new(), None)
        .drive()
        .await
        .expect_err("a failing shared replay must fail the run");

    assert!(
        matches!(error, Error::Lua(_) | Error::LuaRuntime { .. }),
        "the shared replay failure keeps its Lua kind: {error}"
    );
    assert!(
        error.to_string().contains("shared boom"),
        "the failure names the shared chunk's error: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_live_h1_pass_fires_no_section_boundaries() {
    // The completion-flag contract on the scheduler: the H1 frame never
    // arms completion and is never a walked section, so the pass reports
    // its teardown pair but neither SECTION_STARTED nor SECTION_FINISHED;
    // the first walked section reports both.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Boundaries\n\n\
        ```lua\nvar.x = 1\n```\n\n\
        ## Only\n\n\
        ```lua\nreturn 'done-now'\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = h1_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the pass and the walk complete");

    assert_eq!(out, "done-now");
    let observed = recorder.events();
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    assert!(
        observed.contains(&("Only".to_string(), started.clone())),
        "the walked section reports started: {observed:?}"
    );
    assert!(
        observed.contains(&("Only".to_string(), finished.clone())),
        "the walked section reports finished: {observed:?}"
    );
    assert!(
        observed
            .iter()
            .any(|(section, event)| section == "Boundaries"
                && event == &detail::LUA_TEARDOWN_SUCCEEDED.to_string()),
        "the H1 frame's drop fires the teardown pair: {observed:?}"
    );
    assert!(
        !observed
            .iter()
            .any(|(section, event)| section == "Boundaries"
                && (event == &started || event == &finished)),
        "the live H1 pass fires no section boundaries: {observed:?}"
    );
}
