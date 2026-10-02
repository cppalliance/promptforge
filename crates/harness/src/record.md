Record every run a Harness makes in a store of your own, and read what a session sent.

You need this when your program keeps a history of runs, or when a client reconnects to a session and must see the events it missed.

# Where this fits

[The crate overview](crate) shows how to launch a session, answer its questions, and watch its events. Every run the Harness makes is also written to a *recorder*, an object your program passes to [`Harness::new`](crate::Harness::new) beside the config, the capability registry, and the services. The Harness only writes to it. It holds no database and no storage path, and it never reads a run back. The recorder is the Host's, so the Host decides where a history lives and how long it stays.

The Harness keeps one thing for itself: each session's *transcript*, the events the session has sent, in memory. This page shows how to write a recorder, how to use the one the crate ships for tests, and how to read a transcript.

# Record runs in a store of your own

`desk` is the Host you built on the main page. It wants a line for every record each run makes, in a store it controls. A file, a database, or a queue are all fine, because the Harness knows only the three calls of a [`RunRecorder`].

Passing a recorder feels like handing a logger a `Write`: the Harness calls it for each record, and you decide where the bytes go. Unlike a plain writer, each call returns a future that the Harness awaits before it goes on, and the recorder issues the id of each run.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{
    Record, RecordKind, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use harness::{Harness, HarnessConfig};
use std::error::Error;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn Error>> {

// 1. desk's recorder: a run counter, and a list of lines standing in for desk's own store.
#[derive(Default)]
struct DeskRecorder {
    runs: AtomicI64,
    lines: Mutex<Vec<String>>,
}

impl DeskRecorder {
    fn note(&self, line: String) -> Result<(), RecorderError> {
        let mut lines = self
            .lines
            .lock()
            .map_err(|_| RecorderError::new("desk's store is locked for good"))?;
        lines.push(line);
        Ok(())
    }
}

impl RunRecorder for DeskRecorder {
    // 2. A run begins: issue its id, and keep what the Harness knew about it.
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let run = RunId::from_raw(self.runs.fetch_add(1, Ordering::SeqCst) + 1);
            self.note(format!("run {run} begins: agent {}", meta.agent))?;
            Ok(run)
        })
    }

    // 3. Every effect, answer, and event of the run arrives here, in order.
    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move { self.note(format!("run {run}: {}", record.kind.as_str())) })
    }

    // 4. The run ends, once, with how it ended.
    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        Box::pin(async move { self.note(format!("run {run} ends: {}", outcome.as_str())) })
    }
}

// 5. Hand the recorder to the Harness after its config, and keep a handle of your own.
let recorder = Arc::new(DeskRecorder::default());
let shared: Arc<dyn RunRecorder> = recorder.clone();
let config = HarnessConfig { agents_path: "desk/agents".into() };
let harness = Harness::new(config, shared, CapabilityRegistry::new(), HostServices::new());
assert_eq!(harness.discover(), ["chat"]);

// 6. Call the recorder the way the Harness does: begin, append, end.
let meta = RunMeta {
    session_id: "session-1".into(),
    agent: "chat".into(),
    prompt_hash: "sha256:00".into(),
    seed: 7,
    flags: 0,
    started_at: 0,
};
let run = recorder.begin_run(meta).await?;
let record = Record {
    task_id: "0".into(),
    task_seq: 0,
    kind: RecordKind::Event,
    effect_id: None,
    payload: [("kind", "parse_started")].into_iter().collect(),
};
recorder.append(run, record).await?;
recorder.end_run(run, RunOutcome::Cancelled).await?;
assert_eq!(
    *recorder.lines.lock().map_err(|_| "poisoned")?,
    ["run 1 begins: agent chat", "run 1: event", "run 1 ends: cancelled"],
);
# Ok(())
# }
````

1. Step 1 defines `DeskRecorder`. One recorder serves every session, and calls from different runs may overlap, so its state sits behind an atomic and a lock. Calls within one run never overlap. The `lines` list stands in for a store of your own.
2. Step 2 implements `begin_run`. It receives a [`RunMeta`], what the Harness knows when a run starts, and returns the [`RunId`] the run will carry in every later call. The recorder picks the number, so a database can hand out its own row ids.
3. Step 3 implements `append`, which receives one [`Record`] for every effect the Engine issues, every answer the Harness gives it, and every event it reports. `desk` keeps only the kind. A real store would keep the `payload`, which is JSON.
4. Step 4 implements `end_run`, which receives how the run ended as a [`RunOutcome`].
5. Step 5 builds the Harness with `Arc<dyn RunRecorder>` as its second argument, between the config and the capability registry and services, here both empty, and keeps a typed handle to read the store back. [`Harness::new`](crate::Harness::new) touches no file, and neither does a launch, because the recorder is yours.
6. Step 6 calls the recorder by hand in the order the Harness does, and asserts the three lines. The hand-written calls stand in for a launched run, which needs a live model.

The Harness awaits each call before it goes on, so the order you see is the order the run took:

- A step's events reach the recorder before the step's effects start.
- Every effect gets exactly one answer record, and an effect the Harness drops gets the answer `Dropped`.
- A run begun by a prompt that does not parse still gets its parse events, then an `end_run` with a failed outcome.

A recorder that returns an error stops its run. The Harness sends no further call for that run, so it never reaches `end_run`, and the session reports [`FailureKind::RunFailed`](crate::FailureKind::RunFailed). A failed `begin_run` fails the run the same way, not the launch: [`Harness::launch`](crate::Harness::launch) still returns the session. A recorder that prefers to keep running logs its own failure and returns `Ok`.

You might expect the Harness to skip a record that fails and keep going. Instead, it stops the run, because a record that was skipped can never be added later, and a history with a hole in it misleads whoever reads it.

Write one recorder for the whole Host, order your writes by run, and let a failure stop the run. Next, [Keep runs in memory](#keep-runs-in-memory) uses the recorder the crate ships.

# Keep runs in memory

A test, or a Host that needs no durable record, can use the [`MemoryRecorder`] the crate ships. It keeps every run in memory and forgets all of them when it drops.

A `MemoryRecorder` feels like a `Vec` behind a lock that you read back after the work is done. Unlike a bare vector, it issues the run ids, and it refuses a write that breaks the call order, so a test notices a caller that does.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RunId, RunMeta, RunOutcome, RunRecorder};
use harness::{Harness, HarnessConfig};
use std::error::Error;
use std::sync::Arc;
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn Error>> {

// 1. Give the Harness one handle to a memory recorder, and keep another to read it back.
let recorder = Arc::new(MemoryRecorder::new());
let shared: Arc<dyn RunRecorder> = recorder.clone();
let config = HarnessConfig { agents_path: "desk/agents".into() };
let harness = Harness::new(config, shared, CapabilityRegistry::new(), HostServices::new());

// 2. Summarize runs by the ids a session reports through `Session::run_ids`.
fn summary(recorder: &MemoryRecorder, runs: &[RunId]) -> Vec<String> {
    runs.iter()
        .map(|run| {
            let records = recorder.records(*run).len();
            let outcome = recorder.outcome(*run).map_or("open", |outcome| outcome.as_str());
            format!("run {run}: {records} records, {outcome}")
        })
        .collect()
}

// 3. A recorder that never saw a run knows nothing about it.
assert!(recorder.meta(RunId::from_raw(1)).is_none());

// 4. Begin a run by hand: ids start at 1, and the run is open until it ends.
let meta = RunMeta {
    session_id: "session-1".into(),
    agent: "chat".into(),
    prompt_hash: "sha256:00".into(),
    seed: 7,
    flags: 0,
    started_at: 0,
};
let run = recorder.begin_run(meta).await?;
assert_eq!(run.get(), 1);
assert_eq!(recorder.meta(run).map(|meta| meta.agent), Some("chat".to_owned()));
assert_eq!(recorder.outcome(run), None);

// 5. End the run; a second end is refused, and the first outcome stays.
recorder.end_run(run, RunOutcome::Cancelled).await?;
assert!(recorder.end_run(run, RunOutcome::Cancelled).await.is_err());
assert_eq!(summary(&recorder, &[run]), ["run 1: 0 records, cancelled"]);

// 6. The recorder outlives the Harness, so a test can read a run after the Harness is gone.
drop(harness);
assert_eq!(recorder.outcome(run), Some(RunOutcome::Cancelled));
# Ok(())
# }
````

1. Step 1 gives the Harness a typed `Arc<MemoryRecorder>` as an `Arc<dyn RunRecorder>`, and keeps the typed handle. A launched session writes to the shared recorder, and you read the same one.
2. Step 2 defines `summary`, which looks each run up by id. In a live Host, [`Session::run_ids`](crate::Session::run_ids) returns the ids of the runs one session made, in launch order, as the recorder issued them.
3. Step 3 asserts that a recorder knows nothing of a run it never began: [`MemoryRecorder::meta`] returns `None`, and `records` returns nothing.
4. Step 4 begins a run by hand. The ids count up from 1, `meta` returns what the run began with, and `outcome` is `None` while the run is open.
5. Step 5 ends the run and asserts that [`MemoryRecorder::outcome`] now returns it. A second `end_run` is an error, and so is an append to an unknown or ended run.
6. Step 6 drops the Harness and reads the run anyway, because the recorder is yours and the Harness only wrote to it.

You might expect `Session::run_ids` to list every run of a Harness. Instead, it lists the runs of one session. The recorder holds every session's runs together, so look a run up by the ids you collected, not by scanning for them.

Keep a typed handle, read runs back by their ids, and treat a refused write as a bug in the caller. Next, [Read a session's transcript](#read-a-sessions-transcript) reads what a session sent.

# Read a session's transcript

`desk`'s client dropped, and when it comes back it must see the events it missed. You do not need the recorder for that. Each session keeps its own transcript, the events it has sent, in order, across every run it made. [`Session::transcript`](crate::Session::transcript) reads it.

A transcript feels like a log file you can seek in. Unlike a file, it lives in the session's memory, so reading it cannot fail and costs no disk.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::MemoryRecorder;
use harness::{Harness, HarnessConfig, SessionId};
use std::error::Error;
use std::sync::Arc;

// desk's client comes back: read what its session sent while the client was away.
async fn catch_up(
    harness: &Harness,
    id: &SessionId,
    last_seen: Option<u64>,
) -> Result<usize, Box<dyn Error>> {
    // 1. Find the running session by the id desk kept.
    let session = harness.session(id).ok_or("desk's session has closed")?;

    // 2. Subscribe first, so an event sent during the read is caught live.
    let _live = session.subscribe_events();

    // 3. Read from one past the last index the client showed, or from the start.
    let from = last_seen.map_or(0, |index| index + 1);
    let missed = session.transcript(from);

    // 4. Each entry holds its own index, and the indexes run one by one.
    assert!(missed.iter().zip(from..).all(|(entry, index)| entry.index == index));
    Ok(missed.len())
}

// 5. A Harness has no session for an id it never issued.
let config = HarnessConfig { agents_path: "desk/agents".into() };
let harness = Harness::new(config, Arc::new(MemoryRecorder::new()), CapabilityRegistry::new(), HostServices::new());
assert!(harness.session(&SessionId::new("never-issued")).is_none());
````

1. Step 1 finds the running session with [`Harness::session`](crate::Harness::session), as the main page's [Reattach after a disconnect](crate#reattach-after-a-disconnect) tour does.
2. Step 2 subscribes before it reads, so no event falls between the two.
3. Step 3 reads from one past the last shown index, or from zero. [`SessionEvent`](crate::SessionEvent)'s `index` counts every event from zero, across every run the session made, restarts included.
4. Step 4 asserts that the entries run in order from `from`, and returns how many the client missed.
5. Step 5 asserts that a Harness finds no session for an id it never issued. The doc test never calls `catch_up`, because it needs a running session.

Each entry holds the `index` and `reply` the live [`Session::subscribe_events`](crate::Session::subscribe_events) stream gave the same event, so a client can merge the two by index with no special case. The session adds an event to the transcript before it sends it live. A client that subscribes and then reads therefore sees no gap.

The transcript holds events only. The recorder also holds the effects and answers of each run, which no session call returns. The transcript includes the events of a run that failed to prepare, such as a prompt that does not parse.

A held [`Session`](crate::Session) handle still reads its transcript after the session closes. The transcript grows with every event, and the session frees it once the session has closed and every handle is dropped, so a Host that runs many long sessions should close them and drop its handles.

You might expect a transcript to survive a restart of your program, like a file. Instead, it lives only as long as its session, and the recorder is the lasting copy. Keep the records you need in your recorder, and use the transcript to catch a live client up.

Subscribe, then read from one past the last index. [Where to go next](crate#where-to-go-next) lists the other pages.

# Reference

## MemoryRecorder

[`MemoryRecorder`] is a [`RunRecorder`] that keeps every run in memory, for tests and doc examples. Pass it to [`Harness::new`](crate::Harness::new) as an `Arc`, and keep a typed clone to read runs back. [Keep runs in memory](#keep-runs-in-memory) shows it.

- [`MemoryRecorder::new`]: an empty recorder; so is [`Default`].
- [`MemoryRecorder::records`]: a run's records in the order they were appended; empty for a run this recorder never began.
- [`MemoryRecorder::meta`]: the [`RunMeta`] a run began with, or `None` for a run this recorder never began.
- [`MemoryRecorder::outcome`]: how a run ended, or `None` while it is open and for a run this recorder never began.
- Run ids start at 1 and count up in the order runs begin. An append to an unknown or ended run, and a second `end_run`, each fail with a [`RecorderError`].

## Record

A [`Record`] is one effect, answer, or event of a run, as the Harness appends it. The recorder decides each record's position and time. [Record runs in a store of your own](#record-runs-in-a-store-of-your-own) shows one arriving.

- `task_id`: the nearest enclosing task, a dot-separated path of child indexes, so the main walk is `0` and its second child task is `0.1`.
- `task_seq`: the record's position within its task.
- `kind`: a [`RecordKind`].
- `effect_id`: the in-flight effect's handle, for effects and their answers; `None` for events.
- `payload`: the serialized effect, answer, or event, as JSON.

## RecordKind

[`RecordKind`] says which side of the run a [`Record`] came from. `Effect` is something the Engine asked for. `Answer` is what the Harness answered, `Dropped` included. `Event` is something the Engine reported.

- [`RecordKind::as_str`]: `"effect"`, `"answer"`, or `"event"`, the text a store would keep.
- [`RecordKind::parse`]: reads that text back, and returns `None` for any other text.

## RecorderError

[`RecorderError`] says that a recorder could not take a write. Build one with [`RecorderError::new`] from the error, or the message, that made the recorder fail. Its own text is `the run recorder failed` and names no cause, so show it through [`display_chain`](crate::display_chain). The Harness stops the run that got it. [Record runs in a store of your own](#record-runs-in-a-store-of-your-own) teaches this.

## RecorderFuture

[`RecorderFuture`] is what every [`RunRecorder`] call returns: a boxed, sendable future that borrows the recorder and resolves to a `Result` with a [`RecorderError`]. Write `Box::pin(async move { ... })` and return it. The name keeps it apart from the per-event callback the Harness uses inside its own loop.

## RunId

[`RunId`] names one run as its recorder issued it: whatever [`RunRecorder::begin_run`] returned. It means something only to the recorder that issued it. [`Session::run_ids`](crate::Session::run_ids) lists the ids of one session's runs.

- [`RunId::from_raw`]: wraps any `i64` with no check, including zero and negative values, so a successful call does not mean the run exists.
- [`RunId::get`]: the raw number, the value a store would keep.
- `Display` prints the bare number.

## RunMeta

[`RunMeta`] is what the Harness knows about a run when it begins, and what [`RunRecorder::begin_run`] receives.

- `session_id`: the session that launched the run.
- `agent`: the agent the session runs.
- `prompt_hash`: `sha256:` and the lowercase hex digest of the prompt file's text, so a history can be matched to the exact text that produced it.
- `seed`: the Harness-drawn seed the Engine received.
- `flags`: the Engine's behavior flags as a bit set; `0` until a flag exists.
- `started_at`: when the run started, in UTC milliseconds since the Unix epoch.

## RunOutcome

[`RunOutcome`] is how a run ended, as [`RunRecorder::end_run`] receives it. Its variants and fields are public, so a store can build one back from a saved row.

| Variant | Meaning |
|---|---|
| `Completed` | The run finished; `final_text` holds its final text. |
| `Failed` | The run failed; `kind` names the failure's class and `message` describes it. |
| `Cancelled` | The Host cancelled the run. |

- [`RunOutcome::as_str`]: `"completed"`, `"failed"`, or `"cancelled"`, the text a store would keep.

## RunRecorder

[`RunRecorder`] is the trait a Host implements to take a run's history. Pass an `Arc<dyn RunRecorder>` to [`Harness::new`](crate::Harness::new). It is `Send` and `Sync`, because one recorder serves every session. Calls within one run never overlap, and calls from different runs may, so an implementation guards its own state. [Record runs in a store of your own](#record-runs-in-a-store-of-your-own) teaches it.

- [`RunRecorder::begin_run`]: receives a [`RunMeta`] once per run and returns the run's [`RunId`].
- [`RunRecorder::append`]: receives each [`Record`] of an open run, in loop order.
- [`RunRecorder::end_run`]: receives the [`RunOutcome`] once per run that finishes. A run stopped by a refused write gets no `end_run`.
- Every call returns a [`RecorderFuture`], which the Harness awaits before it goes on.