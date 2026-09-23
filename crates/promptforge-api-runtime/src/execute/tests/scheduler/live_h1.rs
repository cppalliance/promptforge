//! The live H1 pass on the scheduler.

use super::*;
use crate::test_support::tokio_driver::TokioDriver;

// --- The live H1 pass on the scheduler ---

/// Builds the run context for a scheduler live-H1 test: the shared model
/// set starts empty - the live H1 pass under test records its own
/// bindings, exactly as the legacy run's H1 hand-off leaves them.
fn h1_context(prompt: &Prompt) -> (RunState, RunHost) {
    h1_context_on(prompt, &TestStore::new(), Arc::new(NullObserver::default()))
}

/// Builds the H1 run context and its observing host on the given store and
/// observer, so a pass test can inspect the store's contents and the
/// observation stream afterward. The context's model bindings are filled the
/// way prepare's trivial fill does: every declared role bound to the test
/// model.
fn h1_context_on(
    prompt: &Prompt,
    store: &TestStore,
    observer: Arc<dyn Observer>,
) -> (RunState, RunHost) {
    let mut ctx = test_context(EXECUTION);
    for (label, _) in prompt.frontmatter().models().iter() {
        ctx.model_bindings.bind(
            label,
            ModelDescriptor::new(
                ModelId::gateway("claude-sonnet-4-6").expect("the test model id is valid"),
                "A general model for tests",
                NonZeroU32::new(131_072).expect("131072 is non-zero"),
                ThinkingMode::Switchable,
            ),
        );
    }
    let state = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &store.vfs(),
        LuaProgram::empty().expect("the empty chunk compiles"),
        &ctx,
    );
    (state, RunHost::new().observer(observer))
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_infer_runs_once() {
    // Mirror of the legacy case of the same name: the H1 pass selects the
    // default model by label, a handle's `infer` yields through the shim,
    // and the H1 `var` hand-off seeds the walk.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
    let md = "---\nname: live-h1\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Live H1\n\n\
        ```lua\n\
        local writer = models.default('writer')\n\
        var.answer = models.infer(writer, 'answer once')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the H1 pass must run on the scheduler");

    assert_eq!(out, "h1 answer");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_models_infer_resolves_the_default_model_without_touching_sys() {
    // Mirror of the legacy case of the same name: the H1
    // `models.infer` (no handle) resolves the current model from the
    // shared set and runs the one infer shape - a single tool-free
    // round on a fresh conversation that leaves `sys` untouched.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 answer")]).await;
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
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("H1 models.infer must run on the scheduler");

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

#[tokio::test(flavor = "current_thread")]
async fn live_h1_chunk_takes_root_entry_zero_and_the_first_walked_section_takes_root_entry_one() {
    // Mirror of the legacy case of the same name: the H1 pass is the root
    // chain's entry 0, so the first walked section takes entry 1 of the
    // same chain.
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
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
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
    let (ctx, host) = h1_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, None)
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
    let (ctx, host) = h1_context_on(&prompt, &TestStore::new(), recorder.clone());
    let error = TokioDriver::new(&ctx, host, None)
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
    let (ctx, host) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
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
    let shared = prompt
        .replay()
        .cloned()
        .expect("the prompt's shared chunk compiles at parse");
    let ctx = RunState::new(
        Arc::new(prompt.clone()),
        "",
        &TestStore::new().vfs(),
        shared,
        &test_context(EXECUTION),
    );
    let error = TokioDriver::new(&ctx, RunHost::new(), None)
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
async fn call_from_h1_runs_the_target_as_a_contained_chain() {
    // The control stubs are gone: H1 is section 0, so `call` resolves
    // against the top-level sections exactly as in any section.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nvar.answer = call('## Answer')\n```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Answer\n\n\
        ```lua\nreturn 'called from h1'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("call from H1 runs the target section");

    assert_eq!(out, "called from h1");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_from_h1_and_a_call_from_the_first_walked_section_take_consecutive_child_ids() {
    // The H1 pass and the walk that follows it are one root chain, so the
    // hand-off copies the pass's child counter into the walk: a `call`
    // the pass made is child `0.0`, and the walk's first `call` is child
    // `0.1`, not a second `0.0`. Were the counter copy dropped at the
    // hand-off, both calls would read `0.0.0`. The walk's own entry
    // counter continues too: its first section is still `0.1`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nvar.first = call('## Answer')\n```\n\n\
        ## Result\n\n\
        ```lua\n\
        assert(sys.id == '0.1', 'the first walked section takes root entry 1')\n\
        return var.first .. ',' .. call('## Answer')\n\
        ```\n\n\
        ## Answer\n\n\
        ```lua\nreturn sys.id\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a call from H1 and a call from the walk both complete");

    assert_eq!(
        out, "0.0.0,0.1.0",
        "the H1 call is root child 0 and the walk's call is root child 1"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn call_from_h1_to_an_unknown_section_is_a_catchable_error() {
    // A `call` naming no visible section fails as the call's answer: the
    // shim raises it at the call site, where an author `pcall` catches it;
    // uncaught, it ends the run as the H1 gate failure.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\nlocal ok, err = pcall(call, '## Nope'); return tostring(ok) .. ':' .. tostring(err)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the caught call failure is the run's result");

    assert!(
        out.starts_with("false:") && out.contains("## Nope"),
        "the caught error names the missing section: {out}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_from_h1_starts_the_walk_at_the_target() {
    // A jump out of H1 ends the pass and starts the walk at the resolved
    // top-level target, skipping the sections before it.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\njump('## Target')\n```\n\n\
        ## Skipped\n\n\
        ```lua\nerror('the jump target must skip this section')\n```\n\n\
        ## Target\n\n\
        ```lua\nreturn 'jumped'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("jump from H1 starts the walk at the target");

    assert_eq!(out, "jumped");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_from_h1_to_an_unknown_section_fails_the_run() {
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\njump('## Nope')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("jump from H1 to an unknown section must fail");

    assert!(
        error.to_string().contains("## Nope"),
        "the failure names the missing section: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_from_h1_runs_the_worker_over_the_collection() {
    // `fanout` works in H1 as in any section: the worker resolves against
    // the top-level sections and the arms join in collection order.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\n\
        local r = fanout('## Worker', {'a', 'b'})\n\
        var.answer = r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Worker\n\n\
        ```lua\nreturn 'item:' .. item\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("fanout from H1 joins the arms");

    assert_eq!(out, "item:a|item:b");
}

#[tokio::test(flavor = "current_thread")]
async fn the_h1_decision_tool_idiom_runs_before_the_walk() {
    // The decision-tool idiom in H1: a local tool with an enum parameter is
    // the verdict channel - the model's loop call lands in the Lua handler,
    // and the captured verdict drives the run's shape before the walk. This
    // needs `tools.add_local` and `models.loop` in H1, both section-only
    // before the one-install-path consolidation.
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_1", "decide", "{\"choice\":\"use_mcp\"}"),
        resp_text("decided"),
    ])
    .await;
    let md = "---\nname: h1-decision\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Decide\n\n\
        ```lua\n\
        models.default('writer')\n\
        tools.add_local('decide', 'Record the verdict', { choice = 'string' }, function(args)\n\
          var.verdict = args.choice\n\
          return 'recorded'\n\
        end)\n\
        local msgs = messages.new()\n\
        msgs:user('interpret the guidance')\n\
        models.loop(msgs)\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.verdict\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the H1 decision-tool idiom runs");

    assert_eq!(out, "use_mcp");
    assert_eq!(
        gateway.call_count(),
        2,
        "the loop runs the tool-call round and the terminal text round"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn list_from_section_works_on_the_h1() {
    // `list_from_section` resolves over H1's visible set - the whole
    // top-level slice.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Test prompt\n\n\
        ```lua\n\
        local items = list_from_section('## Items')\n\
        var.answer = table.concat(items, ',')\n\
        ```\n\n\
        ## Result\n\n\
        ```lua\nreturn var.answer\n```\n\n\
        ## Items\n\n\
        - one\n\
        - two\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("list_from_section from H1 reads the target's items");

    assert_eq!(out, "one,two");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_return() {
    // Mirror of the legacy case of the same name: an H1-only prompt's
    // scalar return is the run's result.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ```lua\nreturn \"hello\"\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the H1-only return runs");

    assert_eq!(out, "hello");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_only_lua_no_return() {
    // Mirror of the legacy case of the same name: an H1-only prompt that
    // produces nothing ends in the shared generic completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Title\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the H1-only fall-through runs");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_scalar_return_short_circuits_the_walk() {
    // The short-circuit half of the H1 return rule: a scalar return from
    // the live H1 pass ends the whole run, so no section ever runs - the
    // walk's erroring section is the tripwire.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Short Circuit\n\n\
        ```lua\nreturn 'early'\n```\n\n\
        ## Never\n\n\
        ```lua\nerror('the walk must not start after an H1 return')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the H1 return short-circuits the run");

    assert_eq!(out, "early");
}

#[tokio::test(flavor = "current_thread")]
async fn h1_prose_inferred_explicitly_is_the_run_result() {
    // An H1-only prompt whose Lua reads its pending buffer into an explicit
    // infer ends the run with the inferred text: the scalar return
    // short-circuits the (empty) walk.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
        # Only Prose\n\n\
        ```lua\n\
        models.default('writer')\n\
        ```\n\n\
        say something\n\n\
        ```lua\n\
        return models.infer(prose)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the H1 infer of its prose ends the run");

    assert_eq!(out, "h1 reply");
    assert_eq!(gateway.call_count(), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn h1_and_h2_prose_each_infer_explicitly_in_source_order() {
    // Mirror of the legacy
    // `h1_and_h2_prose_both_run_through_the_shared_block_loop`: the live H1
    // pass and the H2 section each read their own pending buffer into an
    // explicit infer - two completions, in source order.
    let gateway = ScriptedGateway::start(vec![resp_text("h1 reply"), resp_text("h2 reply")]).await;
    let md = "---\nname: shared-loop\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
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
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("H1 prose and H2 prose each infer explicitly");

    assert_eq!(out, "h2 reply");
    assert_eq!(
        gateway.call_count(),
        2,
        "the H1 prose and the H2 prose each drive exactly one completion"
    );
    let requests = gateway.requests();
    let first_prose = requests[0]["messages"][0]["content"]
        .as_str()
        .expect("the first request includes a user message");
    let second_prose = requests[1]["messages"][0]["content"]
        .as_str()
        .expect("the second request includes a user message");
    assert!(
        first_prose.contains("h1 prose turn"),
        "the first completion is the H1 prose: {first_prose}"
    );
    assert!(
        second_prose.contains("h2 prose turn"),
        "the second completion is the H2 prose: {second_prose}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn unread_h1_prose_stays_inert_and_explicit_infer_requires_a_model() {
    // Mirror of the legacy
    // `live_h1_substitutes_and_skips_empty_prose_before_requiring_a_model`:
    // H1 prose no longer drives inference, so an unread buffer - even one
    // whose substitution would fail or stay empty - discards at the pass's
    // end without requiring a model. Only an explicit `models.infer` of the
    // prose requires a binding.
    let unread = "---\nname: empty-h1\ndescription: d\npromptforge: 0\n---\n\n\
        # Empty H1\n\n\
        ```lua\nvar.omit = ''\n```\n\n\
        {{ var.omit }}\n\n\
        ## Result\n\n\
        ```lua\nreturn 'ok'\n```\n";
    let prompt = parse(unread);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("unread H1 prose must not require a model");
    assert_eq!(out, "ok");

    let reading = "---\nname: read-h1\ndescription: d\npromptforge: 0\n---\n\n\
        # Read H1\n\n\
        ask\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(reading);
    let (ctx, host) = h1_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("an explicit infer of H1 prose with no binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn live_h1_prose_infers_explicitly_and_var_accumulates_into_the_walk() {
    // Mirror of the var half of the legacy
    // `live_h1_prose_preserves_non_final_and_final_semantics_and_captures_var`:
    // the pass reads its pending buffer only through an explicit infer, and
    // `var` writes accumulate across the pass into the walk.
    let gateway = ScriptedGateway::start(vec![resp_text("final answer")]).await;
    let md = "---\nname: live-h1-prose\ndescription: d\npromptforge: 0\nmodels:\n  writer: {}\n---\n\n\
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
    let prompt = parse(md);
    let (ctx, host) = h1_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("live H1 prose infers explicitly");

    assert_eq!(out, "final answer:2");
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
    let (ctx, host) = h1_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, host, None)
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
