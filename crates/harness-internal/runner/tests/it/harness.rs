//! The per-run Harness: `run` resolves the launch model through the
//! Host's broker, prepares the request's source, drives the run as one
//! future, and reads the declared output; source that fails to parse is a
//! recorded failed run; a Vfs effect is answered inline as it is issued;
//! and one run completes under a thread-parking executor with no runtime
//! behind it. How a stop and a cancel reach the effects in flight sits in
//! the `stop` child module, and every error `run` returns in the `errors`
//! child module.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread, ThreadId};
use std::time::Duration;

use harness_capabilities::{CapabilityRegistry, HostServices};
use harness_runner::environment::HostSnapshot;
use harness_runner::files::OutputError;
use harness_runner::recorder::{MemoryRecorder, Record, RecordKind, RunOutcome};
use harness_runner::{Harness, HarnessError, RunControl, RunReport, RunRequest};
use promptforge::vfs::VfsRef;
use serde_json::{Value, json};

use crate::scripted::{Kept, MODEL, ScriptedBroker};
use crate::support::{HookedBackend, PendingTimer};

#[path = "harness-errors.rs"]
mod errors;
#[path = "harness-stop.rs"]
mod stop;

/// The request's name: every event's `execution` and the run's
/// `session_id`.
const NAME: &str = "harness-test";

/// How long a test waits for a run before it fails.
const PATIENCE: Duration = Duration::from_secs(10);

/// A request to run `source` under [`NAME`] with [`MODEL`] selected, no
/// arguments, no input text, and a fresh memory store.
fn request(source: &str) -> RunRequest {
    RunRequest {
        name: NAME.to_owned(),
        source: source.to_owned(),
        args: String::new(),
        input_text: None,
        vfs: VfsRef::default(),
        host: HostSnapshot {
            selected_model: Some(MODEL.to_owned()),
            ..HostSnapshot::default()
        },
    }
}

/// A Harness over `recorder` and `broker` with no capabilities, no Host
/// services, and a timer no test here reaches.
fn plain_harness(recorder: &Arc<MemoryRecorder>, broker: ScriptedBroker) -> Harness {
    Harness::new(
        recorder.clone(),
        Arc::new(broker),
        Arc::new(PendingTimer::default()),
        CapabilityRegistry::new(),
        HostServices::new(),
    )
}

/// Yields until `condition` holds, or fails naming `what`. Every
/// performer here is in memory, so the run beside the caller reaches each
/// state within a few polls.
async fn until(what: &str, condition: impl Fn() -> bool) {
    for _ in 0..10_000 {
        if condition() {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("{what}");
}

/// Runs `request` on `harness` beside `steer`, which is handed the run's
/// control, and returns the run's result.
async fn run_beside<F>(
    harness: Harness,
    request: RunRequest,
    steer: impl FnOnce(RunControl) -> F,
) -> Result<RunReport, HarnessError>
where
    F: Future<Output = ()>,
{
    let control = harness.control();
    let (result, ()) = tokio::time::timeout(PATIENCE, async {
        tokio::join!(harness.run(request), steer(control))
    })
    .await
    .expect("the run ends in time");
    result
}

/// The answer payloads of the effects whose record payload is keyed by
/// `kind` (`Chat`, `ToolCall`, `Timer`, `Vfs`) and whose body passes
/// `filter`, in effect order.
fn answers_where(records: &[Record], kind: &str, filter: impl Fn(&Value) -> bool) -> Vec<Value> {
    records
        .iter()
        .filter(|record| record.kind == RecordKind::Effect)
        .filter_map(|effect| {
            let body = effect.payload.get(kind)?;
            filter(body).then_some(effect.effect_id)
        })
        .map(|id| {
            records
                .iter()
                .find(|record| record.kind == RecordKind::Answer && record.effect_id == id)
                .expect("every effect is answered")
                .payload
                .clone()
        })
        .collect()
}

/// The answer payloads of every effect of `kind`, in effect order.
fn answers_to(records: &[Record], kind: &str) -> Vec<Value> {
    answers_where(records, kind, |_| true)
}

/// A prompt declaring `paper.md` as its input and `report.md` as its
/// output, whose one section writes the output from the input.
const FILES: &str = "---\nname: files\ndescription: d\npromptforge: 0\n\
    input:\n  path: paper.md\n  description: The paper\n\
    output:\n  path: report.md\n  description: The report\n---\n\n\
    # Title\n\n## Only\n\n```lua\n\
    store.write('report.md', 'seen: ' .. store.read('paper.md'))\nreturn 'done'\n```\n";

/// A prompt with unclosed frontmatter, so it does not parse.
const UNCLOSED: &str = "---\nname: unclosed\ndescription: d\npromptforge: 0\n\n# Title\n";

/// A prompt that writes two store files and reads them back.
const WRITES_TWICE: &str = "---\nname: writes\ndescription: d\npromptforge: 0\n---\n\n\
    # Title\n\n## Only\n\n```lua\n\
    store.write('a.md', '1')\nstore.write('b.md', '2')\n\
    return store.read('a.md') .. store.read('b.md')\n```\n";

/// A prompt that writes a store file, infers once through its one role,
/// and returns the reply beside the file.
const INFERS_AND_STORES: &str = "---\nname: infers\ndescription: d\npromptforge: 0\n\
    models:\n  writer: {}\n---\n\n\
    # Title\n\n```lua\nmodels.default('writer')\n```\n\n## Only\n\n```lua\n\
    store.write('note.md', 'kept')\n\
    return models.infer('asked') .. '|' .. store.read('note.md')\n```\n";

#[tokio::test]
async fn run_returns_the_declared_output_and_the_outcome_under_the_requests_name() {
    let recorder = Arc::new(MemoryRecorder::new());
    let report = plain_harness(&recorder, ScriptedBroker::replying())
        .run(RunRequest {
            input_text: Some("# Paper".to_owned()),
            ..request(FILES)
        })
        .await
        .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "done".to_owned()
        }
    );
    assert_eq!(report.output, Ok("seen: # Paper".to_owned()));
    let run_id = report.run_id.expect("the run began at the recorder");
    assert_eq!(
        recorder.outcome(run_id),
        Some(report.outcome.clone()),
        "the recorder ends the run with the reported outcome"
    );
    let meta = recorder.meta(run_id).expect("the recorder began the run");
    assert_eq!(
        meta.session_id, NAME,
        "the metadata holds the request's name"
    );
    assert_eq!(meta.agent, "", "the Harness names no agent");

    let records = recorder.records(run_id);
    let kinds: Vec<RecordKind> = records.iter().map(|record| record.kind).collect();
    assert_eq!(
        kinds.first(),
        Some(&RecordKind::Event),
        "the parse's events lead"
    );
    assert_eq!(
        kinds.last(),
        Some(&RecordKind::Event),
        "the run's end is an event"
    );
    let effects: Vec<usize> = kinds
        .iter()
        .enumerate()
        .filter(|(_, kind)| **kind == RecordKind::Effect)
        .map(|(position, _)| position)
        .collect();
    assert_eq!(effects.len(), 2, "the read and the write");
    for position in effects {
        assert_eq!(
            kinds[position + 1],
            RecordKind::Answer,
            "a serial run's answer follows its effect"
        );
    }
    for event in records
        .iter()
        .filter(|record| record.kind == RecordKind::Event)
    {
        assert_eq!(
            event.payload["execution"], NAME,
            "every event names the request's name as its execution"
        );
    }
}

#[tokio::test]
async fn source_that_fails_to_parse_ends_as_a_recorded_failed_run_with_its_parse_events() {
    let recorder = Arc::new(MemoryRecorder::new());
    let report = plain_harness(&recorder, ScriptedBroker::replying())
        .run(request(UNCLOSED))
        .await
        .expect("a parse failure is the run's outcome, not an error");

    let run_id = report
        .run_id
        .expect("the run began before its source parsed");
    let RunOutcome::Failed { kind, .. } = &report.outcome else {
        panic!("the run failed: {:?}", report.outcome);
    };
    assert_eq!(kind, "Parse");
    assert_eq!(recorder.outcome(run_id), Some(report.outcome.clone()));
    assert_eq!(report.output, Err(OutputError::NotCompleted));
    let records = recorder.records(run_id);
    assert!(
        records
            .iter()
            .all(|record| record.kind == RecordKind::Event),
        "a run that failed to parse records only the parse's events"
    );
    assert_eq!(
        records.first().map(|record| record.payload["kind"].clone()),
        Some(json!("parse_started"))
    );
    assert_eq!(
        records.last().map(|record| record.payload["kind"].clone()),
        Some(json!("parse_failed"))
    );
}

#[tokio::test]
async fn a_vfs_effect_is_answered_inline_on_the_thread_that_polls_the_run() {
    let ran_on: Kept<ThreadId> = Kept::default();
    let seen = Arc::clone(&ran_on);
    let vfs = VfsRef::builder()
        .store(
            "/",
            HookedBackend::new(move || seen.lock().unwrap().push(thread::current().id())),
        )
        .build();
    let recorder = Arc::new(MemoryRecorder::new());
    let report = plain_harness(&recorder, ScriptedBroker::replying())
        .run(RunRequest {
            vfs,
            ..request(WRITES_TWICE)
        })
        .await
        .expect("the run reaches an outcome");

    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "12".to_owned()
        }
    );
    assert_eq!(
        *ran_on.lock().unwrap(),
        vec![thread::current().id(); 2],
        "both writes ran inline, on the thread that polls the run"
    );
    let records = recorder.records(report.run_id.expect("the run began"));
    for (position, effect) in records.iter().enumerate() {
        if effect.kind == RecordKind::Effect {
            let answer = &records[position + 1];
            assert_eq!(answer.kind, RecordKind::Answer, "nothing comes between");
            assert_eq!(answer.effect_id, effect.effect_id, "the answer is its own");
        }
    }
}

/// Wakes the thread parked in [`block_on`].
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// A std-only executor: polls `future` on this thread and parks between
/// polls until its waker unparks the thread.
fn block_on<F: Future + Unpin>(mut future: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(thread::current())));
    let mut cx = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = Pin::new(&mut future).poll(&mut cx) {
            return output;
        }
        thread::park();
    }
}

/// Compiles only for a `Send` value.
fn assert_send<T: Send>(_: &T) {}

#[test]
fn one_run_completes_on_the_tests_own_thread_under_a_thread_parking_block_on() {
    let recorder = Arc::new(MemoryRecorder::new());
    let broker = ScriptedBroker::replying();
    let rounds = broker.rounds();
    let run = plain_harness(&recorder, broker).run(request(INFERS_AND_STORES));
    assert_send(&run);

    let report = block_on(Box::pin(run)).expect("the run reaches an outcome with no runtime");
    assert_eq!(
        report.outcome,
        RunOutcome::Completed {
            final_text: "re: asked|kept".to_owned()
        }
    );
    assert_eq!(rounds.lock().unwrap().len(), 1, "the one infer");
}
