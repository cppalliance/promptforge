//! How the driver schedules a fanout's arm chains: results in collection
//! order whatever the finish order, arms interleaved at I/O points on one
//! thread, the admission limit, the arms' child ids, and a large
//! collection admitted under the ceiling.

use std::num::NonZeroUsize;

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn fanout_results_follow_collection_order_not_finish_order() {
    // Arm "two" finishes first (one infer) while arm "one" is still parked
    // on its second; the packed sequence must follow collection order. A
    // join that keyed results by completion order would return "r2|r1:r3".
    let gateway =
        ScriptedGateway::start(vec![resp_text("r1"), resp_text("r2"), resp_text("r3")]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local first = models.infer(item .. ':1')\n\
        if item == 'one' then\n\
          return first .. ':' .. models.infer('one:2')\n\
        end\n\
        return first\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "r1:r3|r2");
    assert_eq!(
        request_prompts(&gateway),
        vec!["one:1", "two:1", "one:2"],
        "both arms start before either completes, and arm one finishes last"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_arms_interleave_at_io_points_on_one_thread() {
    // The interleaving proof: both arms reach their first infer before
    // either resumes, so the gateway logs `one:1` then `two:1` - arms
    // driven sequentially would log `one:1`, `one:2` first. Each arm
    // returns its first answer, so the result is deterministic regardless
    // of which arm's second answer lands first.
    let gateway = ScriptedGateway::start(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'one', 'two'})\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':1')\n\
        local b = models.infer(item .. ':2')\n\
        return a\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "r1|r2");
    let prompts = request_prompts(&gateway);
    assert_eq!(prompts.len(), 4, "both arms run both infers: {prompts:?}");
    assert_eq!(
        prompts[..2],
        ["one:1", "two:1"],
        "the second arm reaches I/O before the first arm resumes: {prompts:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_admission_limit_gates_the_arms_a_fanout_runs_at_once() {
    // With the run's concurrency ceiling at 1, the first arm runs both of its
    // infers and ends before the second is admitted - admission that let
    // arms overlap would interleave the requests (x:a, y:a, ...). The
    // fanout spawns every arm up front, so the later arms wait queued,
    // holding no Lua VM, until the ceiling admits them.
    let gateway = ScriptedGateway::start(vec![
        resp_text("r1"),
        resp_text("r2"),
        resp_text("r3"),
        resp_text("r4"),
        resp_text("r5"),
        resp_text("r6"),
    ])
    .await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'x', 'y', 'z'})\n\
        return r[1].text .. '|' .. r[2].text .. '|' .. r[3].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        local a = models.infer(item .. ':a')\n\
        local b = models.infer(item .. ':b')\n\
        return a .. b\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_with_limits(
        &prompt,
        RunLimits::new().max_concurrency(NonZeroUsize::new(1).expect("1 is non-zero")),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the ceilinged fanout completes on the scheduler");

    assert_eq!(out, "r1r2|r3r4|r5r6");
    assert_eq!(
        request_prompts(&gateway),
        vec!["x:a", "x:b", "y:a", "y:b", "z:a", "z:b"],
        "with the ceiling at 1 each arm finishes before the next is admitted"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_arms_take_child_ids_in_collection_order_per_fanout_index_and_structured_results() {
    // Each arm is a child chain of the caller (`0.0`, `0.1`) whose worker
    // entry is `0.K.0`, `sys.index` is the
    // 1-based per-fanout position, and the packed sequence holds `.ok`
    // and `.item` with `__tostring` driving `table.concat`. The ids log is
    // arm-scoped (the pattern happens-before teaches): two arms appending
    // one path are unordered and always boom; the parent's post-join read
    // merges the arm logs in order, because the fanout's join_any rounds
    // join every arm before it returns.
    let store = TestStore::new();
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'a', 'b'})\n\
        assert(r[1].ok and r[2].ok, 'both arms succeed')\n\
        assert(r[2].item == 'b', 'the result object holds the item')\n\
        store.append('ids.txt', 'parent:' .. sys.id .. '\\n')\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('ids-' .. sys.index .. '.txt', sys.id .. ':' .. sys.index .. '\\n')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the fanout completes on the scheduler");

    assert_eq!(out, "a,b");
    assert_eq!(
        store.read("ids-1.txt").expect("arm 1's ids log"),
        "0.0.0:1\n",
        "arm 1 is the caller's child 0 with its per-fanout index"
    );
    assert_eq!(
        store.read("ids-2.txt").expect("arm 2's ids log"),
        "0.1.0:2\n",
        "arm 2 is the caller's child 1 with its per-fanout index"
    );
    assert_eq!(
        store.read("ids.txt").expect("the parent's ids log"),
        "parent:0.1\n",
        "the parent keeps the walk's first entry id"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_over_a_large_collection_admits_arms_under_the_ceiling() {
    // A 1025-member collection runs to completion under the 8-wide default
    // ceiling - every arm spawns up front and the admission queue feeds
    // them in as slots free. Each arm parks on one model round, so its Lua
    // VM stays live from its admission to its end; the test-support tally
    // of live section VMs must never exceed the ceiling's arms plus the
    // parent. The completion assertion alone cannot see a regression that
    // builds a VM per queued arm at spawn - all 1025 would complete
    // regardless - but the tally's peak sees it, because every VM would be
    // live at once.
    let gateway = ScriptedGateway::start(vec![resp_text("r"); 1025]).await;
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local items = {}\n\
        for i = 1, 1025 do items[i] = tostring(i) end\n\
        local r = fanout('### Worker', items)\n\
        return #r .. ':' .. r[1].text .. ':' .. r[1025].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        models.infer(item)\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    promptforge_lua::reset_section_vm_peak();
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("a collection over the ceiling width completes");

    assert_eq!(out, "1025:1:1025");
    let ceiling = RunLimits::new().concurrency().get();
    let peak = promptforge_lua::section_vm_peak();
    assert!(
        (2..=ceiling + 1).contains(&peak),
        "between the parent plus one admitted arm and the ceiling's arms plus the parent hold \
         a Lua VM at once: peak {peak}"
    );
}
