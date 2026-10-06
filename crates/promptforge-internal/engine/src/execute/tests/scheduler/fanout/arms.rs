//! What a fanout arm runs with and hands back: a pre-cancelled fanout, an
//! arm infer with no model binding, the shared replay seeing `item`, jumps
//! from an arm into child walks, a hash collection in sorted key order, and
//! sealed results.

use super::*;

#[tokio::test(flavor = "current_thread")]
async fn pre_cancelled_fanout_returns_interrupted() {
    // A fanout entered under an already-cancelled handle fails the run with
    // Error::Interrupted instead of running the arms.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn item\n```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let mut driver = TokioDriver::new(&ctx, fixture, None);
    driver.cancel_handle().cancel();
    let result = driver.drive().await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "a pre-cancelled fanout must interrupt the run, got {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn model_required_when_arm_infer_has_no_binding() {
    // An arm whose explicit infer of its prose has no model binding fails
    // the fanout with Error::ModelRequired naming the worker section. The
    // context is built directly so the model set stays empty - the shared
    // test context pre-fills a default binding.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        fanout('### Worker', {'alpha'})\n\
        ```\n\n\
        ### Worker\n\n\
        Ask the model about {{ item }}.\n\n\
        ```lua\nreturn models.infer(prose)\n```\n";
    let prompt = parse(md);
    let shared = LuaProgram::empty().expect("the empty chunk compiles");
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
        .expect_err("an arm infer without a model binding must fail");
    assert!(
        matches!(error, Error::ModelRequired { .. }),
        "expected ModelRequired, got {error}"
    );
    assert!(
        error
            .to_string()
            .contains("model binding required for section Worker"),
        "error must name the worker section: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn the_shared_replay_sees_the_arm_item() {
    // The `item` global installs before `replay_shared`, so the shared
    // library's top-level code may read `item`; moving the install after
    // the replay would capture nil in the arm and fail this test. The
    // context holds the prompt's real compiled shared library, not the
    // empty stand-in the other scheduler tests use.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ```lua shared\n\
        captured_by_shared = item\n\
        ```\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        return tostring(captured_by_shared) .. '|' .. tostring(item)\n\
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
    let out = TokioDriver::new(&ctx, RunFixture::new(), None)
        .drive()
        .await
        .expect("the arm must succeed");

    assert_eq!(
        out, "alpha|alpha",
        "the shared chunk captured the item before the worker ran"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_inside_a_fanout_arm_drives_a_child_walk() {
    // The arm's remaining blocks are skipped, the walk continues on the
    // target's own slice from the target (the arm chain's entry sequence
    // continues, the walk falls through to the target's following
    // siblings), and the walk's reply becomes the arm's text. A
    // `resolve_arm_target` that resolved over the wrong set would error
    // the jump not-found; one that started the walk elsewhere would break
    // the order log.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        jump('### Target')\n\
        error('the arm remaining blocks are skipped')\n\
        ```\n\n\
        ### Target\n\n\
        ```lua\n\
        assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')\n\
        store.append('order.txt', 'Target\\n')\n\
        ```\n\n\
        ### Tail\n\n\
        ```lua\n\
        store.append('order.txt', 'Tail\\n')\n\
        return 'tail-reply'\n\
        ```\n";
    let store = TestStore::new();
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a jump inside an arm drives a child walk");

    assert_eq!(out, "tail-reply");
    assert_eq!(
        store.read("order.txt").expect("the order log"),
        "Target\nTail\n",
        "the child walk runs the target and falls through to its siblings"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_jump_from_an_arm_to_a_worker_child_walks_the_child_slice() {
    // The descent runs the worker's child slice from the target, the target
    // takes the arm chain's next entry id with no `item` seed (the transfer
    // clears the arm's at-worker state, so the child walk runs as plain
    // sections), and the walk falls through to the target's child siblings.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        jump('#### Child')\n\
        error('the arm remaining blocks are skipped')\n\
        ```\n\n\
        #### Child\n\n\
        ```lua\n\
        assert(sys.id == '0.0.1', 'the child walk continues the arm chain sys.id sequence')\n\
        assert(item == nil, 'the child walk runs as a plain section')\n\
        store.append('order.txt', 'Child\\n')\n\
        ```\n\n\
        #### ChildTail\n\n\
        ```lua\n\
        store.append('order.txt', 'ChildTail\\n')\n\
        return 'child-tail-reply'\n\
        ```\n";
    let store = TestStore::new();
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a jump to a worker child walks the child slice");

    assert_eq!(out, "child-tail-reply");
    assert_eq!(
        store.read("order.txt").expect("the order log"),
        "Child\nChildTail\n",
        "the child walk runs the target and falls through to its child siblings"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_hash_collection_iterates_in_sorted_key_order() {
    // The hash part of a collection has no Lua-defined order (`pairs` walks
    // the string hash seed's layout, which differs per state), so the shim
    // sorts it by key: the arms take their ids, `sys.index`, and result
    // slots in key order on every run. A shim that walked `pairs` order
    // would place `zeta` first on some runs and fail the fixed expectation.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', { zeta = 1, alpha = 2, mid = 3 })\n\
        assert(r[1].item.key == 'alpha' and r[3].item.key == 'zeta', 'results land by sorted key')\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        return item.key .. '=' .. item.value .. '@' .. sys.index .. ':' .. sys.id\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("a hash-shaped collection fans out");

    assert_eq!(out, "alpha=2@1:0.0.0,mid=3@2:0.1.0,zeta=1@3:0.2.0");
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_results_are_sealed_against_writes_and_metatable_replacement() {
    // The seal on a result object: an assignment raises, `setmetatable`
    // is refused (the guard cannot be swapped out), `getmetatable` hands
    // back a decoy that exposes no `__index` (so the hidden fields table
    // cannot be reached and mutated), and the decoy still exposes
    // `__tostring` so the hardened `table.concat` renders the result. The
    // fields and `tostring` read as before.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        local ok, err = pcall(function() r[1].text = 'forged' end)\n\
        assert(not ok and tostring(err):find('read-only', 1, true), 'a write raises: ' .. tostring(err))\n\
        ok, err = pcall(setmetatable, r[1], nil)\n\
        assert(not ok and tostring(err):find('protected metatable', 1, true), 'setmetatable is refused: ' .. tostring(err))\n\
        ok, err = pcall(setmetatable, r[1], {})\n\
        assert(not ok, 'no replacement metatable is accepted')\n\
        local decoy = getmetatable(r[1])\n\
        assert(type(decoy) == 'table' and decoy.__index == nil and decoy.__newindex == nil, 'the guard is hidden')\n\
        assert(type(decoy.__tostring) == 'function', 'the decoy still renders')\n\
        assert(r[1].text == 'alpha-1' and r[1].ok == true and r[1].item == 'alpha' and r[1].exhausted == false)\n\
        assert(tostring(r[1]) == 'alpha-1')\n\
        return table.concat(r, ',')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        return item .. '-' .. sys.index\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, fixture) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, fixture, None)
        .drive()
        .await
        .expect("the sealed results read and render");

    assert_eq!(out, "alpha-1,beta-2");
}
