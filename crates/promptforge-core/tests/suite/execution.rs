//! Section execution over offline fixtures: author checkpoints, prologue
//! return, store fall-through, concurrent execution-id isolation, and the
//! typed execution-error contract.

use std::collections::BTreeSet;
use std::sync::Arc;

use promptforge_core::execute::RunErrorKind;
use promptforge_core::observe::Observer;
use promptforge_core::store::StoreRef;

use super::support::{Record, Recorder, RunOptions, parse_execution_fixture, run, run_fixture};

const LOG_EXECUTION: &str = "fixture-log-checkpoints";
const PROLOGUE_EXECUTION: &str = "fixture-prologue-return";
const STORE_EXECUTION: &str = "fixture-store-fallthrough";
const REPLY_NIL_EXECUTION: &str = "fixture-reply-nil";
const STORE_TRIAD_EXECUTION: &str = "fixture-store-triad";
const REPLY_SUBST_NIL_EXECUTION: &str = "fixture-reply-subst-nil";
const ITEM_OUTSIDE_EXECUTION: &str = "fixture-item-outside";

const LOG_CHECKPOINTS: &str = include_str!("../prompts/execution/log-checkpoints.md");
const PROLOGUE_RETURN: &str = include_str!("../prompts/execution/prologue-return.md");
const STORE_FALLTHROUGH: &str = include_str!("../prompts/execution/store-fallthrough.md");
const REPLY_NIL_SECTION_ONE: &str =
    include_str!("../prompts/execution/reply-nil-section-one.md");
const STORE_TRIAD: &str = include_str!("../prompts/execution/store-triad.md");
const REPLY_SUBSTITUTION_NIL: &str =
    include_str!("../prompts/invalid/reply-substitution-nil.md");
const ITEM_OUTSIDE_FANOUT: &str = include_str!("../prompts/invalid/item-outside-fanout.md");

struct ExecutionErrorFixture {
    name: &'static str,
    source: &'static str,
    execution: &'static str,
    kind: RunErrorKind,
    message_fragment: &'static str,
}

const EXECUTION_ERROR_FIXTURES: &[ExecutionErrorFixture] = &[
    ExecutionErrorFixture {
        name: "invalid/reply-substitution-nil.md",
        source: REPLY_SUBSTITUTION_NIL,
        execution: REPLY_SUBST_NIL_EXECUTION,
        kind: RunErrorKind::Substitution,
        message_fragment: "nil",
    },
    ExecutionErrorFixture {
        name: "invalid/item-outside-fanout.md",
        source: ITEM_OUTSIDE_FANOUT,
        execution: ITEM_OUTSIDE_EXECUTION,
        kind: RunErrorKind::Substitution,
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

#[tokio::test]
async fn log_fixture_reports_exact_author_checkpoints() {
    let run = run_fixture(
        LOG_CHECKPOINTS,
        "execution/log-checkpoints.md",
        LOG_EXECUTION,
        "",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the log checkpoint fixture must execute offline");

    assert_eq!(result, "logged");
    assert_eq!(
        run.store
            .read("state.txt")
            .expect("the prepare section writes state"),
        "prepared"
    );
    assert_eq!(
        checkpoints(&run.recorder.records(), LOG_EXECUTION),
        [
            Record::new(LOG_EXECUTION, "Log Checkpoints", "Lua: shared loaded"),
            Record::new(LOG_EXECUTION, "Prepare", "Lua: prepare started"),
            Record::new(LOG_EXECUTION, "Prepare", "Lua: prepare finished"),
            Record::new(LOG_EXECUTION, "Finish", "Lua: finish started"),
        ]
    );
}

#[tokio::test]
async fn prologue_return_fixture_skips_model_and_epilog() {
    let run = run_fixture(
        PROLOGUE_RETURN,
        "execution/prologue-return.md",
        PROLOGUE_EXECUTION,
        "early result",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the prologue return fixture must execute without a model");

    assert_eq!(result, "early result");
    assert!(
        run.store.read("unreachable.txt").is_err(),
        "the epilog after a scalar prologue return must not run"
    );
    assert_eq!(
        checkpoints(&run.recorder.records(), PROLOGUE_EXECUTION),
        [Record::new(
            PROLOGUE_EXECUTION,
            "Stop Early",
            "Lua: returning early"
        )]
    );
}

#[tokio::test]
async fn store_fixture_persists_state_across_fall_through() {
    let run = run_fixture(
        STORE_FALLTHROUGH,
        "execution/store-fallthrough.md",
        STORE_EXECUTION,
        "carried value",
        None,
    )
    .await;
    let result = run
        .result
        .expect("the store fall-through fixture must execute offline");

    assert_eq!(result, "carried value");
    assert_eq!(
        run.store
            .read("handoff.txt")
            .expect("the handoff remains stored"),
        "carried value"
    );
    assert_eq!(
        checkpoints(&run.recorder.records(), STORE_EXECUTION),
        [
            Record::new(STORE_EXECUTION, "Write", "Lua: writing state"),
            Record::new(STORE_EXECUTION, "Read", "Lua: reading state"),
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_runs_keep_execution_ids_separate() {
    const FIRST: &str = "fixture-concurrent-first";
    const SECOND: &str = "fixture-concurrent-second";

    let recorder = Arc::new(Recorder::default());
    let barrier = Arc::new(tokio::sync::Barrier::new(2));
    let first_prompt = Arc::new(parse_execution_fixture(
        PROLOGUE_RETURN,
        "execution/prologue-return.md",
        FIRST,
        recorder.as_ref(),
    ));
    let second_prompt = Arc::new(parse_execution_fixture(
        PROLOGUE_RETURN,
        "execution/prologue-return.md",
        SECOND,
        recorder.as_ref(),
    ));

    let first_recorder = Arc::clone(&recorder);
    let first_barrier = Arc::clone(&barrier);
    let first = tokio::spawn(async move {
        first_barrier.wait().await;
        run(
            first_prompt.as_ref(),
            "first result",
            &[],
            &StoreRef::memory(),
            RunOptions {
                execution: FIRST,
                observer: Arc::clone(&first_recorder) as Arc<dyn Observer>,
            },
        )
        .await
    });

    let second_recorder = Arc::clone(&recorder);
    let second = tokio::spawn(async move {
        barrier.wait().await;
        run(
            second_prompt.as_ref(),
            "second result",
            &[],
            &StoreRef::memory(),
            RunOptions {
                execution: SECOND,
                observer: Arc::clone(&second_recorder) as Arc<dyn Observer>,
            },
        )
        .await
    });

    assert_eq!(
        first
            .await
            .expect("the first fixture task must join")
            .expect("the first fixture run must succeed"),
        "first result"
    );
    assert_eq!(
        second
            .await
            .expect("the second fixture task must join")
            .expect("the second fixture run must succeed"),
        "second result"
    );

    let records = recorder.records();
    assert_eq!(
        records
            .iter()
            .map(|record| record.execution.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([FIRST, SECOND]),
        "the shared recorder must retain only the two caller-provided ids"
    );
    for execution in [FIRST, SECOND] {
        assert_eq!(
            checkpoints(&records, execution),
            [Record::new(execution, "Stop Early", "Lua: returning early")],
            "each concurrent run must retain its own checkpoint"
        );
        assert!(
            records.iter().any(|record| {
                record.execution == execution
                    && record.section == "Prologue Return"
                    && record.detail == "Run succeeded"
            }),
            "{execution} must retain its own terminal run record"
        );
    }
}

#[tokio::test]
async fn reply_nil_in_section_one() {
    let run = run_fixture(
        REPLY_NIL_SECTION_ONE,
        "execution/reply-nil-section-one.md",
        REPLY_NIL_EXECUTION,
        "",
        None,
    )
    .await;
    assert_eq!(
        run.result
            .expect("the reply nil fixture must execute offline"),
        "section one done"
    );
}

#[tokio::test]
async fn store_triad_numbered_vs_verbatim_vs_wrapped() {
    let run = run_fixture(
        STORE_TRIAD,
        "execution/store-triad.md",
        STORE_TRIAD_EXECUTION,
        "",
        None,
    )
    .await;
    assert_eq!(
        run.result
            .expect("the store triad fixture must execute offline"),
        "1| alpha\n2| beta|alpha\nbeta"
    );
}

#[tokio::test]
async fn execution_error_fixtures_report_their_typed_contract() {
    for fixture in EXECUTION_ERROR_FIXTURES {
        let run = run_fixture(fixture.source, fixture.name, fixture.execution, "", None).await;
        // Format the fixture name only on the unexpected-success path so the
        // assertion never allocates on the common error path.
        let error = match run.result {
            Ok(value) => panic!(
                "fixture {} must fail at execution, got {value:?}",
                fixture.name
            ),
            Err(error) => error,
        };
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
