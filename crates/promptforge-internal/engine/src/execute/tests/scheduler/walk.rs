//! Walk, call, jump, cancel, and depth tests for the scheduler.

use super::*;
use crate::test_support::tokio_driver::TokioDriver;

#[tokio::test(flavor = "current_thread")]
async fn nested_call_and_inference_run_end_to_end_on_a_current_thread_runtime() {
    // THE DECISION GATE: under the legacy bridge this prompt fails with
    // `Error::Internal` on a current-thread runtime; under the scheduler
    // the nested call and both infers complete on the one thread.
    let gateway =
        ScriptedGateway::start(vec![resp_text("inner answer"), resp_text("outer answer")]).await;
    let md = "---\nname: gate\ndescription: d\npromptforge: 0\n---\n\n\
        # Gate\n\n\
        ## Outer\n\n\
        ```lua\n\
        local inner = call('## Inner')\n\
        return models.infer('outer saw: ' .. inner)\n\
        ```\n\n\
        ## Inner\n\n\
        ```lua\n\
        return models.infer('inner ask')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the gate scenario runs end to end on one thread");

    assert_eq!(out, "outer answer");
    assert_eq!(
        gateway.call_count(),
        2,
        "the child's infer and the parent's infer each drive one completion"
    );
    let requests = gateway.requests();
    assert_eq!(
        requests[0]["messages"][0]["content"].as_str(),
        Some("inner ask"),
        "the contained chain's infer runs first: {requests:?}"
    );
    assert_eq!(
        requests[1]["messages"][0]["content"].as_str(),
        Some("outer saw: inner answer"),
        "the parent resumes with the contained chain's final text: {requests:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn cancellation_while_suspended_on_infer_interrupts_the_run() {
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "too late",
        std::time::Duration::from_secs(30),
    )])
    .await;
    let md = "---\nname: cancel\ndescription: d\npromptforge: 0\n---\n\n\
        # Cancel\n\n\
        ## Only\n\n\
        ```lua\nreturn models.infer('hang')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let mut driver = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())));
    let canceller = driver.cancel_handle();
    let calls = Arc::clone(&gateway.calls);
    tokio::spawn(async move {
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            while calls.load(Ordering::SeqCst) == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        canceller.cancel();
    });

    let result = driver.drive().await;

    assert!(
        matches!(result, Err(Error::Interrupted)),
        "cancelling a suspended infer must interrupt the run, got {result:?}"
    );
    assert_eq!(
        gateway.call_count(),
        1,
        "the cancellation must occur after infer reached the gateway"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn call_depth_cap_reads_the_chain_field() {
    // Two sections calling each other ping-pong down the chain stack; the
    // cap must fire from the requesting chain's call-depth field. The
    // typed error then round-trips through every parent's answer envelope
    // without flattening.
    let md = "---\nname: depth\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\nreturn call('## Beta')\n```\n\n\
        ## Beta\n\n\
        ```lua\nreturn call('## Alpha')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "call recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_lua_infer_of_prose_uses_the_run_configured_client() {
    // The chain's client slot is seeded from the run's configured client, so
    // a section's explicit `models.infer(prose)` reaches that gateway rather
    // than falling back to an environment client; the returned text becomes
    // the run's result.
    let gateway = ScriptedGateway::start(vec![resp_text("prose answer")]).await;
    let md = "---\nname: prose\ndescription: d\npromptforge: 0\n---\n\n\
        # Prose\n\n\
        ## Only\n\n\
        Say something.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an explicit infer of the prose runs through the scheduler");

    assert_eq!(out, "prose answer");
    assert_eq!(gateway.call_count(), 1, "the infer drives one completion");
    let requests = gateway.requests();
    let content = requests[0]["messages"][0]["content"]
        .as_str()
        .unwrap_or_default();
    assert!(
        content.contains("Say something."),
        "the prose text reaches the gateway: {requests:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_dispatch_failure_resumes_through_the_envelope_into_pcall() {
    // A failed dispatch (here an unresolvable call target) is the call's
    // answer resumed through the error envelope, so an author `pcall`
    // catches it exactly as on the legacy callback path; a driver that
    // failed the chain instead would error the run.
    let md = "---\nname: catch\ndescription: d\npromptforge: 0\n---\n\n\
        # Catch\n\n\
        ## Only\n\n\
        ```lua\n\
        local ok, err = pcall(call, '## Missing')\n\
        if ok then return 'uncaught' end\n\
        return 'caught: ' .. tostring(err)\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the dispatch failure is catchable");

    assert!(
        out.starts_with("caught: "),
        "the pcall catches the dispatch failure, got {out:?}"
    );
    assert!(
        out.contains("not found"),
        "the caught error is the target resolution failure, got {out:?}"
    );
}

// --- Walk translation: the core rules mirrored from the legacy suite ---
// Each test names the legacy case it mirrors. The legacy cases keep
// exercising the legacy engine untouched; these prove the scheduler.

#[tokio::test(flavor = "current_thread")]
async fn sections_run_in_fall_through_order() {
    // Mirror of the legacy `falls_through_to_next_section`, strengthened
    // with an order log: a section without a return falls through to the
    // next section in document order.
    let store = TestStore::new();
    let md = "---\nname: walk\ndescription: d\npromptforge: 0\n---\n\n\
        # Walk\n\n\
        ## First\n\n\
        ```lua\nstore.append('order.txt', 'First\\n')\n```\n\n\
        ## Second\n\n\
        ```lua\nstore.append('order.txt', 'Second\\n')\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the walk falls through in document order");

    assert_eq!(out, "First\nSecond\n");
}

#[tokio::test(flavor = "current_thread")]
async fn generic_result_when_nothing_produced() {
    // Mirror of the legacy `generic_result_when_nothing_produced`: a walk
    // that exhausts its slice with no reply yields the shared generic
    // completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Generic\n\n\
        ## Only\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the empty walk completes");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_increments_per_section() {
    // Mirror of the legacy `sys_id_increments_per_section`: every section
    // entry takes the walk chain's next entry id (`0.N`; entry 0 is the
    // H1 pass, present or not).
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Ids\n\n\
        ## First\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Second\n\n\
        ```lua\nreturn tostring(sys.id)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("each section entry takes the next id");

    assert_eq!(out, "0.2");
}

#[tokio::test(flavor = "current_thread")]
async fn call_chain_over_off_walk_siblings_returns_to_the_caller() {
    // Mirror of the legacy case of the same name: A executes the off-walk
    // S1, which runs because it is addressed; the chain falls through to
    // S2, and S2's reply returns to A. The main walk ends at B and never
    // runs S1 or S2.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Siblings\n\n\
        ## A\n\n\
        ```lua\n\
        local r = call('## S1')\n\
        store.append('order.txt', 'A:' .. r .. '\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## S1\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'S1\\n')\n```\n\n\
        ## S2\n\n\
        ```lua\n\
        store.append('order.txt', 'S2\\n')\n\
        return 's2-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the chain must run the addressed off-walk target and fall through");

    assert_eq!(out, "S1\nS2\nA:s2-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_sections_in_fall_through() {
    // Mirror of the fall-through half of the legacy
    // `var_persists_across_sections_fallthrough_and_jump` (its jump half
    // lands with the jump translation): one section's `var` writes reach
    // the next across fall-through.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Var\n\n\
        ## A\n\n\
        ```lua\nvar.from_a = 'a'\n```\n\n\
        ## B\n\n\
        ```lua\n\
        assert(var.from_a == 'a', 'fall-through keeps the walk var')\n\
        var.from_b = 'b'\n\
        ```\n\n\
        ## C\n\n\
        ```lua\nreturn var.from_a .. var.from_b\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("var must persist across the walk");

    assert_eq!(out, "ab");
}

#[tokio::test(flavor = "current_thread")]
async fn call_clones_var_in_and_discards_child_writes() {
    // Mirror of the legacy case of the same name: `call` clones the
    // caller's `var` in; the contained chain reads the clone, and its
    // writes are discarded when the chain ends.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Clone\n\n\
        ## Main\n\n\
        ```lua\n\
        var.shared = 'caller'\n\
        local r = call('## Sub')\n\
        assert(r == 'sub saw caller', 'the child reads the cloned var')\n\
        assert(var.child_write == nil, 'child writes must not reach the caller')\n\
        return 'ok'\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        var.child_write = 'sub'\n\
        return 'sub saw ' .. var.shared\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("call must clone var in and discard child writes");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_chain_counts_its_own_entries_and_the_outer_walk_resumes_its_own_sequence() {
    // Mirror of the legacy case of the same name: the contained chain is
    // the walk's first child `0.0`, so its entries are `0.0.N`, and the
    // outer walk resumes its own `0.N` sequence when the chain ends.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Sequence\n\n\
        ## Main\n\n\
        ```lua\n\
        assert(sys.id == '0.1', 'the first walked section takes entry 1 of the root chain')\n\
        local r = call('## Sub')\n\
        store.append('order.txt', r .. '\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        assert(sys.id == '0.2', 'the outer walk resumes its own sequence')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        assert(sys.id == '0.0.0', 'the contained chain is child 0 and starts at entry 0')\n\
        ```\n\n\
        ## Tail\n\n\
        ```lua\n\
        assert(sys.id == '0.0.1', 'the chain fall-through takes its next entry')\n\
        return 'tail-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a call chain must take ids nested under its own chain");

    assert_eq!(out, "tail-reply\n");
}

#[tokio::test(flavor = "current_thread")]
async fn entering_the_same_section_twice_takes_two_ids() {
    // Mirror of the legacy case of the same name: entering the same
    // section twice hands out two distinct `sys.id` values - two call
    // children of the walk, so two chains `0.0` and `0.1`.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Twice\n\n\
        ## Main\n\n\
        ```lua\n\
        local a = call('## Sub')\n\
        local b = call('## Sub')\n\
        return a .. ',' .. b\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\nreturn tostring(sys.id)\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("re-entering a section must take a fresh id");

    assert_eq!(out, "0.0.0,0.1.0");
}

#[tokio::test(flavor = "current_thread")]
async fn fall_through_fires_section_finished_before_the_next_section_starts() {
    // The boundary half of the legacy
    // `a_two_section_run_reports_the_exact_observation_sequence`: each
    // entered section's armed frame drop fires SECTION_FINISHED at the
    // fall-through, before the next section's SECTION_STARTED.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Boundaries\n\n\
        ## One\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Two\n\n\
        ```lua\nreturn 'two-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the walk completes both sections");

    assert_eq!(out, "two-ran");
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    let boundaries: Vec<(String, String)> = recorder
        .events()
        .into_iter()
        .filter(|(_, event)| event == &started || event == &finished)
        .collect();
    assert_eq!(
        boundaries,
        vec![
            ("One".to_owned(), started.clone()),
            ("One".to_owned(), finished.clone()),
            ("Two".to_owned(), started.clone()),
            ("Two".to_owned(), finished.clone()),
        ]
    );
}

// --- Walk translation: jumps, returns, and observation boundaries ---
// Each test names the legacy case it mirrors. The legacy cases keep
// exercising the legacy engine untouched; these prove the scheduler.

#[tokio::test(flavor = "current_thread")]
async fn jump_transfer_skips_the_jumpers_remaining_blocks() {
    // Mirror of the legacy
    // `jump_target_sees_no_prior_reply_and_transfer_skips_remaining_blocks`:
    // the jump transfers control and the jumper's remaining blocks never
    // run.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Jump\n\n\
        ## Check\n\n\
        ```lua\n\
        store.write('seen.txt', 'check')\n\
        jump('## Help')\n\
        store.write('seen.txt', 'should-not-run')\n\
        ```\n\n\
        ## Accept\n\n\
        ```lua\nreturn 'accepted'\n```\n\n\
        ## Help\n\n\
        ```lua\n\
        return 'helped:' .. store.read('seen.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("jump must transfer control");

    assert_eq!(out, "helped:check");
    assert_eq!(store.read("seen.txt").expect("seen"), "check");
}

#[tokio::test(flavor = "current_thread")]
async fn section_cannot_jump_to_itself() {
    // Mirror of the legacy case of the same name: the caller is outside its
    // own visible set, so naming its own heading to `jump` resolves as
    // not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Self\n\n\
        ## Self\n\n\
        ```lua\njump('## Self')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("self-jump must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "the caller is not in its own visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_off_walk_section_runs_it() {
    // Mirror of the legacy case of the same name: a jump addresses an
    // off-walk section directly, so it runs.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Addressed\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ---\n\n\
        ```lua\nreturn 'b-ran'\n```\n\n\
        ## C\n\n\
        ```lua\nreturn 'c-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump to an off-walk section must run it");

    assert_eq!(out, "b-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_a_jump() {
    // The jump half of the legacy
    // `var_persists_across_sections_fallthrough_and_jump` (its H1-seed half
    // has no scheduler counterpart - the scheduler's drive starts at the
    // walk): the jumper's `var` writes cross the transfer, and the target's
    // writes roll forward into the fall-through that follows.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Var\n\n\
        ## A\n\n\
        ```lua\n\
        var.from_a = 'a'\n\
        jump('## C')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\nerror('the jump must skip B')\n```\n\n\
        ## C\n\n\
        ```lua\n\
        assert(var.from_a == 'a', 'the jump keeps the jumper writes')\n\
        var.from_c = 'c'\n\
        ```\n\n\
        ## D\n\n\
        ```lua\n\
        assert(var.from_c == 'c', 'fall-through after the jumped target keeps var')\n\
        return var.from_a .. var.from_c\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("var must persist across the jump");

    assert_eq!(out, "ac");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_fires_section_finished_for_the_jumper_before_the_target_starts() {
    // The jump half of the observation-boundary contract (the fall-through
    // half is `fall_through_fires_section_finished_before_the_next_section_starts`
    // above): a jump is a completion, so the jumper's armed frame drop
    // fires SECTION_FINISHED before the target's SECTION_STARTED.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Boundaries\n\n\
        ## A\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the jump completes both sections");

    assert_eq!(out, "b-ran");
    let started = detail::SECTION_STARTED.to_string();
    let finished = detail::SECTION_FINISHED.to_string();
    let boundaries: Vec<(String, String)> = recorder
        .events()
        .into_iter()
        .filter(|(_, event)| event == &started || event == &finished)
        .collect();
    assert_eq!(
        boundaries,
        vec![
            ("A".to_owned(), started.clone()),
            ("A".to_owned(), finished.clone()),
            ("B".to_owned(), started.clone()),
            ("B".to_owned(), finished.clone()),
        ]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_erroring_section_reports_started_but_not_finished() {
    // Mirror of the legacy case of the same name: a section that errors
    // mid-walk emits SECTION_STARTED and never SECTION_FINISHED - the
    // frame's drop stays unarmed on the error path.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fail\n\n\
        ## Only\n\n\
        ```lua\nerror('expected failure')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = TokioDriver::new(&ctx, host, None).drive().await;

    assert!(result.is_err());
    let observed = recorder.events();
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

#[tokio::test(flavor = "current_thread")]
async fn jump_to_a_child_starts_the_child_level_walk() {
    // Mirror of the legacy case of the same name: a jump to an H3 child
    // starts a child-level walk at the target, which falls through to the
    // target's following siblings; when the level exhausts, the parent walk
    // resumes after the jumper.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Descend\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump to a child must start the child-level walk");

    assert_eq!(out, "A\nX\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn child_walk_recurses_to_h4() {
    // Mirror of the legacy case of the same name: the child-level rule
    // recurses - a jump from an H3 child to an H4 grandchild starts an
    // H4-level walk, and each level's exhaustion resumes its parent after
    // the jumper.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Recurse\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('order.txt', 'X\\n')\n\
        jump('#### P')\n\
        ```\n\n\
        #### P\n\n\
        ```lua\nstore.append('order.txt', 'P\\n')\n```\n\n\
        #### Q\n\n\
        ```lua\nstore.append('order.txt', 'Q\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the child-level rule must recurse to H4");

    assert_eq!(out, "A\nX\nP\nQ\nY\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_an_off_walk_child_runs_it() {
    // Mirror of the legacy case of the same name: an off-walk child stays
    // addressable - a jump to it runs it, and the fall-through that follows
    // skips nothing addressed.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # OffChild\n\n\
        ## A\n\n\
        ```lua\njump('### Off')\n```\n\n\
        ### X\n\n\
        ```lua\nstore.append('order.txt', 'X\\n')\n```\n\n\
        ### Off\n\n\
        ---\n\n\
        ```lua\nstore.append('order.txt', 'Off\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump to an off-walk child must run it");

    assert_eq!(out, "Off\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_addresses_its_own_siblings_and_children() {
    // Mirror of the legacy case of the same name: a running child's visible
    // set is its own siblings plus its own children - it can execute a
    // child and jump to a sibling.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Visible\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        local r = call('#### Grand')\n\
        store.append('order.txt', 'X:' .. r .. '\\n')\n\
        jump('### Y')\n\
        ```\n\n\
        #### Grand\n\n\
        ```lua\nreturn 'grand-ran'\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('order.txt', 'Y\\n')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a running child must address its own siblings and children");

    assert_eq!(out, "X:grand-ran\nY\n");
}

#[tokio::test(flavor = "current_thread")]
async fn running_child_cannot_address_a_top_level_section() {
    // Mirror of the legacy case of the same name: a running child cannot
    // address a top-level section - the parent level is not in its visible
    // set, so the jump resolves as not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Escape\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\njump('## B')\n```\n\n\
        ## B\n\n\
        ```lua\nreturn 'b-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a child jumping to a top-level section must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a top-level section is not in a child's visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn jump_to_a_niece_errors() {
    // Mirror of the legacy case of the same name: a sibling's child (a
    // niece or nephew) is not in the visible set, so the jump resolves as
    // not-found.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Niece\n\n\
        ## A\n\n\
        ```lua\njump('### Niece')\n```\n\n\
        ## B\n\n\
        ### Niece\n\n\
        ```lua\nreturn 'niece-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a jump to a niece must fail");

    let rendered = error.to_string();
    assert!(
        rendered.contains("not found"),
        "a niece is not in the visible set: {rendered}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_counts_the_sections_one_chain_enters_across_a_jump_into_a_child_level() {
    // Mirror of the legacy case of the same name: `sys.id` counts the
    // sections the walk chain has entered - the detour into a child level
    // is the same chain, so it continues the count rather than
    // restarting it.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Ids\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        jump('### X')\n\
        ```\n\n\
        ### X\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ### Y\n\n\
        ```lua\nstore.append('ids.txt', tostring(sys.id) .. '\\n')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('ids.txt', tostring(sys.id) .. '\\n')\n\
        return store.read('ids.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("sys.id must count the sections the one chain enters");

    assert_eq!(out, "0.1\n0.2\n0.3\n0.4\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_child_takes_ids_nested_under_its_own_chain_distinct_from_the_parent() {
    // Hierarchical identity: the parent walk is the root chain `0`, so its
    // entries are `0.N`; a `call` child is the root's first child chain
    // `0.0`, so its entries are `0.0.N`. The two never collide because
    // they are different chains, and a fanout inside the child nests its
    // arms under the child's chain id, not the root's.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Identity\n\n\
        ## Main\n\n\
        ```lua\n\
        store.append('ids.txt', 'main:' .. sys.id .. '\\n')\n\
        call('## Sub')\n\
        store.append('ids.txt', 'after:' .. sys.id .. '\\n')\n\
        return store.read('ids.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        store.append('ids.txt', 'sub:' .. sys.id .. '\\n')\n\
        local r = fanout('## Worker', {'a', 'b'})\n\
        store.append('ids.txt', r[1].text .. '\\n' .. r[2].text .. '\\n')\n\
        return 'sub-done'\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        return 'arm' .. sys.index .. ':' .. sys.id\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the call child and its fanout complete");

    assert_eq!(
        out, "main:0.1\nsub:0.0.0\narm1:0.0.0.0\narm2:0.0.1.0\nafter:0.1\n",
        "parent entries are `0.N`, the call child's are `0.0.N`, and the \
         child's fanout arms nest under `0.0`"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_runs_of_the_same_prompt_produce_identical_ids() {
    // No run-global counter: every id is a path of chain-local counters,
    // so two runs of one prompt allocate the same ids for the walk, a call
    // child, a fanout's arms, and a section entered after both.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Identity\n\n\
        ## Main\n\n\
        ```lua\n\
        local ids = { sys.id }\n\
        ids[#ids + 1] = call('## Sub')\n\
        local r = fanout('## Worker', {'x', 'y', 'z'})\n\
        for i = 1, #r do ids[#ids + 1] = r[i].text end\n\
        ids[#ids + 1] = call('## Sub')\n\
        var.ids = table.concat(ids, ',')\n\
        ```\n\n\
        ## Last\n\n\
        ```lua\n\
        return var.ids .. ',' .. sys.id\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        return sys.id\n\
        ```\n\n\
        ## Worker\n\n\
        ```lua\n\
        return sys.id\n\
        ```\n";
    let prompt = parse(md);
    let mut outputs = Vec::new();
    for _ in 0..2 {
        let (ctx, host) = scheduler_context(&prompt);
        let out = TokioDriver::new(&ctx, host, None)
            .drive()
            .await
            .expect("the identity prompt completes");
        outputs.push(out);
    }

    assert_eq!(outputs[0], outputs[1], "two runs allocate the same ids");
    assert_eq!(
        outputs[0], "0.1,0.0.0,0.1.0,0.2.0,0.3.0,0.4.0,0.2",
        "the walk's entries are `0.N`, and call children and fanout arms \
         share the root's child counter in dispatch order"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_child_walk_ends_the_whole_chain() {
    // The rule-5 clause the legacy cases imply but none isolates: a scalar
    // return inside a jump-started child-level walk ends the whole chain,
    // not just the child level - the parent walk never resumes.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Return\n\n\
        ## A\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\nreturn 'x-value'\n```\n\n\
        ## B\n\n\
        ```lua\nerror('the return must end the chain before the parent resumes')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a return in the child walk ends the whole chain");

    assert_eq!(out, "x-value");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_call_is_contained_in_the_chain() {
    // Mirror of the legacy case of the same name: a jump inside `call()`
    // is contained by the chain - followed, not rejected. The chain's index
    // moves to the target, the sections between the jumper and the target
    // do not run, and the target's reply returns to the caller.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Contained\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = call('## Sub')\n\
        return 'main:' .. r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Skipped\n\n\
        ```lua\nerror('the chain jump must move past me')\n```\n\n\
        ## Peer\n\n\
        ```lua\nreturn 'peer-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump inside call must be followed within the chain");

    assert_eq!(out, "main:peer-ran");
}

#[tokio::test(flavor = "current_thread")]
async fn jump_inside_a_call_chain_moves_within_the_chain() {
    // Mirror of the legacy case of the same name: a jump inside a
    // `call()` chain to a sibling moves within the contained chain - the
    // walk continues from the jump target under the normal rules, and the
    // chain's final reply is the call's return value.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Move\n\n\
        ## A\n\n\
        ```lua\n\
        local r = call('## Sub')\n\
        return 'A:' .. r\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Peer\n\n\
        ```lua\nstore.append('order.txt', 'Peer\\n')\n```\n\n\
        ## Tail\n\n\
        ```lua\n\
        store.append('order.txt', 'Tail\\n')\n\
        return 'tail-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a jump inside the chain must move within the chain");

    assert_eq!(out, "A:tail-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Peer\nTail\n");
}

#[tokio::test(flavor = "current_thread")]
async fn call_chain_jumps_to_a_child_and_returns_the_chain_result() {
    // Mirror of the legacy
    // `call_chain_jumps_to_a_child_and_returns_the_chain_reply` (the
    // canonical contained chain): A calls Sub; Sub jumps to its child S1,
    // starting a child-level walk that falls through to S2; S2's return is
    // the chain's final text back to A, and the outer walk continues at B,
    // never having moved.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Chain\n\n\
        ## A\n\n\
        ```lua\n\
        store.append('order.txt', 'A1\\n')\n\
        local r = call('## Sub')\n\
        assert(r == 's2-result', 'the chain final text returns to A')\n\
        store.append('order.txt', 'A2\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\n\
        store.append('order.txt', 'Sub\\n')\n\
        jump('### S1')\n\
        ```\n\n\
        ### S1\n\n\
        ```lua\nstore.append('order.txt', 'S1\\n')\n```\n\n\
        ### S2\n\n\
        ```lua\n\
        store.append('order.txt', 'S2\\n')\n\
        return 's2-result'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the call chain must jump, fall through, and return its final text");

    assert_eq!(out, "A1\nSub\nS1\nS2\nA2\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn the_outer_walk_never_moves_during_a_contained_chain() {
    // Mirror of the legacy case of the same name: the outer walk never
    // moves while a contained chain runs - wherever the chain ends, the
    // outer walk resumes at the section after the caller.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Outer\n\n\
        ## A\n\n\
        ```lua\n\
        call('## Sub')\n\
        store.append('order.txt', 'A-done\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\njump('## Peer')\n```\n\n\
        ## Peer\n\n\
        ```lua\n\
        store.append('order.txt', 'Peer\\n')\n\
        return 'p'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the outer walk must resume at the section after the caller");

    assert_eq!(out, "Peer\nA-done\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_return_inside_a_chain_ends_the_chain_not_the_run() {
    // Mirror of the legacy case of the same name: a return inside a
    // contained chain ends the chain, not the run - the returned value is
    // the call's return, the chain's remaining sections do not run, and
    // the outer walk continues.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Scoped\n\n\
        ## A\n\n\
        ```lua\n\
        local r = call('## Sub')\n\
        store.append('order.txt', 'A:' .. r .. '\\n')\n\
        ```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n\n\
        ## Sub\n\n\
        ```lua\nreturn 'sub-reply'\n```\n\n\
        ## After\n\n\
        ```lua\nerror('a return must end the chain before fall-through')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a return must end the chain, not the run");

    assert_eq!(out, "A:sub-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn call_to_a_child_starts_a_contained_chain() {
    // Mirror of the legacy case of the same name: `call` to a child
    // starts a contained chain at the target - the chain falls through to
    // the target's following siblings under the same rules as any walk, and
    // the chain's final reply is the call's return value.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # ChildExecute\n\n\
        ## Main\n\n\
        ```lua\n\
        local r = call('### Sub')\n\
        return 'got:' .. r\n\
        ```\n\n\
        ### Sub\n\n\
        ```lua\nstore.append('order.txt', 'Sub\\n')\n```\n\n\
        ### After\n\n\
        ```lua\n\
        store.append('order.txt', 'After\\n')\n\
        return 'after-reply'\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("call to a child must start a contained chain");

    assert_eq!(out, "got:after-reply");
    assert_eq!(store.read("order.txt").expect("order log"), "Sub\nAfter\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_descent_does_not_consume_call_depth() {
    // The depth-cap interaction, identical to the legacy engine: a jump
    // descent is not a call, so the child level shares the chain's
    // call-depth field. X and Y ping-pong calls from inside a
    // jump-started child walk; each entry appends once. The cap trips when
    // the ninth nested call would run (depth 9 > 8), after exactly nine
    // section entries - a descent that wrongly consumed depth would trip
    // the cap one entry earlier.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Main\n\n\
        ```lua\njump('### X')\n```\n\n\
        ### X\n\n\
        ```lua\n\
        store.append('depth.txt', 'x\\n')\n\
        return call('### Y')\n\
        ```\n\n\
        ### Y\n\n\
        ```lua\n\
        store.append('depth.txt', 'y\\n')\n\
        return call('### X')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("the depth cap must fail the run");

    match &error {
        Error::Lua(message) => assert_eq!(message, "call recursion exceeded cap of 8"),
        other => panic!("expected the typed depth-cap Lua error, got {other:?}"),
    }
    assert_eq!(
        store.read("depth.txt").expect("depth log"),
        "x\ny\nx\ny\nx\ny\nx\ny\nx\n",
        "the descent shares the chain's call depth: nine entries, then the cap"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn walk_never_descends_into_children() {
    // Mirror of the legacy case of the same name: the walk never descends -
    // a section's children do not run unless addressed. This is the
    // negative half of the child-descent rule: a fall-through that
    // descended would run the child and trip its error.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # NoDescent\n\n\
        ## A\n\n\
        ```lua\nstore.append('order.txt', 'A\\n')\n```\n\n\
        ### Child\n\n\
        ```lua\nerror('a child must not run by fall-through')\n```\n\n\
        ## B\n\n\
        ```lua\n\
        store.append('order.txt', 'B\\n')\n\
        return store.read('order.txt')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the walk must never descend into children");

    assert_eq!(out, "A\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn a_failed_jump_resolution_still_finishes_the_jumper() {
    // The error half of the jump observation boundary: the jumper's frame
    // closes as completed before the heading resolves (the legacy walk
    // resolves after the jumper's teardown), so SECTION_FINISHED fires for
    // the jumper even when the target does not resolve.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Unresolved\n\n\
        ## A\n\n\
        ```lua\njump('## Missing')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let result = TokioDriver::new(&ctx, host, None).drive().await;

    let error = result.expect_err("an unresolvable jump target must fail the run");
    assert!(
        error.to_string().contains("not found"),
        "the failure is the resolution, not the transfer: {error}"
    );
    let observed = recorder.events();
    assert!(
        observed.contains(&("A".to_string(), detail::SECTION_FINISHED.to_string())),
        "the jumper completed before the resolution failed: {observed:?}"
    );
}
