//! Concurrent fanout: per-arm start/terminal accounting, store writes across
//! arms, and the propagated arm-failure error contract.

use crate::RunErrorKind;
use crate::test_support::scenario::{Record, Scenario};

const FANOUT_BASIC_EXECUTION: &str = "fixture-fanout-basic";
const FANOUT_EPILOG_EXECUTION: &str = "fixture-fanout-epilog";
const FANOUT_STORE_EXECUTION: &str = "fixture-fanout-store";
const FANOUT_FAILURE_EXECUTION: &str = "fixture-fanout-failure";
const FANOUT_CROSS_ARM_EXECUTION: &str = "fixture-fanout-cross-arm-append";

const FANOUT_BASIC: &str = include_str!("../../../../tests/prompts/execution/fanout-basic.md");
const FANOUT_EPILOG: &str = include_str!("../../../../tests/prompts/execution/fanout-epilog.md");
const FANOUT_STORE_WRITES: &str =
    include_str!("../../../../tests/prompts/execution/fanout-store-writes.md");
const FANOUT_ARM_FAILURE: &str =
    include_str!("../../../../tests/prompts/execution/fanout-arm-failure.md");
const FANOUT_CROSS_ARM_APPEND: &str =
    include_str!("../../../../tests/prompts/execution/fanout-cross-arm-append.md");

/// The section that calls `fanout` in the two-arm fixtures: an arm is a
/// task the shim spawns, so its `Task started` reports under the caller.
const CALLER_SECTION: &str = "Research";

/// The worker-template section name both fanout arms execute under: an
/// arm's terminal task observation reports under it. The stream keys arm
/// events by section, not by `sys.index` (which the runtime injects only
/// into arm Lua), so the exact per-arm index pairing is proven by the arms'
/// index-bearing result rather than the event stream.
const WORKER_SECTION: &str = "Worker";

/// Asserts the caller section started exactly two tasks, the worker section
/// reported exactly two task successes, and no other task terminal (failed,
/// cancelled, or abandoned) fired anywhere.
fn assert_two_arms_all_succeeded(records: &[Record]) {
    let events: Vec<(&str, &str)> = records
        .iter()
        .filter(|record| record.detail.starts_with("Task "))
        .map(|record| (record.section.as_str(), record.detail.as_str()))
        .collect();
    let started = events
        .iter()
        .filter(|(section, detail)| *section == CALLER_SECTION && *detail == "Task started")
        .count();
    let succeeded = events
        .iter()
        .filter(|(section, detail)| *section == WORKER_SECTION && *detail == "Task succeeded")
        .count();
    assert_eq!(
        started, 2,
        "two arms must start under the caller section: {events:?}"
    );
    assert_eq!(
        succeeded, 2,
        "two arms must succeed under the worker section: {events:?}"
    );
    assert_eq!(
        events.len(),
        started + succeeded,
        "each arm must pair one start with one success and emit no failed, cancelled, or abandoned event: {events:?}"
    );
}

#[test]
fn fanout_basic_two_items_prologue_return() {
    let trace = Scenario::new(FANOUT_BASIC)
        .name(FANOUT_BASIC_EXECUTION)
        .run();
    // The index-bearing output pins each arm's `item .. '-' .. sys.index`.
    assert_eq!(trace.text(), "alpha-1\nbeta-2");
    assert_two_arms_all_succeeded(&trace.records());
}

#[test]
fn fanout_epilog_two_items() {
    let trace = Scenario::new(FANOUT_EPILOG)
        .name(FANOUT_EPILOG_EXECUTION)
        .run();
    assert_eq!(trace.text(), "x-1,y-2");
    assert_two_arms_all_succeeded(&trace.records());
}

#[test]
fn fanout_store_writes_persist_across_arms() {
    // Arm-scoped writes under happens-before: each arm writes only its
    // own path, so no two unordered identities ever claim one path, and
    // the parent's post-join glob sees the merged state because the
    // fanout's join_any rounds join every arm before it returns.
    // Interleaving coverage lives in the scheduler's
    // `fanout_arms_interleave_at_io_points_on_one_thread`.
    let trace = Scenario::new(FANOUT_STORE_WRITES)
        .name(FANOUT_STORE_EXECUTION)
        .run();
    // Both prologue-only arms write distinct paths and the parent reply vector
    // stays list-ordered.
    assert_eq!(trace.text(), "2:alpha,beta");
    assert_eq!(trace.read("arm-1.md").expect("arm 1 must write"), "alpha");
    assert_eq!(trace.read("arm-2.md").expect("arm 2 must write"), "beta");
    // The ordered merge: the join's collection-order results land in one
    // parent-written file, deterministic by construction.
    assert_eq!(
        trace.read("merged.md").expect("the merge must land"),
        "alpha,beta"
    );
}

#[test]
fn a_cross_arm_append_terminates_the_run_with_a_determinism_violation() {
    // The arms append unordered - neither joins the other before the
    // fanout's own rounds - and claims are never released during a run,
    // so the second append's claim check meets the first's standing
    // write claim in every interleaving. The fixture's pcall proves the
    // violation is uncatchable: were it resumed into the arm, the
    // handler would record the catch and the run would return
    // "alpha,beta" instead of failing.
    let trace = Scenario::new(FANOUT_CROSS_ARM_APPEND)
        .name(FANOUT_CROSS_ARM_EXECUTION)
        .run();
    let error = trace.error();
    assert_eq!(
        error.kind(),
        RunErrorKind::Determinism,
        "a conflicting access classifies as a determinism violation: {error:?}"
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
    let evidence = trace.read("evidence.md").expect("one arm appended");
    assert!(
        evidence == "alpha\n" || evidence == "beta\n",
        "exactly one arm's append may land: {evidence:?}"
    );
}

#[test]
fn fanout_arm_failure_propagates() {
    let trace = Scenario::new(FANOUT_ARM_FAILURE)
        .name(FANOUT_FAILURE_EXECUTION)
        .run();
    let error = trace.error();
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
