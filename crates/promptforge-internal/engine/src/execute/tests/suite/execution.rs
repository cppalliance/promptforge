//! Section execution over offline fixtures: author checkpoints, prologue
//! return, store fall-through, concurrent execution-id isolation, and the
//! typed execution-error contract.

use std::collections::BTreeSet;

use crate::RunErrorKind;
use crate::test_support::recording::detail;
use crate::test_support::scenario::{Record, Scenario};

const LOG_EXECUTION: &str = "fixture-log-checkpoints";
const PROLOGUE_EXECUTION: &str = "fixture-prologue-return";
const STORE_EXECUTION: &str = "fixture-store-fallthrough";
const STORE_TRIAD_EXECUTION: &str = "fixture-store-triad";
const REPLY_SUBST_NIL_EXECUTION: &str = "fixture-reply-subst-nil";
const ITEM_OUTSIDE_EXECUTION: &str = "fixture-item-outside";

const LOG_CHECKPOINTS: &str =
    include_str!("../../../../tests/prompts/execution/log-checkpoints.md");
const PROLOGUE_RETURN: &str =
    include_str!("../../../../tests/prompts/execution/prologue-return.md");
const STORE_FALLTHROUGH: &str =
    include_str!("../../../../tests/prompts/execution/store-fallthrough.md");
const STORE_TRIAD: &str = include_str!("../../../../tests/prompts/execution/store-triad.md");
const REPLY_SUBSTITUTION_NIL: &str =
    include_str!("../../../../tests/prompts/invalid/reply-substitution-nil.md");
const ITEM_OUTSIDE_FANOUT: &str =
    include_str!("../../../../tests/prompts/invalid/item-outside-fanout.md");

struct ExecutionErrorFixture {
    name: &'static str,
    source: &'static str,
    execution: &'static str,
    kind: RunErrorKind,
    message_fragment: &'static str,
}

const EXECUTION_ERROR_FIXTURES: &[ExecutionErrorFixture] = &[
    // Both fixtures fail at the lazy-`prose` read site inside the section's
    // Lua, so the run-level kind is Lua: the substitution error is an
    // ordinary, pcall-able Lua error there, not a run-phase failure.
    ExecutionErrorFixture {
        name: "invalid/reply-substitution-nil.md",
        source: REPLY_SUBSTITUTION_NIL,
        execution: REPLY_SUBST_NIL_EXECUTION,
        kind: RunErrorKind::Lua,
        message_fragment: "reply",
    },
    ExecutionErrorFixture {
        name: "invalid/item-outside-fanout.md",
        source: ITEM_OUTSIDE_FANOUT,
        execution: ITEM_OUTSIDE_EXECUTION,
        kind: RunErrorKind::Lua,
        message_fragment: "nil",
    },
];

fn checkpoints(records: &[Record], execution: &str) -> Vec<Record> {
    records
        .iter()
        .filter(|record| record.execution == execution && record.detail.starts_with("Lua: "))
        .cloned()
        .collect()
}

#[test]
fn log_fixture_reports_exact_author_checkpoints() {
    let trace = Scenario::new(LOG_CHECKPOINTS).name(LOG_EXECUTION).run();
    assert_eq!(trace.text(), "logged");
    assert_eq!(
        trace
            .read("state.txt")
            .expect("the prepare section writes state"),
        "prepared"
    );
    assert_eq!(
        checkpoints(&trace.records(), LOG_EXECUTION),
        [
            Record::new(LOG_EXECUTION, "Log Checkpoints", "Lua: H1 ran"),
            Record::new(LOG_EXECUTION, "Prepare", "Lua: prepare started"),
            Record::new(LOG_EXECUTION, "Prepare", "Lua: prepare finished"),
            Record::new(LOG_EXECUTION, "Finish", "Lua: finish started"),
        ]
    );
}

#[test]
fn prologue_return_fixture_skips_model_and_epilog() {
    let trace = Scenario::new(PROLOGUE_RETURN)
        .name(PROLOGUE_EXECUTION)
        .args("early result")
        .run();
    assert_eq!(trace.text(), "early result");
    assert!(
        trace.read("unreachable.txt").is_err(),
        "the epilog after a scalar prologue return must not run"
    );
    assert_eq!(
        checkpoints(&trace.records(), PROLOGUE_EXECUTION),
        [Record::new(
            PROLOGUE_EXECUTION,
            "Stop Early",
            "Lua: returning early"
        )]
    );
}

#[test]
fn store_fixture_persists_state_across_fall_through() {
    let trace = Scenario::new(STORE_FALLTHROUGH)
        .name(STORE_EXECUTION)
        .args("handoff value")
        .run();
    assert_eq!(trace.text(), "handoff value");
    assert_eq!(
        trace
            .read("handoff.txt")
            .expect("the handoff remains stored"),
        "handoff value"
    );
    assert_eq!(
        checkpoints(&trace.records(), STORE_EXECUTION),
        [
            Record::new(STORE_EXECUTION, "Write", "Lua: writing state"),
            Record::new(STORE_EXECUTION, "Read", "Lua: reading state"),
        ]
    );
}

#[test]
fn concurrent_runs_keep_execution_ids_separate() {
    const FIRST: &str = "fixture-concurrent-first";
    const SECOND: &str = "fixture-concurrent-second";

    let start = |name, args| {
        Scenario::new(STORE_FALLTHROUGH)
            .name(name)
            .args(args)
            .start()
    };
    let (mut first, mut second) = (start(FIRST, "first result"), start(SECOND, "second result"));
    // Both runs take every step on this thread, one step of each in turn.
    assert!(first.step(), "the first run is pending on its store write");
    assert!(
        second.step(),
        "the second run starts and parks while the first is still pending"
    );
    while first.step() | second.step() {}
    let (first, second) = (first.finish(), second.finish());
    assert_eq!(first.text(), "first result");
    assert_eq!(second.text(), "second result");

    for (execution, trace) in [(FIRST, &first), (SECOND, &second)] {
        let records = trace.records();
        assert_eq!(
            records
                .iter()
                .map(|record| record.execution.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([execution]),
            "each run's records must carry only its own caller-provided id"
        );
        assert_eq!(
            checkpoints(&records, execution),
            [
                Record::new(execution, "Write", "Lua: writing state"),
                Record::new(execution, "Read", "Lua: reading state"),
            ],
            "each interleaved run must retain its own checkpoints"
        );
        assert!(
            records.iter().any(|record| {
                record.section == "Store Fall-through"
                    && record.detail == detail::RUN_SUCCEEDED.to_string()
            }),
            "{execution} must retain its own terminal run record"
        );
    }
}

#[test]
fn store_triad_numbered_vs_verbatim_vs_wrapped() {
    let trace = Scenario::new(STORE_TRIAD).name(STORE_TRIAD_EXECUTION).run();
    assert_eq!(trace.text(), "1| alpha\n2| beta|alpha\nbeta");
}

#[test]
fn execution_error_fixtures_report_their_typed_contract() {
    for fixture in EXECUTION_ERROR_FIXTURES {
        let trace = Scenario::new(fixture.source).name(fixture.execution).run();
        let error = trace.error();
        assert_eq!(
            error.kind(),
            fixture.kind,
            "fixture {} returned the wrong error kind: {error:?}",
            fixture.name
        );
        assert!(
            error.to_string().contains(fixture.message_fragment),
            "fixture {} error did not contain {:?}: {error}",
            fixture.name,
            fixture.message_fragment
        );
    }
}
