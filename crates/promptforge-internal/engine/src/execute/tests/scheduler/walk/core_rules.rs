//! The core walk rules: fall-through order, the generic result, `sys.id`
//! per section entry, `call` chains over off-walk siblings, `var` across
//! fall-through and into calls, and the section boundaries fall-through
//! fires.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn sections_run_in_fall_through_order() {
    // A section without a return falls through to the next section in
    // document order, as the order log shows.
    let store = TestStore::new();
    let md = "---\nname: walk\ndescription: d\npromptforge: 0\n---\n\n\
        # Walk\n\n\
        ## First\n\n\
        ```lua\nstore.append('order.txt', 'First\\n')\n```\n\n\
        ## Second\n\n\
        ```lua\nstore.append('order.txt', 'Second\\n')\nreturn store.read('order.txt')\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the walk falls through in document order");

    assert_eq!(out, "First\nSecond\n");
}

#[tokio::test(flavor = "current_thread")]
async fn generic_result_when_nothing_produced() {
    // A walk that exhausts its slice with no reply yields the shared
    // generic completion.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Generic\n\n\
        ## Only\n\n\
        ```lua\nlocal x = 1\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the empty walk completes");

    assert_eq!(out, "done");
}

#[tokio::test(flavor = "current_thread")]
async fn sys_id_increments_per_section() {
    // Every section entry takes the walk chain's next entry id (`0.N`;
    // entry 0 is the H1 pass, present or not).
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Ids\n\n\
        ## First\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Second\n\n\
        ```lua\nreturn tostring(sys.id)\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("each section entry takes the next id");

    assert_eq!(out, "0.2");
}

#[tokio::test(flavor = "current_thread")]
async fn call_chain_over_off_walk_siblings_returns_to_the_caller() {
    // A executes the off-walk S1, which runs because it is addressed; the
    // chain falls through to S2, and S2's reply returns to A. The main walk
    // ends at B and never runs S1 or S2.
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
    let (ctx, fixture) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the chain must run the addressed off-walk target and fall through");

    assert_eq!(out, "S1\nS2\nA:s2-reply\nB\n");
}

#[tokio::test(flavor = "current_thread")]
async fn var_persists_across_sections_in_fall_through() {
    // One section's `var` writes reach the next across fall-through.
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
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("var must persist across the walk");

    assert_eq!(out, "ab");
}

#[tokio::test(flavor = "current_thread")]
async fn call_clones_var_in_and_discards_child_writes() {
    // `call` clones the caller's `var` in; the contained chain reads the
    // clone, and its writes are discarded when the chain ends.
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
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("call must clone var in and discard child writes");

    assert_eq!(out, "ok");
}

#[tokio::test(flavor = "current_thread")]
async fn a_call_chain_counts_its_own_entries_and_the_outer_walk_resumes_its_own_sequence() {
    // The contained chain is the walk's first child `0.0`, so its entries
    // are `0.0.N`, and the outer walk resumes its own `0.N` sequence when
    // the chain ends.
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
    let (ctx, fixture) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a call chain must take ids nested under its own chain");

    assert_eq!(out, "tail-reply\n");
}

#[tokio::test(flavor = "current_thread")]
async fn entering_the_same_section_twice_takes_two_ids() {
    // Entering the same section twice hands out two distinct `sys.id`
    // values - two call children of the walk, so two chains `0.0` and
    // `0.1`.
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
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("re-entering a section must take a fresh id");

    assert_eq!(out, "0.0.0,0.1.0");
}

#[tokio::test(flavor = "current_thread")]
async fn fall_through_fires_section_finished_before_the_next_section_starts() {
    // Each entered section's armed frame drop fires SECTION_FINISHED at the
    // fall-through, before the next section's SECTION_STARTED.
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Boundaries\n\n\
        ## One\n\n\
        ```lua\nlocal x = 1\n```\n\n\
        ## Two\n\n\
        ```lua\nreturn 'two-ran'\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context_on(&prompt, &TestStore::new(), recorder.clone());
    let out = TokioDriver::new(&ctx, fixture, None)
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
