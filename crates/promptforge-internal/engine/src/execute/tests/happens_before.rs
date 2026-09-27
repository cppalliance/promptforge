//! The happens-before trace tests: the plan's determinism traces become
//! tests, each driven under the serial driver (three answer batchings)
//! and the multi-threaded tokio driver with a seeded effect-completion
//! shuffle (one interleaving per seed), asserting that the store
//! verdicts and contents never change with the interleaving.
//!
//! Traces 3, 4, 8, and 9 live at the VFS level in `promptforge-vfs`'s
//! handle tests. Traces 1, 2, 5, 7, 10, and 11 - plus the freeform and
//! pipeline patterns from the plan's Functional Specification - live
//! here. Trace 2 stays in the fanout suites, which pin the unconditional
//! conflict directly. The only outcomes that may differ between
//! interleavings are the ones the plan admits as recorded nondeterminism:
//! which task `join_any` returns, and how far a cancelled task got; the
//! tests below pin the invariant around them.

use std::sync::Arc;

use crate::RunErrorKind;
use crate::execute::RunResult;
use crate::execute::run::Run;
use crate::test_support::tokio_driver::TokioDriver;

use super::scheduler::scheduler_context_from;
use super::scheduler::scheduler_context_on;
use super::serial_driver::{Batching, drive_batched};
use super::*;

/// The first canonical pattern: a fanout of seven into one file. The arms
/// write their own partitions, and the caller merges by index after the
/// fanout returns - the fanout's join_any rounds join every arm before
/// it returns, so the merge reads are ordered after every write.
const SEVEN_ARM_FANOUT: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Research\n\n\
    ## Parent\n\n\
    ```lua\n\
    local topics = { 'a', 'b', 'c', 'd', 'e', 'f', 'g' }\n\
    local results = fanout('### Worker', topics)\n\
    local parts = {}\n\
    for i = 1, #results do parts[i] = store.read('research/' .. i .. '.md') end\n\
    store.write('research.md', table.concat(parts, '\\n\\n'))\n\
    return store.read('research.md')\n\
    ```\n\n\
    ### Worker\n\n\
    ```lua\n\
    store.write('research/' .. sys.index .. '.md', 'part ' .. item)\n\
    return 'ok'\n\
    ```\n";

/// The merged file the seven-arm fanout and the freeform pattern both
/// produce.
const SEVEN_PARTS: &str = "part a\n\npart b\n\npart c\n\npart d\n\npart e\n\npart f\n\npart g";

/// The freeform pattern: prepare the spawns, join the whole set, then
/// merge by index. `tasks.join` delivers every member, so every arm's
/// write is joined before the merge reads.
const FREE_FORM: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Research\n\n\
    ## Parent\n\n\
    ```lua\n\
    local topics = { 'a', 'b', 'c', 'd', 'e', 'f', 'g' }\n\
    local set = {}\n\
    for i, topic in ipairs(topics) do\n\
      set[i] = tasks.spawn('### Worker', { item = topic, index = i })\n\
    end\n\
    tasks.join(set)\n\
    local parts = {}\n\
    for i = 1, #set do parts[i] = store.read('research/' .. i .. '.md') end\n\
    store.write('research.md', table.concat(parts, '\\n\\n'))\n\
    return store.read('research.md')\n\
    ```\n\n\
    ### Worker\n\n\
    ```lua\n\
    store.write('research/' .. sys.index .. '.md', 'part ' .. item)\n\
    return 'ok'\n\
    ```\n";

/// The pipeline pattern: a task cannot join its sibling, so the owner
/// orders them - Summarize is spawned after Gather is joined, so it sees
/// Gather's files.
const PIPELINE: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Pipeline\n\n\
    ## Parent\n\n\
    ```lua\n\
    local g = tasks.spawn('### Gather')\n\
    tasks.join({ g })\n\
    local s = tasks.spawn('### Summarize')\n\
    tasks.join({ s })\n\
    return store.read('summary.md')\n\
    ```\n\n\
    ### Gather\n\n\
    ```lua\n\
    store.write('gathered.md', 'gathered content')\n\
    return 'done'\n\
    ```\n\n\
    ### Summarize\n\n\
    ```lua\n\
    local content = store.read('gathered.md')\n\
    store.write('summary.md', 'summary of: ' .. content)\n\
    return 'done'\n\
    ```\n";

/// Trace 5: `join_any({a, b})`, then reading the returned task's file.
/// It passes exactly when the file read belongs to the task the wait
/// returned, and that choice is the recorded nondeterminism.
const TRACE_5_PASS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Waits\n\n\
    ## Main\n\n\
    ```lua\n\
    local a = tasks.spawn('## Arm', { item = 'a' })\n\
    local b = tasks.spawn('## Arm', { item = 'b' })\n\
    local first, ok, result = tasks.join_any({ a, b })\n\
    assert(ok, tostring(result))\n\
    if first.task == a.task then return store.read('a.txt') end\n\
    return store.read('b.txt')\n\
    ```\n\n\
    ## Arm\n\n\
    ```lua\n\
    store.write(item .. '.txt', 'written by ' .. item)\n\
    return models.infer('arm ' .. item)\n\
    ```\n";

/// Trace 5's negative: reading the file of the task the wait did NOT
/// return. The undelivered sibling is unordered with the caller, so the
/// read always fails with the fatal determinism violation - in every
/// interleaving, whichever arm won.
const TRACE_5_FAIL: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Waits\n\n\
    ## Main\n\n\
    ```lua\n\
    local a = tasks.spawn('## Arm', { item = 'a' })\n\
    local b = tasks.spawn('## Arm', { item = 'b' })\n\
    local first, ok, result = tasks.join_any({ a, b })\n\
    assert(ok, tostring(result))\n\
    if first.task == a.task then return store.read('b.txt') end\n\
    return store.read('a.txt')\n\
    ```\n\n\
    ## Arm\n\n\
    ```lua\n\
    store.write(item .. '.txt', 'written by ' .. item)\n\
    return models.infer('arm ' .. item)\n\
    ```\n";

/// Trace 7: the H1 pass writes config.md and spawns T; the walk reads
/// config.md, then joins T. One root identity spans the pass and the
/// walk, so there is no false conflict, and the join makes T's write
/// readable.
const TRACE_7: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Trace 7\n\n\
    ```lua\n\
    store.write('config.md', 'configured')\n\
    var.t = tasks.spawn('## Task')\n\
    ```\n\n\
    ## Result\n\n\
    ```lua\n\
    local cfg = store.read('config.md')\n\
    assert(cfg == 'configured', cfg)\n\
    local _, ok, result = tasks.join_any({ var.t })\n\
    assert(ok and result == 'task done', tostring(result))\n\
    return cfg .. '/' .. store.read('t.txt')\n\
    ```\n\n\
    ## Task\n\n\
    ```lua\n\
    store.write('t.txt', 'task wrote')\n\
    return 'task done'\n\
    ```\n";

/// Trace 10, the delivered half: a timed join over one slow arm. The
/// quick arm's delivery (an ordinary join) is joined, and the slow arm is
/// joined either by the timed join itself or by the later join - either
/// way its write is readable, and the result never changes.
const TRACE_10_PASS: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Timed\n\n\
    ## Main\n\n\
    ```lua\n\
    local b = tasks.spawn('## Quick')\n\
    local _, ok, result = tasks.join_any({ b })\n\
    assert(ok and result == 'quick arm', tostring(result))\n\
    local btext = store.read('b.txt')\n\
    local a = tasks.spawn('## Slow')\n\
    tasks.join({ a }, { timeout = 0.05 })\n\
    local ok2 = pcall(tasks.join_any, { a })\n\
    local atext = store.read('a.txt')\n\
    return btext .. '|' .. atext\n\
    ```\n\n\
    ## Slow\n\n\
    ```lua\n\
    store.write('a.txt', 'slow arm')\n\
    return 'slow arm'\n\
    ```\n\n\
    ## Quick\n\n\
    ```lua\n\
    store.write('b.txt', 'quick arm')\n\
    return 'quick arm'\n\
    ```\n";

/// Trace 10, the late-member half: reading the late member's file before
/// a later join delivers it always fails with the fatal determinism
/// violation. The slow arm parks on a model round past the timeout, so
/// the timeout provably wins in every interleaving.
const TRACE_10_FAIL: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Timed\n\n\
    ## Main\n\n\
    ```lua\n\
    local a = tasks.spawn('## Slow')\n\
    tasks.join({ a }, { timeout = 0.05 })\n\
    return store.read('a.txt')\n\
    ```\n\n\
    ## Slow\n\n\
    ```lua\n\
    store.write('a.txt', 'slow arm')\n\
    return models.infer('slow')\n\
    ```\n";

/// Trace 11: cancellation. However far the cancelled task got, the join
/// at delivery makes its completed write readable.
const TRACE_11: &str = "---\nname: t\ndescription: d\npromptforge: 0\n---\n\n\
    # Cancel\n\n\
    ## Main\n\n\
    ```lua\n\
    local a = tasks.spawn('## Task')\n\
    store.exists('p1')\n\
    store.exists('p2')\n\
    tasks.cancel(a)\n\
    tasks.join_any({ a })\n\
    return store.read('partial.txt')\n\
    ```\n\n\
    ## Task\n\n\
    ```lua\n\
    store.write('partial.txt', 'partial')\n\
    return models.infer('slow')\n\
    ```\n";

/// The three serial-driver batchings: one answer per step, all at once,
/// and reversed - the effect-completion shuffles the serial driver
/// offers.
const BATCHINGS: [Batching; 3] = [
    Batching::OnePerStep,
    Batching::AllAtOnce,
    Batching::Reversed,
];

/// Runs `prompt` under the serial driver with every batching, on a fresh
/// scope each time, returning the results in batching order.
fn serial_results(prompt: &Prompt, store: &TestStore) -> Vec<RunResult> {
    BATCHINGS
        .into_iter()
        .map(|batching| {
            let (state, _host) =
                scheduler_context_from(prompt, store, &test_context(EXECUTION), RunHost::new());
            drive_batched(Run::from_state(state), batching).result
        })
        .collect()
}

/// The text of a successful serial result.
fn ok_text(result: &RunResult) -> String {
    match result {
        RunResult::Ok(text) => text.clone(),
        other => panic!("the run succeeds: {other:?}"),
    }
}

/// Whether a serial result is the fatal determinism violation.
fn is_determinism(result: &RunResult) -> bool {
    matches!(
        result,
        RunResult::Failure(error) if error.kind() == RunErrorKind::Determinism
    )
}

/// The tokio completion-order seeds: the tokio driver offers no
/// batching modes, so each trace runs once per seed, and the driver's
/// seeded shuffle permutes every wave of concurrently completed
/// effects.
const TOKIO_SEEDS: [u64; 3] = [1, 2, 3];

/// Drives `prompt` on the tokio driver once per seed, on a fresh scope
/// with a fresh `client` each time, returning the results in seed
/// order.
async fn tokio_results(
    prompt: &Prompt,
    store: &TestStore,
    client: impl Fn() -> Option<MockGatewayClient>,
) -> Vec<Result<String>> {
    let mut results = Vec::new();
    for seed in TOKIO_SEEDS {
        let (ctx, host) = scheduler_context_on(prompt, store, Arc::new(NullObserver::default()));
        let mut driver = TokioDriver::new(&ctx, host, client());
        driver.set_shuffle_for_test(seed);
        results.push(driver.drive().await);
    }
    results
}

/// The text of a successful tokio result.
fn ok_tokio_text(result: &Result<String>) -> String {
    match result {
        Ok(text) => text.clone(),
        Err(error) => panic!("the tokio run succeeds: {error}"),
    }
}

/// Whether a tokio result is the fatal determinism violation.
fn is_tokio_determinism(result: &Result<String>) -> bool {
    matches!(result, Err(Error::Determinism(_)))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_seven_arm_fanout_merges_its_partitions_in_every_interleaving() {
    let prompt = parse(SEVEN_ARM_FANOUT);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), SEVEN_PARTS);
    }
    for result in &tokio_results(&prompt, &store, || None).await {
        assert_eq!(ok_tokio_text(result), SEVEN_PARTS);
    }
    assert_eq!(
        store.read("research.md").expect("the merge lands"),
        SEVEN_PARTS
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_freeform_pattern_merges_by_index_in_every_interleaving() {
    let prompt = parse(FREE_FORM);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), SEVEN_PARTS);
    }
    for result in &tokio_results(&prompt, &store, || None).await {
        assert_eq!(ok_tokio_text(result), SEVEN_PARTS);
    }
    assert_eq!(
        store.read("research.md").expect("the merge lands"),
        SEVEN_PARTS
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_pipeline_pattern_orders_the_summary_after_the_gather_in_every_interleaving() {
    let prompt = parse(PIPELINE);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), "summary of: gathered content");
    }
    for result in &tokio_results(&prompt, &store, || None).await {
        assert_eq!(ok_tokio_text(result), "summary of: gathered content");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn join_any_joins_the_task_it_returns_and_only_that_task() {
    // The pass half: reading the returned task's file passes, and the
    // text is exactly that arm's. Which arm the wait returns is the
    // recorded nondeterminism, so each interleaving is one of the two
    // legal texts.
    let prompt = parse(TRACE_5_PASS);
    let store = TestStore::new();
    let serials = serial_results(&prompt, &store);
    for (index, result) in serials.iter().enumerate() {
        let text = ok_text(result);
        assert!(
            text == "written by a" || text == "written by b",
            "batching {index} produced an illegal verdict: {text}"
        );
    }
    let gateway = ScriptedGateway::start(vec![resp_text("ra"), resp_text("rb")]).await;
    for (index, result) in tokio_results(&prompt, &store, || Some(gateway_client(gateway.addr())))
        .await
        .iter()
        .enumerate()
    {
        let text = ok_tokio_text(result);
        assert!(
            text == "written by a" || text == "written by b",
            "seed {index} produced an illegal verdict: {text}"
        );
    }

    // The fail half: reading the file of the task the wait did NOT
    // return always ends the run as the fatal determinism violation.
    let fail = parse(TRACE_5_FAIL);
    for result in &serial_results(&fail, &store) {
        assert!(
            is_determinism(result),
            "expected determinism, got {result:?}"
        );
    }
    let gateway = ScriptedGateway::start(vec![resp_text("ra"), resp_text("rb")]).await;
    for (index, result) in tokio_results(&fail, &store, || Some(gateway_client(gateway.addr())))
        .await
        .iter()
        .enumerate()
    {
        assert!(
            is_tokio_determinism(result),
            "seed {index} must conflict, got {result:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn one_root_identity_spans_the_h1_pass_and_the_walk() {
    let prompt = parse(TRACE_7);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), "configured/task wrote");
    }
    for result in &tokio_results(&prompt, &store, || None).await {
        assert_eq!(ok_tokio_text(result), "configured/task wrote");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_join_joins_the_members_it_delivers_and_a_later_join_the_rest() {
    // The delivered half: whichever join delivers the slow arm (the
    // timed join, or the later join), its write is readable, and the
    // quick arm's ordinary-join delivery is joined the same way.
    let prompt = parse(TRACE_10_PASS);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), "quick arm|slow arm");
    }
    for result in &tokio_results(&prompt, &store, || None).await {
        assert_eq!(ok_tokio_text(result), "quick arm|slow arm");
    }

    // The late-member half: before a later join delivers the late arm,
    // reading its file always fails - the timeout provably wins, because
    // the arm parks on a model round past it.
    let fail = parse(TRACE_10_FAIL);
    for result in &serial_results(&fail, &store) {
        assert!(
            is_determinism(result),
            "expected determinism, got {result:?}"
        );
    }
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "slow answer",
        std::time::Duration::from_millis(500),
    )])
    .await;
    for (index, result) in tokio_results(&fail, &store, || Some(gateway_client(gateway.addr())))
        .await
        .iter()
        .enumerate()
    {
        assert!(
            is_tokio_determinism(result),
            "seed {index} must conflict, got {result:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_tasks_partial_write_is_readable_after_the_join() {
    let prompt = parse(TRACE_11);
    let store = TestStore::new();
    for result in &serial_results(&prompt, &store) {
        assert_eq!(ok_text(result), "partial");
    }
    let gateway = ScriptedGateway::start(vec![resp_delayed_text(
        "slow answer",
        std::time::Duration::from_millis(500),
    )])
    .await;
    for result in &tokio_results(&prompt, &store, || Some(gateway_client(gateway.addr()))).await {
        assert_eq!(ok_tokio_text(result), "partial");
    }
}
