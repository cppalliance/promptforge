Record every run a Harness drives in a store of your own, and read a run back.

You need this when your program keeps a history of runs, or wants to see what one run did step by step.

# Where this fits

[The crate overview](crate) shows how to run a prompt, answer its questions, and stop it. Every run is also written to a *recorder*, an object your program passes to [`Harness::new`](crate::Harness::new) beside the broker, the timer, the capability registry, and the services. The Harness only writes to it. It holds no database and no storage path, and it never reads a run back. The recorder is the Host's, so the Host decides where a history lives and how long it stays.

This page shows how to write a recorder, how to use the one the crate ships for tests, and how to read a run's events back from it.

# Record runs in a store of your own

`desk` is the Host you built on the main page. It wants a line for every record each run makes, in a store it controls. A file, a database, or a queue are all fine, because the Harness knows only the three calls of a [`RunRecorder`].

Passing a recorder feels like handing a logger a `Write`: the Harness calls it for each record, and you decide where the bytes go. Unlike a plain writer, each call returns a future that the Harness awaits before it goes on, and the recorder issues the id of each run.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{
    Record, RecordKind, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};
use harness::vfs::VfsRef;
use harness::{Harness, HostSnapshot, RunRequest};
use std::error::Error;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
# use harness::{BoxFuture, InferenceBroker, OnDelta, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# struct Offline;
# impl InferenceBroker for Offline {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         Box::pin(async { Ok(ModelCatalog::empty()) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round, _: Option<OnDelta>) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         let kind = CompletionErrorKind::Unavailable;
#         Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
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
            self.note(format!("run {run} begins: {}", meta.session_id))?;
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

// 5. Hand the recorder to the run's Harness, and keep a handle of your own.
let recorder = Arc::new(DeskRecorder::default());
let shared: Arc<dyn RunRecorder> = recorder.clone();
let harness = Harness::new(shared, Arc::new(Offline), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new());

// 6. Run a prompt that returns a text: the run begins, records, and ends.
let hello = "---\nname: hello\ndescription: Says hello\npromptforge: 0\n---\n\n# Hello\n\n## Greet\n\n```lua\nreturn 'Hello.'\n```\n";
let request = RunRequest {
    name: "desk-hello-1".into(),
    source: hello.into(),
    args: String::new(),
    input_text: None,
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};
harness.run(request).await?;
let lines = recorder.lines.lock().map_err(|_| "poisoned")?.clone();
assert_eq!(lines.first().map(String::as_str), Some("run 1 begins: desk-hello-1"));
assert_eq!(lines.last().map(String::as_str), Some("run 1 ends: completed"));
assert!(lines.iter().any(|line| line == "run 1: event"));
# Ok(())
# }
````

1. Step 1 defines `DeskRecorder`. One recorder may serve every run your program makes, and calls from different runs may overlap, so its state sits behind an atomic and a lock. Calls within one run never overlap. The `lines` list stands in for a store of your own.
2. Step 2 implements `begin_run`. It receives a [`RunMeta`], what the Harness knows when a run starts, and returns the [`RunId`] the run will carry in every later call. The recorder picks the number, so a database can hand out its own row ids. `session_id` holds the request's `name`.
3. Step 3 implements `append`, which receives one [`Record`] for every effect the Engine issues, every answer the Harness gives it, and every event it reports. `desk` keeps only the kind. A real store would keep the `payload`, which is JSON.
4. Step 4 implements `end_run`, which receives how the run ended as a [`RunOutcome`].
5. Step 5 hands an `Arc<dyn RunRecorder>` to [`Harness::new`](crate::Harness::new) as its first argument, and keeps a typed handle to read the store back. The broker, `Offline`, and the timer, `Clock`, are defined in hidden lines.
6. Step 6 runs a prompt that needs no model, and asserts the first and last lines and that an event was recorded between them.

The Harness awaits each call before it goes on, so the order you see is the order the run took:

- A step's events reach the recorder before the step's effects start.
- Every effect gets exactly one answer record, and an effect the Harness drops gets the answer `Dropped`.
- A run begun by a prompt that does not parse still gets its parse events, then an `end_run` with a failed outcome.

A recorder that returns an error stops its run. The Harness sends no further call for that run, so it never reaches `end_run`, and [`Harness::run`](crate::Harness::run) returns [`HarnessError::Recorder`](crate::HarnessError::Recorder) naming the run it issued. A recorder that prefers to keep running logs its own failure and returns `Ok`.

A recorder can sit in front of another one. A Host that wants to see each event as the run reports it passes a recorder that hands every call on to its store and, once the store accepted an event, shows it: the event then reaches the operator only after it is on record.

You might expect the Harness to skip a record that fails and keep going. Instead, it stops the run, because a record that was skipped can never be added later, and a history with a hole in it misleads whoever reads it.

Write one recorder for the whole Host, order your writes by run, and let a failure stop the run. Next, [Keep runs in memory](#keep-runs-in-memory) uses the recorder the crate ships.

# Keep runs in memory

A test, or a Host that needs no durable record, can use the [`MemoryRecorder`] the crate ships. It keeps every run in memory and forgets all of them when it drops.

A `MemoryRecorder` feels like a `Vec` behind a lock that you read back after the work is done. Unlike a bare vector, it issues the run ids, and it refuses a write that breaks the call order, so a test notices a caller that does.

````
use harness::record::{MemoryRecorder, RunId, RunMeta, RunOutcome, RunRecorder};
use std::error::Error;
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn Error>> {

// 1. A recorder that never saw a run knows nothing about it.
let recorder = MemoryRecorder::new();
assert!(recorder.meta(RunId::from_raw(1)).is_none());

// 2. Begin a run by hand: ids start at 1, and the run is open until it ends.
let meta = RunMeta {
    session_id: "desk-hello-1".into(),
    agent: String::new(),
    prompt_hash: "sha256:00".into(),
    seed: 7,
    flags: 0,
    started_at: 0,
};
let run = recorder.begin_run(meta).await?;
assert_eq!(run.get(), 1);
assert_eq!(recorder.meta(run).map(|meta| meta.session_id), Some("desk-hello-1".to_owned()));
assert_eq!(recorder.outcome(run), None);

// 3. End the run; a second end is refused, and the first outcome stays.
recorder.end_run(run, RunOutcome::Cancelled).await?;
assert!(recorder.end_run(run, RunOutcome::Cancelled).await.is_err());
assert_eq!(recorder.outcome(run), Some(RunOutcome::Cancelled));
assert!(recorder.records(run).is_empty());
# Ok(())
# }
````

1. Step 1 asserts that a recorder knows nothing of a run it never began: [`MemoryRecorder::meta`] returns `None`, and `records` returns nothing.
2. Step 2 begins a run by hand, as the Harness does. The ids count up from 1, `meta` returns what the run began with, and `outcome` is `None` while the run is open.
3. Step 3 ends the run and asserts that [`MemoryRecorder::outcome`] now returns it. A second `end_run` is an error, and so is an append to an unknown or ended run.

Pass a typed `Arc<MemoryRecorder>` to the Harness as an `Arc<dyn RunRecorder>`, and keep the typed handle. The recorder outlives the Harness, so a test reads a run after [`Harness::run`](crate::Harness::run) has consumed it, by the `run_id` its [`RunReport`](crate::RunReport) carries.

You might expect a recorder to find runs for you. Instead, it answers by id, and the run's report is where your program gets the id.

Keep a typed handle, read runs back by their ids, and treat a refused write as a bug in the caller. Next, [Read a run's events back](#read-a-runs-events-back) reads what one run reported.

# Read a run's events back

`desk` wants to show what a run reported, after the run has ended. The recorder holds every record of the run, in order; the events are the records whose kind is [`RecordKind::Event`].

Reading a run back feels like reading a log file: every line is there, in the order it was written. Unlike a log file, each line is a typed [`Record`] whose payload is the step itself, in JSON.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::{MemoryRecorder, RecordKind};
use harness::vfs::VfsRef;
use harness::{Harness, HostSnapshot, RunRequest};
use std::error::Error;
use std::sync::Arc;
# use harness::{BoxFuture, InferenceBroker, OnDelta, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# struct Offline;
# impl InferenceBroker for Offline {
#     fn models(&self) -> BoxFuture<Result<ModelCatalog, CompletionError>> {
#         Box::pin(async { Ok(ModelCatalog::empty()) })
#     }
#     fn chat(&self, _: ModelBinding, _: Vec<Message>, _: Vec<ToolSchema>, _: CompletionOptions, _: promptforge::effect::Round, _: Option<OnDelta>) -> BoxFuture<Result<Box<Completion>, CompletionError>> {
#         let kind = CompletionErrorKind::Unavailable;
#         Box::pin(async move { Err(CompletionError::new(kind, kind.phrase())) })
#     }
# }
# struct Clock;
# impl Timer for Clock {
#     fn sleep(&self, seconds: f64) -> BoxFuture<()> {
#         Box::pin(tokio::time::sleep(std::time::Duration::from_secs_f64(seconds)))
#     }
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn Error>> {

// 1. Run a prompt over a memory recorder, and keep a handle to read it back.
let recorder = Arc::new(MemoryRecorder::new());
let harness = Harness::new(recorder.clone(), Arc::new(Offline), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new());
let hello = "---\nname: hello\ndescription: Says hello\npromptforge: 0\n---\n\n# Hello\n\n## Greet\n\n```lua\nreturn 'Hello.'\n```\n";
let request = RunRequest {
    name: "desk-hello-1".into(),
    source: hello.into(),
    args: String::new(),
    input_text: None,
    vfs: VfsRef::default(),
    host: HostSnapshot::default(),
};
let report = harness.run(request).await?;

// 2. Read the run's records by the id its report carries, and keep the events.
let run = report.run_id.ok_or("the recorder began the run")?;
let kinds: Vec<String> = recorder
    .records(run)
    .into_iter()
    .filter(|record| record.kind == RecordKind::Event)
    .filter_map(|record| record.payload["kind"].as_str().map(str::to_owned))
    .collect();

// 3. The events say what the run did, in order, each naming the run.
assert!(kinds.iter().any(|kind| kind == "section_started"));
assert!(recorder.records(run).iter().all(|record| record.kind != RecordKind::Event || record.payload["execution"] == "desk-hello-1"));
# Ok(())
# }
````

1. Step 1 runs a prompt over a [`MemoryRecorder`], as the main page's [Run a prompt](crate#run-a-prompt) tour does.
2. Step 2 reads the run's records with [`MemoryRecorder::records`], by the `run_id` the report carries, and keeps the `kind` of each event's payload.
3. Step 3 asserts that the run reported its section starting, and that every event names the run by the request's `name` in its `execution` field.

The records hold effects and answers too, so a history shows what the run asked for and what the Harness answered, not only what the run reported. A model round's thinking, reply, and tool-call events each carry the round's id as `round`, the id the round's [`Round`](promptforge::effect::Round) held when it reached your broker.

You might expect the Harness to keep a run's events for you to read later. Instead, it keeps nothing: the recorder is the only copy, so a Host that shows a run's history keeps the records it needs in its own recorder.

Read a run back by its id, and pick the events by kind. [Where to go next](crate#where-to-go-next) lists the other pages.

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

[`RecorderFuture`] is what every [`RunRecorder`] call returns: a boxed, sendable future that borrows the recorder and resolves to a `Result` with a [`RecorderError`]. Write `Box::pin(async move { ... })` and return it.

## RunId

[`RunId`] names one run as its recorder issued it: whatever [`RunRecorder::begin_run`] returned. It means something only to the recorder that issued it. A [`RunReport`](crate::RunReport) carries the id of its run.

- [`RunId::from_raw`]: wraps any `i64` with no check, including zero and negative values, so a successful call does not mean the run exists.
- [`RunId::get`]: the raw number, the value a store would keep.
- `Display` prints the bare number.

## RunMeta

[`RunMeta`] is what the Harness knows about a run when it begins, and what [`RunRecorder::begin_run`] receives.

- `session_id`: the run's name, the [`RunRequest`](crate::RunRequest)'s `name`.
- `agent`: empty from the Harness; a recorder in front of another may fill it in before handing the call on.
- `prompt_hash`: `sha256:` and the lowercase hex digest of the prompt's text, so a history can be matched to the exact text that produced it.
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

[`RunRecorder`] is the trait a Host implements to take a run's history. Pass an `Arc<dyn RunRecorder>` to [`Harness::new`](crate::Harness::new). It is `Send` and `Sync`, because one recorder may serve every run. Calls within one run never overlap, and calls from different runs may, so an implementation guards its own state. [Record runs in a store of your own](#record-runs-in-a-store-of-your-own) teaches it.

- [`RunRecorder::begin_run`]: receives a [`RunMeta`] once per run and returns the run's [`RunId`].
- [`RunRecorder::append`]: receives each [`Record`] of an open run, in loop order.
- [`RunRecorder::end_run`]: receives the [`RunOutcome`] once per run that finishes. A run stopped by a refused write gets no `end_run`.
- Every call returns a [`RecorderFuture`], which the Harness awaits before it goes on.
