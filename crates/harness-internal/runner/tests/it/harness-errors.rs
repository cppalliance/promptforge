//! What `run` returns when the run cannot reach an outcome of its own: a
//! model list the broker cannot fetch is `HarnessError::Model`, a cancel
//! before `run` or while the broker holds its list is a cancelled report
//! with no run, a
//! recorder that refuses a write is `HarnessError::Recorder` holding the
//! run id it issued, and a broker that panics answers its round
//! `Dropped`.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::task::Poll;

use harness_plugins::{HostServices, PluginRegistry};
use harness_runner::display_chain;
use harness_runner::environment::CurrentModelError;
use harness_runner::recorder::MemoryRecorder;
use harness_runner::{Harness, HarnessError};
use promptforge::model::{Completion, CompletionError, CompletionErrorKind};

use super::{
    OutputError, PendingTimer, RunOutcome, answers_to, plain_harness, request, run_beside, until,
};
use crate::scripted::{Listing, ScriptedBroker};
use crate::support::FailingRecorder;

/// A Plugin-free prompt whose one section returns a constant.
const PLAIN: &str = "---\nname: plain\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nreturn 'plain'\n```\n";

/// A prompt of one store write and one read.
const STORES: &str = "---\nname: stores\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\nstore.write('a.md', '1')\nreturn store.read('a.md')\n```\n";

/// A prompt whose one round sits under a `pcall` that reports how it
/// ended.
const CATCHES_A_ROUND: &str = "---\nname: catches\ndescription: d\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Title\n\n```lua\nmodels.default('writer')\n```\n\n## Only\n\n```lua\n\
    local ok, err = pcall(models.infer, 'asked')\n\
    return tostring(ok) .. ':' .. err.kind\n```\n";

/// A Harness over `recorder` and `broker` with no Plugins and no
/// Host services.
fn harness_over(recorder: &Arc<FailingRecorder>, broker: ScriptedBroker) -> Harness {
    Harness::new(
        recorder.clone(),
        Arc::new(broker),
        Arc::new(PendingTimer::default()),
        PluginRegistry::new(),
        HostServices::new(),
    )
}

#[tokio::test]
async fn a_model_list_the_broker_cannot_fetch_returns_the_model_error_and_begins_no_run() {
    let recorder = Arc::new(FailingRecorder::never_failing());
    let broker = ScriptedBroker::replying().listing(Listing::Fails(CompletionErrorKind::Transport));

    let error = harness_over(&recorder, broker)
        .run(request(PLAIN))
        .await
        .expect_err("a failed listing is the run's error");
    assert!(
        matches!(
            error,
            HarnessError::Model(CurrentModelError::CatalogFetchFailed(_))
        ),
        "{error:?}"
    );
    assert!(
        display_chain(&error).contains(CompletionErrorKind::Transport.phrase()),
        "the chain carries the broker's message: {}",
        display_chain(&error)
    );
    assert_eq!(recorder.calls(), 0, "the run never reached the recorder");
}

#[tokio::test]
async fn a_cancel_while_the_broker_holds_its_list_returns_a_cancelled_report_with_no_run() {
    let recorder = Arc::new(FailingRecorder::never_failing());
    let broker = ScriptedBroker::replying().listing(Listing::Holds);
    let listings = broker.listings();

    let report = run_beside(
        harness_over(&recorder, broker),
        request(PLAIN),
        |control| async move {
            until("the run asks for the model list", || {
                listings.load(Ordering::SeqCst) == 1
            })
            .await;
            control.cancel();
        },
    )
    .await
    .expect("a cancel before the run begins is an outcome");
    assert_eq!(report.run_id, None, "no run began");
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(report.output, Err(OutputError::NotCompleted));
    assert_eq!(recorder.calls(), 0, "the run never reached the recorder");
}

#[tokio::test]
async fn a_cancel_before_run_returns_a_cancelled_report_with_no_run_against_a_ready_listing() {
    let recorder = Arc::new(FailingRecorder::never_failing());
    let harness = harness_over(&recorder, ScriptedBroker::replying());
    harness.control().cancel();

    let report = harness
        .run(request(PLAIN))
        .await
        .expect("a cancel before the run begins is an outcome");
    assert_eq!(report.run_id, None, "no run began");
    assert_eq!(report.outcome, RunOutcome::Cancelled);
    assert_eq!(report.output, Err(OutputError::NotCompleted));
    assert_eq!(recorder.calls(), 0, "the run never reached the recorder");
}

#[tokio::test]
async fn a_recorder_failure_returns_the_recorder_error_holding_the_issued_run_id() {
    let refuses_begin = Arc::new(FailingRecorder::failing_on(1));
    let error = harness_over(&refuses_begin, ScriptedBroker::replying())
        .run(request(STORES))
        .await
        .expect_err("a refused begin is the run's error");
    let HarnessError::Recorder { run, .. } = error else {
        panic!("the refusal is the recorder's: {error:?}");
    };
    assert_eq!(run, None, "a refused begin issued no run");

    // Every later call names the run the recorder issued: the parse's
    // events in preparation, the loop's records, and the run's end.
    let baseline = Arc::new(FailingRecorder::never_failing());
    harness_over(&baseline, ScriptedBroker::replying())
        .run(request(STORES))
        .await
        .expect("the baseline run reaches an outcome");
    let total = baseline.calls();
    assert!(
        total >= 6,
        "begin, events, two effects, two answers, and the end"
    );
    for call in 2..=total {
        let recorder = Arc::new(FailingRecorder::failing_on(call));
        let error = harness_over(&recorder, ScriptedBroker::replying())
            .run(request(STORES))
            .await
            .expect_err("a refused write is the run's error");
        let HarnessError::Recorder { run, source } = error else {
            panic!("call {call} of {total}: the refusal is the recorder's: {error:?}");
        };
        let issued = recorder.begun();
        assert_eq!(issued.len(), 1, "call {call} of {total}: one run began");
        assert_eq!(
            run,
            Some(issued[0]),
            "call {call} of {total}: the error holds the issued run id"
        );
        assert!(
            display_chain(&source).contains(&format!("refuses call {call}")),
            "call {call} of {total}: {}",
            display_chain(&source)
        );
        assert_eq!(recorder.calls(), call, "nothing is sent after the refusal");
    }
}

#[tokio::test]
async fn a_panicking_broker_answers_its_round_dropped() {
    let in_the_call = ScriptedBroker::new(|_round, _messages| {
        panic!("the broker panics in its call");
    });
    let in_the_round = ScriptedBroker::new(|_round, _messages| {
        Box::pin(std::future::poll_fn(
            |_cx| -> Poll<Result<Box<Completion>, CompletionError>> {
                panic!("the broker panics in its round")
            },
        ))
    });
    for (what, broker) in [("call", in_the_call), ("round", in_the_round)] {
        let recorder = Arc::new(MemoryRecorder::new());
        let report = plain_harness(&recorder, broker)
            .run(request(CATCHES_A_ROUND))
            .await
            .expect("a panicking broker does not end the run with an error");
        assert_eq!(
            report.outcome,
            RunOutcome::Completed {
                final_text: "false:cancelled".to_owned()
            },
            "a panic in the {what}: the pcall caught the dropped round"
        );
        let records = recorder.records(report.run_id.expect("the run began"));
        assert_eq!(
            answers_to(&records, "Chat"),
            [serde_json::json!("Dropped")],
            "a panic in the {what}: the round is answered Dropped"
        );
    }
}
