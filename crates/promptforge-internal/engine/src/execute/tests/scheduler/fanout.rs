//! Fanout mechanics and failure semantics on the scheduler.

use std::num::NonZeroUsize;

use super::models_loop::{echo_tools, loop_context_observed};
use super::*;
use crate::test_support::tokio_driver::TokioDriver;

// --- Fanout on the scheduler: N arm chains interleaved by the driver ---
// Each mirrored test names the legacy case it mirrors. The legacy cases
// keep exercising the legacy fanout driver untouched; these prove the
// scheduler's arm chains.

/// Builds the run context and its silent host for a scheduler fanout test
/// with the given limits, so a window test can narrow the concurrency.
fn scheduler_context_with_limits(prompt: &Prompt, limits: RunLimits) -> (RunState, RunHost) {
    scheduler_context_from(
        prompt,
        &TestStore::new(),
        &test_context(EXECUTION).limits(limits),
        RunHost::new(),
    )
}

/// The prompt in each gateway request, in arrival order.
pub(super) fn request_prompts(gateway: &ScriptedGateway) -> Vec<String> {
    gateway
        .requests()
        .iter()
        .map(|body| {
            body["messages"][0]["content"]
                .as_str()
                .expect("an infer request includes a user message")
                .to_owned()
        })
        .collect()
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_results_follow_collection_order_not_finish_order() {
    // Mirror of the legacy `results_follow_collection_order_not_finish_order`:
    // arm "two" finishes first (one infer) while arm "one" is still parked
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
async fn fanout_concurrency_window_limits_active_arms() {
    // The window mirror of the legacy `ArmWindow` contract: with the
    // window at 1, each arm runs both of its infers before the next arm
    // starts - a window that let arms overlap would interleave the
    // requests (x:a, y:a, ...).
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
        RunLimits::new().max_fanout_concurrency(NonZeroUsize::new(1).expect("1 is non-zero")),
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("the windowed fanout completes on the scheduler");

    assert_eq!(out, "r1r2|r3r4|r5r6");
    assert_eq!(
        request_prompts(&gateway),
        vec!["x:a", "x:b", "y:a", "y:b", "z:a", "z:b"],
        "with the window at 1 each arm finishes before the next starts"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_arms_take_child_ids_in_collection_order_per_fanout_index_and_structured_results() {
    // Mirror of the legacy
    // `fanout_arms_take_child_ids_in_collection_order_and_a_per_fanout_index`
    // plus the structured-result shape of `fanout_returns_structured_results`:
    // each arm is a child chain of the caller (`0.0`, `0.1`) whose worker
    // entry is `0.K.0`, `sys.index` is the
    // 1-based per-fanout position, and the packed sequence holds `.ok`
    // and `.item` with `__tostring` driving `table.concat`. The ids log is
    // arm-scoped (the pattern the claims model teaches): every store op is
    // a leaf yield now, so two arms appending one path would genuinely race
    // and boom; the parent's post-join read merges the arm logs in order.
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
async fn fanout_over_a_large_collection_refills_the_window() {
    // Mirror of the legacy `fanout_accepts_a_list_over_the_old_default_cap`:
    // a 1025-member collection runs to completion past the 8-wide default
    // window - a refill that lost track of the next index would stall the
    // driver or drop results.
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
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a collection over the window width completes");

    assert_eq!(out, "1025:1:1025");
}

#[tokio::test(flavor = "current_thread")]
async fn pre_cancelled_fanout_returns_interrupted() {
    // Mirror of the legacy `pre_cancelled_fanout_returns_interrupted`: a
    // fanout entered under an already-cancelled handle fails the run with
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
    let (ctx, host) = scheduler_context(&prompt);
    let mut driver = TokioDriver::new(&ctx, host, None);
    driver.cancel_handle().cancel();
    let result = driver.drive().await;
    assert!(
        matches!(result, Err(Error::Interrupted)),
        "a pre-cancelled fanout must interrupt the run, got {result:?}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn model_required_when_arm_infer_has_no_binding() {
    // Mirror of the legacy `model_required_when_arm_prose_has_no_binding`:
    // an arm whose explicit infer of its prose has no model binding fails
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
    let error = TokioDriver::new(&ctx, RunHost::new(), None)
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
    // Mirror of the legacy `the_shared_replay_sees_the_arm_item`: the `item`
    // global installs before `replay_shared`, so the shared library's
    // top-level code may read `item`; moving the install after the replay
    // would capture nil in the arm and fail this test. The context holds
    // the prompt's real compiled shared library, not the empty stand-in the
    // other scheduler tests use.
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
    let out = TokioDriver::new(&ctx, RunHost::new(), None)
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
    // Mirror of the legacy `jump_inside_a_fanout_arm_drives_a_child_walk`:
    // the arm's remaining blocks are skipped, the walk continues on the
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
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
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
    // Mirror of the legacy
    // `jump_inside_a_fanout_arm_to_a_worker_child_walks_the_child_slice`:
    // the descent runs the worker's child slice from the target, the target
    // takes the arm chain's next entry id with no `item` seed (the transfer clears
    // the arm's at-worker state, so the child walk runs as plain sections),
    // and the walk falls through to the target's child siblings.
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
    let (ctx, host) = scheduler_context_on(&prompt, &store, Arc::new(NullObserver::default()));
    let out = TokioDriver::new(&ctx, host, None)
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
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("a hash-shaped collection fans out");

    assert_eq!(out, "alpha=2@1:0.0.0,mid=3@2:0.1.0,zeta=1@3:0.2.0");
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_results_are_sealed_against_writes_and_metatable_replacement() {
    // The A9 seal on a result object: an assignment raises, `setmetatable`
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
    let (ctx, host) = scheduler_context(&prompt);
    let out = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect("the sealed results read and render");

    assert_eq!(out, "alpha-1,beta-2");
}

// --- Fanout failure semantics on the scheduler ---
// Each mirrored test names the legacy case it mirrors. The arms are task
// chains the `fanout` shim spawns, so their lifecycle reports through the
// `Task*` observations: started under the caller's section, the terminal
// under the worker's.

#[tokio::test(flavor = "current_thread")]
async fn fanout_empty_collection_errors_before_any_scheduling() {
    // Pin of the pre-scheduling guard, mirroring the legacy
    // `fanout_collection_empty_errors` and
    // `an_empty_collection_is_rejected_before_any_scheduling`: the fanout
    // errors before any arm is created - no STARTED observation, and the
    // worker's store tripwire never fires.
    let store = TestStore::new();
    let recorder = Arc::new(Recorder::default());
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Worker', {})\n```\n\n\
        ### Worker\n\n\
        ```lua\nstore.write('ran.txt', 'yes')\n```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(&prompt, &store, recorder.clone());
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("an empty collection must error");

    assert!(
        error.to_string().contains("empty collection"),
        "error was: {error}"
    );
    assert!(
        store.read("ran.txt").is_err(),
        "the worker never ran: the rejection precedes scheduling"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        0,
        "no arm was ever started: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_worker_that_is_a_list_section_errors() {
    // Pin of the worker-template guard, mirroring the legacy case of the
    // same name: a resolved list section is not a worker template.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\nfanout('### Items', {'x'})\n```\n\n\
        ### Items\n\n\
        - a\n\
        - b\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("a list section is not a worker template");

    assert!(
        error
            .to_string()
            .contains("is a list section, not a worker template"),
        "error was: {error}"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn fanout_depth_cap_reads_the_chain_field() {
    // Pin of the fanout depth-cap guard: Alpha and Beta ping-pong calls
    // down the chain stack, and the chain that lands at depth 8 calls
    // fanout - each arm would run one level deeper, so the cap fires from
    // the requesting chain's call-depth field. The arm's spawn sets the
    // fanout mark, so the spawn arm names the cap after `fanout` in the
    // typed error itself; the shim re-raises that table and the retained
    // typed `Lua` error is substituted back at every `call` level, so the
    // run's error is byte-identical to the retired Rust raise: the exact
    // variant, the exact text, no runtime-error prefix or traceback.
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Depth\n\n\
        ## Alpha\n\n\
        ```lua\n\
        var.n = (var.n or 0) + 1\n\
        if var.n >= 9 then return fanout('### Worker', {'x'}) end\n\
        return call('## Beta')\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\nreturn item\n```\n\n\
        ## Beta\n\n\
        ```lua\n\
        var.n = var.n + 1\n\
        return call('## Alpha')\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context(&prompt);
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("the fanout depth cap must fail the run");

    assert!(
        matches!(&error, Error::Lua(message) if message == "fanout recursion exceeded cap of 8"),
        "expected the typed Lua depth-cap error with fanout's own wording, got {error:?}"
    );
    assert_eq!(
        error.to_string(),
        "fanout recursion exceeded cap of 8",
        "the rendered text is exactly the fanout cap message: no call wording, no prefix"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn an_exhausted_arm_becomes_the_incomplete_stub_and_its_sibling_still_lands() {
    // The `tool_loop_exhausted` arm rule: one arm's `models.loop` runs its
    // round cap against a never-converging model and fails as
    // `tool_loop_exhausted`; the shim turns that arm's slot into the
    // incomplete stub (`ok = false`, `exhausted = true`) and the fanout
    // continues, so the sibling's plain result lands beside it. Only the
    // looping arm touches the gateway, so the scripted replies serve one
    // arm and the run is deterministic. The exhausted arm still reports
    // `TaskFailed` (the stub is the fanout's recovery, not the arm's), and
    // the sibling `TaskSucceeded`.
    let cap = 2;
    let gateway = ScriptedGateway::start(vec![
        resp_tool_call("call_0", "echo", "{\"value\":\"x\"}"),
        resp_tool_call("call_1", "echo", "{\"value\":\"x\"}"),
    ])
    .await;
    let md = format!(
        "---\nname: t\ndescription: d\npromptforge: 0\nmax_tool_iterations: {cap}\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {{'loop', 'plain'}})\n\
        assert(#r == 2, 'both slots are filled')\n\
        assert(r[1].ok == false and r[1].exhausted == true, 'the exhausted arm is flagged')\n\
        assert(r[1].item == 'loop', 'the stub keeps its item')\n\
        assert(r[2].ok == true and r[2].exhausted == false, 'the sibling is a plain success')\n\
        assert(r[2].item == 'plain', 'the sibling keeps its item')\n\
        return r[1].text .. '|' .. r[2].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        if item == 'loop' then\n\
          local msgs = messages.new()\n\
          msgs:user('loop forever')\n\
          models.loop(msgs)\n\
          return 'unreachable'\n\
        end\n\
        return 'plain:' .. item\n\
        ```\n"
    );
    let prompt = parse(&md);
    let recorder = Arc::new(Recorder::default());
    let (ctx, host) = loop_context_observed(
        &prompt,
        echo_tools(),
        Arc::clone(&recorder) as Arc<dyn Observer>,
    );
    let out = TokioDriver::new(&ctx, host, Some(gateway_client(gateway.addr())))
        .drive()
        .await
        .expect("an exhausted arm must not fail the fanout");

    assert_eq!(
        out, "## loop\n\nUNKNOWN\n\n(section incomplete: tool loop exhausted)|plain:plain",
        "the exhausted slot is the stub and the sibling's result lands"
    );
    assert_eq!(
        gateway.call_count(),
        cap,
        "the looping arm made exactly `cap` round trips before exhausting"
    );
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        1,
        "the exhausted arm reports its failure: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_SUCCEEDED),
        1,
        "the sibling reports its success: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_CANCELLED),
        0,
        "an exhausted arm cancels nothing: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_arms_writing_one_path_terminate_the_run_with_a_determinism_violation() {
    // Restructured for the leaf-yield store path: each arm's write is a
    // yield answered from the blocking pool, so two live arms writing one
    // path race and the loser's op booms. The violation is fatal to the
    // whole run at the answer boundary - the loser never resumes into Lua,
    // so no author pcall can catch it - and every arm still parked when
    // the fatal answer lands drops unarmed, reporting cancelled rather
    // than failed.
    //
    // The gate makes the conflict deterministic: the first write to reach
    // the backend parks with its claim held, so the second write's claim
    // check meets it no matter how late the second blocking-pool thread
    // starts. Without the gate the winner could write, return, and retire
    // its claim before the loser's op ran, and the run would succeed.
    //
    // The gate does not order the two answers. It opens on the loser's
    // failed observation, which fires before the loser posts its answer,
    // so the winner's thread may complete its write and post first; the
    // driver then resumes the winner into Lua and that arm succeeds before
    // the fatal answer ends the run. Either interleaving satisfies the
    // contract: both arms started, no arm fails on its own, and at most
    // the winner reports a terminal - the loser is stranded mid-chain by
    // the run's own failure, which is the record of how it ended.
    let recorder = Arc::new(Recorder::default());
    let gate = Arc::new(StoreGate::default());
    let store = gated_store(&gate);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.write('shared.txt', item)\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) =
        scheduler_context_on(&prompt, &store, GateObserver::new(&gate, recorder.clone()));
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("two live arms writing one path must terminate the run");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("shared.txt"), "error was: {detail}");
            assert!(
                detail.contains("conflicts with"),
                "the conflict is named: {detail}"
            );
            assert_eq!(
                detail.matches("ExecId(").count(),
                2,
                "both arms' identities are named: {detail}"
            );
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
    assert_eq!(
        terminal_count(&recorder, TASK_STARTED),
        2,
        "both arms started before the conflict: {:?}",
        recorder.events()
    );
    assert_eq!(
        terminal_count(&recorder, TASK_FAILED),
        0,
        "no arm fails on its own; the run ends at the answer boundary: {:?}",
        recorder.events()
    );
    assert!(
        terminal_count(&recorder, TASK_SUCCEEDED) <= 1,
        "at most the winner (whose answer landed first) reports a terminal: {:?}",
        recorder.events()
    );
}

#[tokio::test(flavor = "current_thread")]
async fn two_live_arms_appending_one_path_terminate_with_a_determinism_violation() {
    // The papergate case the WriteScope registry never caught: `append`
    // claims write intent now, so two live arms appending to one path
    // conflict exactly as two writes do - and under the leaf-yield store
    // path the conflict is the fatal determinism violation, not a
    // per-arm store error. The gate holds the first append's claim until
    // the second append has met it, so the conflict cannot depend on
    // blocking-pool timing.
    let gate = Arc::new(StoreGate::default());
    let store = gated_store(&gate);
    let md = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
        # Fanout\n\n\
        ## Parent\n\n\
        ```lua\n\
        local r = fanout('### Worker', {'alpha', 'beta'})\n\
        return r[1].text\n\
        ```\n\n\
        ### Worker\n\n\
        ```lua\n\
        store.append('evidence.md', item .. '\\n')\n\
        return item\n\
        ```\n";
    let prompt = parse(md);
    let (ctx, host) = scheduler_context_on(
        &prompt,
        &store,
        GateObserver::new(&gate, Arc::new(NullObserver::default())),
    );
    let error = TokioDriver::new(&ctx, host, None)
        .drive()
        .await
        .expect_err("two live arms appending one path must terminate the run");

    match &error {
        Error::Determinism(detail) => {
            assert!(detail.contains("evidence.md"), "error was: {detail}");
            assert_eq!(
                detail.matches("ExecId(").count(),
                2,
                "both arms' identities are named: {detail}"
            );
        }
        other => panic!("expected the fatal determinism violation, got {other:?}"),
    }
}
