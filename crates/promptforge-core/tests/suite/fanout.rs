//! Concurrent fanout: per-arm start/terminal accounting, store writes across
//! arms, and the propagated arm-failure error contract.

use std::time::Duration;

use promptforge_core::execute::RunErrorKind;

use super::support::{Record, run_fixture};

const FANOUT_BASIC_EXECUTION: &str = "fixture-fanout-basic";
const FANOUT_EPILOG_EXECUTION: &str = "fixture-fanout-epilog";
const FANOUT_STORE_EXECUTION: &str = "fixture-fanout-store";
const FANOUT_FAILURE_EXECUTION: &str = "fixture-fanout-failure";
const FANOUT_CROSS_ARM_EXECUTION: &str = "fixture-fanout-cross-arm-append";

const FANOUT_BASIC: &str = include_str!("../prompts/execution/fanout-basic.md");
const FANOUT_EPILOG: &str = include_str!("../prompts/execution/fanout-epilog.md");
const FANOUT_STORE_WRITES: &str = include_str!("../prompts/execution/fanout-store-writes.md");
const FANOUT_ARM_FAILURE: &str = include_str!("../prompts/execution/fanout-arm-failure.md");
const FANOUT_CROSS_ARM_APPEND: &str =
    include_str!("../prompts/execution/fanout-cross-arm-append.md");

/// The worker-template section name both fanout arms execute under. The
/// observation stream keys arm events by this section, not by `sys.index`
/// (which the runtime injects only into arm Lua), so the exact per-arm index
/// pairing is proven by the arms' index-bearing result rather than the event
/// stream.
const WORKER_SECTION: &str = "Worker";

/// Asserts the worker section emitted exactly one start and one success per arm
/// and no other arm terminal (failed, cancelled, exhausted, or the legacy
/// generic finished).
fn assert_two_arms_all_succeeded(records: &[Record]) {
    let events: Vec<&str> = records
        .iter()
        .filter(|record| {
            record.section == WORKER_SECTION && record.detail.starts_with("Fanout arm ")
        })
        .map(|record| record.detail.as_str())
        .collect();
    let started = events
        .iter()
        .filter(|detail| **detail == "Fanout arm started")
        .count();
    let succeeded = events
        .iter()
        .filter(|detail| **detail == "Fanout arm succeeded")
        .count();
    assert_eq!(
        started, 2,
        "two arms must start under the worker section: {events:?}"
    );
    assert_eq!(
        succeeded, 2,
        "two arms must succeed under the worker section: {events:?}"
    );
    assert_eq!(
        events.len(),
        started + succeeded,
        "each arm must pair one start with one success and emit no failed, cancelled, or exhausted event: {events:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_basic_two_items_prologue_return() {
    let run = run_fixture(
        FANOUT_BASIC,
        "execution/fanout-basic.md",
        FANOUT_BASIC_EXECUTION,
        "",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the fanout basic fixture must execute offline");

    // The index-bearing output pins each arm's `item .. '-' .. sys.index`.
    assert_eq!(result, "alpha-1\nbeta-2");
    assert_two_arms_all_succeeded(&run.recorder.records());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_epilog_two_items() {
    let run = run_fixture(
        FANOUT_EPILOG,
        "execution/fanout-epilog.md",
        FANOUT_EPILOG_EXECUTION,
        "",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the fanout epilog fixture must execute offline");

    assert_eq!(result, "x-1,y-2");
    assert_two_arms_all_succeeded(&run.recorder.records());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_store_writes_persist_across_arms() {
    // Arm-scoped writes under the claims model: each arm writes only its
    // own path, so no two live identities ever claim one path, and the
    // parent's post-join glob sees the merged state because a finished
    // arm's claims release at chain end. (The fixture's old ready-*.md
    // rendezvous polled a live sibling's writes - precisely the cross-arm
    // read-while-written pattern the claims model rejects - so it was
    // removed; interleaving coverage lives in the scheduler's
    // `fanout_arms_interleave_at_io_points_on_one_thread`.)
    let run = tokio::time::timeout(
        Duration::from_secs(30),
        run_fixture(
            FANOUT_STORE_WRITES,
            "execution/fanout-store-writes.md",
            FANOUT_STORE_EXECUTION,
            "",
            None,
        ),
    )
    .await
    .expect("the fanout fixture completes");
    let result = run
        .result
        .expect("the fanout store fixture must execute offline");

    // Both prologue-only arms write distinct paths and the parent reply vector
    // stays list-ordered.
    assert_eq!(result, "2:alpha,beta");
    assert_eq!(
        run.store.read("arm-1.md").expect("arm 1 must write"),
        "alpha"
    );
    assert_eq!(
        run.store.read("arm-2.md").expect("arm 2 must write"),
        "beta"
    );
    // The ordered merge: the join's collection-order results land in one
    // parent-written file, deterministic by construction.
    assert_eq!(
        run.store.read("merged.md").expect("the merge must land"),
        "alpha,beta"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cross_arm_append_terminates_the_run_with_a_determinism_violation() {
    // Every store operation is a leaf yield now, so two live arms appending
    // one path genuinely race in the blocking pool; the claims model booms
    // the loser and the violation fails the whole run at the answer
    // boundary. The fixture's pcall proves the violation is uncatchable:
    // were it resumed into the arm, the handler would record the catch and
    // the run would return "alpha,beta" instead of failing.
    let run = run_fixture(
        FANOUT_CROSS_ARM_APPEND,
        "execution/fanout-cross-arm-append.md",
        FANOUT_CROSS_ARM_EXECUTION,
        "",
        None,
    )
    .await;
    let error = match run.result {
        Ok(value) => panic!("a cross-arm append must terminate the run, got {value:?}"),
        Err(error) => error,
    };
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a claims conflict classifies as a determinism violation: {error:?}"
    );
    let text = error.to_string();
    assert!(
        text.contains("evidence.md"),
        "the violation names the contested path: {text}"
    );
    assert!(
        text.contains("conflicts with"),
        "the violation names the conflicting claim: {text}"
    );
    assert_eq!(
        text.matches("ExecId(").count(),
        2,
        "the violation names both arms' identities: {text}"
    );
    // The losing arm's append never reached the backend: exactly one arm's
    // line landed.
    let evidence = run.store.read("evidence.md").expect("one arm appended");
    assert!(
        evidence == "alpha\n" || evidence == "beta\n",
        "exactly one arm's append may land: {evidence:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fanout_arm_failure_propagates() {
    let run = run_fixture(
        FANOUT_ARM_FAILURE,
        "execution/fanout-arm-failure.md",
        FANOUT_FAILURE_EXECUTION,
        "",
        None,
    )
    .await;
    let error = match run.result {
        Ok(value) => panic!("the fanout arm failure must propagate, got {value:?}"),
        Err(error) => error,
    };

    // Assert the stable classification first, then the preserved source context.
    assert_eq!(
        error.kind(),
        RunErrorKind::Lua,
        "a raised arm error must classify as a Lua failure: {error:?}"
    );
    assert!(
        error.to_string().contains("deliberately failed"),
        "error must preserve the arm's message: {error}"
    );
}
