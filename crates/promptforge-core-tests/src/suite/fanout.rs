//! Concurrent fanout: per-arm start/terminal accounting, store writes across
//! arms, and the propagated arm-failure error contract.

use std::time::Duration;

use promptforge_core::execute::RunErrorKind;

use super::support::{Record, run_fixture};

const FANOUT_BASIC_EXECUTION: &str = "fixture-fanout-basic";
const FANOUT_EPILOG_EXECUTION: &str = "fixture-fanout-epilog";
const FANOUT_STORE_EXECUTION: &str = "fixture-fanout-store";
const FANOUT_FAILURE_EXECUTION: &str = "fixture-fanout-failure";

const FANOUT_BASIC: &str = include_str!("../../prompts/execution/fanout-basic.md");
const FANOUT_EPILOG: &str = include_str!("../../prompts/execution/fanout-epilog.md");
const FANOUT_STORE_WRITES: &str = include_str!("../../prompts/execution/fanout-store-writes.md");
const FANOUT_ARM_FAILURE: &str = include_str!("../../prompts/execution/fanout-arm-failure.md");

/// The worker-template section name both fanout arms execute under. The
/// observation stream keys arm events by this section, not by `sys.taskid`
/// (which the runtime injects only into arm Lua), so the exact per-arm task-id
/// pairing is proven by the arms' task-id-bearing result rather than the event
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

    // The task-id-bearing output pins each arm's `item .. '-' .. sys.taskid`.
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
    // The arms rendezvous by writing and polling ready-*.md, so concurrency is
    // proven by both ready markers and both arm writes existing. The timeout is
    // only a safety net against a sequential-fanout regression deadlocking the
    // rendezvous; it is not the pass condition.
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
    .expect("concurrent fanout must finish; a sequential regression would hang on the rendezvous");
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
    assert_eq!(
        run.store
            .read("ready-1.md")
            .expect("arm 1 rendezvous marker"),
        "1"
    );
    assert_eq!(
        run.store
            .read("ready-2.md")
            .expect("arm 2 rendezvous marker"),
        "1"
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
