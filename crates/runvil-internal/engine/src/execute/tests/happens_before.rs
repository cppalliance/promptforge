//! The happens-before trace tests: the plan's determinism traces become
//! tests, each driven under the serial driver (three answer batchings)
//! and on the rig with a seeded effect-completion shuffle (one
//! interleaving per seed), asserting that the store verdicts and contents
//! never change with the interleaving.
//!
//! Traces 3, 4, 8, and 9 live at the VFS level in `runvil-vfs`'s
//! handle tests. Traces 1, 2, 5, 7, 10, and 11 - plus the freeform and
//! pipeline patterns from the plan's Functional Specification - live
//! here, and so do the two concurrency traces: 6 (nested tasks, run
//! within the limits, no deadlock at a ceiling of 1) and 12 (an arm's
//! `tasks.concurrency` limit and its clamping to the caller's ceiling).
//! Trace 2 stays in the fanout suites, which pin the unconditional
//! conflict directly. The only outcomes that may differ between
//! interleavings are the ones the plan admits as recorded nondeterminism:
//! which task `join_any` returns, and how far a cancelled task got; the
//! tests below pin the invariant around them.
//! The two concurrency traces sit in `concurrency`.

use crate::RunErrorKind;
use crate::execute::RunResult;
use crate::test_support::rig::{Rig, RigTrace};
use crate::test_support::scenario::{Batching, TestStore, drive_batched};
use crate::test_support::seeded::Seeded;

use super::scheduler::{asks, scheduler_scenario};
use super::*;

/// The first canonical pattern: a fanout of seven into one file. The arms
/// write their own partitions, and the caller merges by index after the
/// fanout returns - the fanout's join_any rounds join every arm before
/// it returns, so the merge reads are ordered after every write.
const SEVEN_ARM_FANOUT: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
    # Research\n\n\
    ## Parent\n\n\
    ```lua\n\
    local topics = { 'a', 'b', 'c', 'd', 'e', 'f', 'g' }\n\
    local results = tasks.fanout('### Worker', topics)\n\
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
const FREE_FORM: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const PIPELINE: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_5_PASS: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_5_FAIL: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_7: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_10_PASS: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_10_FAIL: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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
const TRACE_11: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
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

/// The store probe: whether the file the run writes was already there
/// when it started, which only an earlier run could have left.
const STORE_PROBE: &str = "---\nname: t\ndescription: d\nrunvil: 0\n---\n\n\
    # Probe\n\n\
    ## Main\n\n\
    ```lua\n\
    local seen = store.exists('probe.txt')\n\
    store.write('probe.txt', 'x')\n\
    if seen then return 'present' end\n\
    return 'absent'\n\
    ```\n";

/// The three serial-driver batchings: one answer per step, all at once,
/// and reversed - the effect-completion shuffles the serial driver
/// offers.
const BATCHINGS: [Batching; 3] = [
    Batching::OnePerStep,
    Batching::AllAtOnce,
    Batching::Reversed,
];

/// A run of the trace `source` on a fresh scope over `store`. The sweep
/// helpers hand each run its own empty store, so no run reads a file an
/// earlier run of the same trace wrote.
fn trace_scenario(source: &str, store: &TestStore) -> Scenario {
    scheduler_scenario(source).vfs(store.vfs())
}

/// Runs `scenario` under the serial driver with every batching, each
/// over its own empty store, returning each batching beside its result
/// and the store the run left.
#[track_caller]
fn serial_results(
    scenario: impl Fn(&TestStore) -> Scenario,
) -> Vec<(Batching, RunResult, TestStore)> {
    let mut results = Vec::new();
    for batching in BATCHINGS {
        let store = TestStore::new();
        let run = scenario(&store).into_run();
        results.push((batching, drive_batched(run, batching).result, store));
    }
    results
}

/// The shuffle seeds: each trace runs once per seed on the rig, which
/// delivers every step's answers in the seed's permutation.
const SEEDS: [u64; 3] = [1, 2, 3];

/// Runs `scenario` on the shuffled rig once per seed, each over its own
/// empty store, returning each seed's checker beside what the run left.
#[track_caller]
fn shuffled_results(scenario: impl Fn(&TestStore) -> Scenario) -> Vec<(Seeded, RigTrace)> {
    let mut results = Vec::new();
    for seed in SEEDS {
        let store = TestStore::new();
        results.push((Seeded::new(seed), Rig::shuffled(scenario(&store), seed)));
    }
    results
}

/// The text of a successful result, or `None`.
fn ok_text(result: &RunResult) -> Option<&str> {
    match result {
        RunResult::Ok(text) => Some(text),
        _ => None,
    }
}

/// Whether a result is the fatal determinism violation.
fn is_determinism(result: &RunResult) -> bool {
    matches!(
        result,
        RunResult::Failure(error) if error.kind() == RunErrorKind::Determinism
    )
}

/// Asserts every serial batching and every seeded interleaving of
/// `scenario` succeeds with `expected`.
#[track_caller]
fn assert_every_interleaving_reads(scenario: impl Fn(&TestStore) -> Scenario, expected: &str) {
    assert_every_interleaving_leaves(scenario, expected, None);
}

/// [`assert_every_interleaving_reads`], and, given `file`, that each run
/// left that `(path, contents)` in its own store.
#[track_caller]
fn assert_every_interleaving_leaves(
    scenario: impl Fn(&TestStore) -> Scenario,
    expected: &str,
    file: Option<(&str, &str)>,
) {
    for (batching, result, store) in serial_results(&scenario) {
        assert_eq!(
            ok_text(&result),
            Some(expected),
            "under {batching:?}: {result:?}"
        );
        if let Some((path, contents)) = file {
            let left = store.read(path);
            assert_eq!(
                left.as_deref().ok(),
                Some(contents),
                "{path} under {batching:?}: {left:?}"
            );
        }
    }
    for (seeded, rig) in shuffled_results(&scenario) {
        let result = rig.trace().result();
        seeded.check_eq(&ok_text(result), &Some(expected), format!("{result:?}"));
        if let Some((path, contents)) = file {
            let left = rig.trace().read(path);
            seeded.check_eq(
                &left.as_deref().ok(),
                &Some(contents),
                format!("{path}: {left:?}"),
            );
        }
    }
}

/// Asserts every serial batching and every seeded interleaving of
/// `scenario` ends as the fatal determinism violation.
#[track_caller]
fn assert_every_interleaving_conflicts(scenario: impl Fn(&TestStore) -> Scenario) {
    for (batching, result, _) in serial_results(&scenario) {
        assert!(
            is_determinism(&result),
            "under {batching:?}, expected determinism, got {result:?}"
        );
    }
    for (seeded, rig) in shuffled_results(&scenario) {
        let result = rig.trace().result();
        seeded.check(
            is_determinism(result),
            format!("expected determinism, got {result:?}"),
        );
    }
}

#[test]
fn a_seven_arm_fanout_merges_its_partitions_in_every_interleaving() {
    let fanout = |store: &TestStore| trace_scenario(SEVEN_ARM_FANOUT, store);
    assert_every_interleaving_leaves(fanout, SEVEN_PARTS, Some(("research.md", SEVEN_PARTS)));
    assert!(
        shuffled_results(fanout).iter().any(|(_, rig)| {
            rig.trace()
                .effects()
                .iter()
                .map(|(effect, _)| effect)
                .ne(rig.issued())
        }),
        "no seed in {SEEDS:?} answered the arms' effects out of issue order"
    );
}

#[test]
fn the_freeform_pattern_merges_by_index_in_every_interleaving() {
    assert_every_interleaving_leaves(
        |store| trace_scenario(FREE_FORM, store),
        SEVEN_PARTS,
        Some(("research.md", SEVEN_PARTS)),
    );
}

#[test]
fn the_pipeline_pattern_orders_the_summary_after_the_gather_in_every_interleaving() {
    assert_every_interleaving_reads(
        |store| trace_scenario(PIPELINE, store),
        "summary of: gathered content",
    );
}

#[test]
fn join_any_joins_the_task_it_returns_and_only_that_task() {
    // The pass half: reading the returned task's file passes, and the
    // text is exactly that arm's. Which arm the wait returns is the
    // recorded nondeterminism, so each interleaving is one of the two
    // legal texts.
    let gateway = ScriptedChat::new(vec![text("ra"), text("rb")]);
    let pass = |store: &TestStore| trace_scenario(TRACE_5_PASS, store).chat(&gateway);
    let legal =
        |result: &RunResult| matches!(ok_text(result), Some("written by a" | "written by b"));
    for (batching, result, _) in serial_results(pass) {
        assert!(
            legal(&result),
            "under {batching:?}, an illegal verdict: {result:?}"
        );
    }
    for (seeded, rig) in shuffled_results(pass) {
        let result = rig.trace().result();
        seeded.check(legal(result), format!("an illegal verdict: {result:?}"));
    }

    // The fail half: reading the file of the task the wait did NOT
    // return always ends the run as the fatal determinism violation.
    assert_every_interleaving_conflicts(|store| trace_scenario(TRACE_5_FAIL, store).chat(&gateway));
}

#[test]
fn one_root_identity_spans_the_h1_pass_and_the_walk() {
    assert_every_interleaving_reads(
        |store| trace_scenario(TRACE_7, store),
        "configured/task wrote",
    );
}

#[test]
fn a_timed_join_joins_the_members_it_delivers_and_a_later_join_the_rest() {
    // The delivered half: whichever join delivers the slow arm (the
    // timed join, or the later join), its write is readable, and the
    // quick arm's ordinary-join delivery is joined the same way.
    assert_every_interleaving_reads(
        |store| trace_scenario(TRACE_10_PASS, store),
        "quick arm|slow arm",
    );

    // The late-member half: before a later join delivers the late arm,
    // reading its file always fails - the timeout provably wins, because
    // the shuffled rig holds the arm's model round while it answers the
    // timer.
    assert_every_interleaving_conflicts(|store| {
        trace_scenario(TRACE_10_FAIL, store).hold(asks("slow"))
    });
}

#[test]
fn a_cancelled_tasks_partial_write_is_readable_after_the_join() {
    // The shuffled rig holds the task's model round, so the cancel lands
    // while the round is out.
    assert_every_interleaving_reads(
        |store| trace_scenario(TRACE_11, store).hold(asks("slow")),
        "partial",
    );
}

#[test]
fn every_interleaving_starts_from_an_empty_store() {
    assert_every_interleaving_reads(|store| trace_scenario(STORE_PROBE, store), "absent");
}

#[path = "happens_before-concurrency.rs"]
mod concurrency;
