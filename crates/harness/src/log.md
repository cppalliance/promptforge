Read back the history a harness saves for each session, and tell its failures apart.

You need this when you read back what a session recorded, or handle a failure to read its saved history.

# Where this fits

[The crate overview](crate) shows how to launch a session, answer its questions, and watch its events. Every session you launch also saves a history as it runs. This page shows how to read that history back after you lose your client, and how to tell its failures apart.

# Read a session's history

Your `desk` host lost its client and wants back everything its session recorded. One execution of the session's agent is a *run*, as [the crate overview](crate#before-you-start) defines it.

A session begins a new run each time it restarts: after you cancel a turn, or after a gateway or catalog push that [the crate overview](crate#the-complete-program) says restarts it. A higher-generation gateway that cannot make a model client closes the session instead and records no new run.

A session's history is like a log file on disk: it outlives the client that watched it. Unlike a plain log file, each entry is a saved event filed under a numbered run, and reading one back can fail on its own.

That log file is a Turso database under the `state_dir` you gave [`HarnessConfig`](crate::HarnessConfig). Every session that harness launches shares it, and this page calls it the *run log*.

Every event a session has recorded, in order, is its *transcript*. Each entry is a [`SessionEvent`](crate::SessionEvent), whose `index` is its place in the transcript and whose `event` is the event itself, as saved JSON. [Stream a reply](crate#stream-a-reply) shows what its `reply` holds.

````
use harness::log::RunId;
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot};
# use harness::{LaunchRequest, SessionId, WaitFrame};
# use std::error::Error;
# use std::path::Path;
# const STUB_MODEL: &str = "http://127.0.0.1:4010";
# async fn desk(dir: &Path) -> Result<(Harness, SessionId), Box<dyn Error>> {
#     std::fs::create_dir_all(dir.join("agents"))?;
#     let harness = Harness::new(HarnessConfig {
#         agents_path: dir.join("agents"),
#         state_dir: dir.join("state"),
#     });
#     harness.set_gateway(GatewayBinding {
#         base_url: STUB_MODEL.to_owned(),
#         key: "desk-key".to_owned(),
#         generation: 1,
#     });
#     harness.set_catalog(CatalogBinding {
#         generation: 1,
#         models: vec![[("kind", "chat"), ("id", "stub")].into_iter().collect()],
#     });
#     harness.set_host(HostSnapshot {
#         selected_model: Some("stub".to_owned()),
#         ..HostSnapshot::default()
#     });
#     let request = LaunchRequest {
#         agent: "chat".to_owned(),
#         args: String::new(),
#         input_text: None,
#     };
#     let session = harness.launch(request).await?;
#     let (mut waits, mut events) = (session.subscribe_waits(), session.subscribe_events());
#     session.resend_waits();
#     let WaitFrame::Required { token } = waits.recv().await? else {
#         return Err("the chat agent's first question was cancelled".into());
#     };
#     session.send_input(&token, "Hello, desk.".to_owned(), || {})?;
#     while events.recv().await?.reply.is_none() {}
#     Ok((harness, session.id().clone()))
# }

// desk reads back everything its session saved, after losing its client.
async fn replay(dir: &Path) -> Result<(), Box<dyn Error>> {
    // desk launched `chat`, and the stub model answered once.
    let (harness, id) = desk(dir).await?;

    // 1. Reattach to the session by its id; its history is on disk.
    let session = harness.session(&id).ok_or("desk's session has closed")?;

    // 2. List the run numbers this history issued, in launch order.
    let first = *session.run_ids().first().ok_or("the session made no run")?;

    // 3. Save the first run's number, then rebuild its id from what you saved.
    let saved: i64 = first.get();
    let rebuilt = RunId::from_raw(saved);

    // 4. Read the transcript from the start, across every run.
    let events = session.transcript(0).await?;
    for event in &events {
        println!("event {}", event.index);
    }

    // 5. The rebuilt id names the same run, and at least one event printed.
    assert_eq!(rebuilt, first);
    assert!(!events.is_empty());
    Ok(())
}
````

1. Step 1 reattaches by the id `desk` kept. [`Harness::session`](crate::Harness::session) finds a [`Session`](crate::Session) only while it runs.
2. Step 2 calls [`Session::run_ids`](crate::Session::run_ids), which lists the runs this session has made, in launch order. It leaves out runs of other sessions in the same log.
3. Step 3 saves the first run's number with [`RunId::get`] and rebuilds an equal [`RunId`] with [`RunId::from_raw`]. No public call takes a `RunId`, so a rebuilt id is good only for comparing against `run_ids` or a failure message.
4. Step 4 calls [`Session::transcript`](crate::Session::transcript). `transcript(from)` returns the events whose `index` is at least `from`, numbered from zero across every run. Pass `0` to read everything, or your last seen index plus one to resume.
5. Step 5 asserts the rebuilt id equals the original and at least one event printed.

Your client can come and go. `transcript` reads from the run log, not from memory, so a client that reattaches gets every event from the first run on, including events sent while it was gone. The run log file stays on disk after your process exits, but no public call opens it, so only a `Session` handle can read it, either one you kept or one `Harness::session` returns while the session runs.

The hidden `desk` launches `chat` against a stub model and answers its first question. The doctest compiles `replay` and `desk` but runs neither, because they need a live model.

`transcript` fails with [`LogError::Payload`] when a stored payload does not parse as an event, even valid JSON of the wrong shape, or [`LogError::Corrupt`] when a stored row does not fit the run log's schema. Both point at the saved data, not the disk or database.

You might expect a `RunId` to name a run anywhere, like a global id. Instead, it is the row number the run log gave the run when it began, and means nothing against another state directory's log.

A history outlives your client, and its run ids mean something only in the log that issued them. Next, [Tell history failures apart](#tell-history-failures-apart) handles a failed read.

# Tell history failures apart

A history call failed, and your host must decide what to report: disk trouble, a damaged record, or a run the log did not expect.

You meet [`LogError`] in two places. [`Session::transcript`](crate::Session::transcript) returns `LogError` directly. [`Harness::launch`](crate::Harness::launch) returns it wrapped in [`LaunchError::Log`](crate::LaunchError::Log), and only when the run log cannot be opened. The harness opens the run log on the first launch, and a failed open is tried again by the next launch. A state directory that cannot be created is one way the open fails, and it reaches you as `Io` inside `LaunchError::Log`.

`LogError` works like [`io::Error`](std::io::Error) and its kind: you branch on the variant first. Unlike `io::Error`, the useful detail sits in the `source()` chain, not the message: `Database` keeps its cause behind [`DatabaseSource`], and `Payload` behind [`JsonSource`].

````
use harness::display_chain;
use harness::log::{DatabaseSource, LogError, RunId};
use std::error::Error;

// desk's report says which part of the history failed, and why.
fn report(error: &LogError) {
    // 1. Print the whole `source()` chain, because the top message names no cause.
    eprintln!("desk: {}", display_chain(error));

    // 2. Match the variant to learn which part failed; the wildcard arm covers new kinds.
    match error {
        LogError::Database { .. } => {
            // 3. Downcast the source to `DatabaseSource`, then reach the database's own error.
            let source = error.source();
            if let Some(database) = source.and_then(|cause| cause.downcast_ref::<DatabaseSource>()) {
                eprintln!("desk: the database reported {}", database.as_inner());
            }
        }
        LogError::Io { .. } => eprintln!("desk: check the disk under the state directory"),
        LogError::Payload { .. } => eprintln!("desk: a saved record's JSON is damaged"),
        LogError::Corrupt(_) => eprintln!("desk: a saved row disagrees with the schema"),
        LogError::UnknownRun(id) => eprintln!("desk: run {id} was never begun here; fix the id"),
        LogError::RunEnded(id) => eprintln!("desk: run {id} has ended; stop writing to it"),
        _ => eprintln!("desk: the history failed in a way desk does not know yet"),
    }
}

// 4. Ask about a run the history never began; this is the failure it answers with.
let never = RunId::from_raw(-1);
let failure = LogError::UnknownRun(never);
report(&failure);

// 5. The failure is `UnknownRun`, and it carries the id desk asked about.
assert!(matches!(failure, LogError::UnknownRun(id) if id == never));
````

1. Step 1 prints the whole chain with [`display_chain`](crate::display_chain).
2. Step 2 matches the variant. [`LogError::Database`] means the database refused an operation, and [`LogError::Io`] means an I/O operation failed. Those two point at the disk or the database. `Payload` means a record's JSON would not serialize on the way in or parse on the way out, and `Corrupt` means a stored row disagrees with the schema. Those point at the data. The `UnknownRun` and `RunEnded` arms word their messages as instructions, but `desk` has nothing to follow them with, because the harness writes the history itself and no public call of yours takes a [`RunId`] or writes a record. Treat those two messages as a report of what the log saw. `LogError` is `#[non_exhaustive]`, so a match outside the crate needs a wildcard arm.
3. Step 3 downcasts the `Database` source to `DatabaseSource`, calls [`DatabaseSource::as_inner`], and gets the database's own [`turso::Error`](https://docs.rs/turso/latest/turso/enum.Error.html).
4. Step 4 asks the history nothing. It builds [`LogError::UnknownRun`] by hand from `RunId::from_raw(-1)`, the value the log returns when asked about a run it never began. No public call takes a `RunId`, so a host cannot make the log answer this itself.
5. Step 5 asserts the failure is `UnknownRun` carrying the id step 4 built.

`DatabaseSource` is transparent, so its `source()` returns the Turso error's own cause, and a walk of `source()` steps straight past the Turso error. That makes step 3's downcast the only way to branch on what the database reported.

The variants hold `DatabaseSource`, so matching on `LogError` never names Turso. Matching on what `as_inner` returns does, and makes your crate depend on `turso`. The `From` conversions and `into_inner` also name Turso's type, and you use them only to wrap database calls of your own.

[`LogError::RunEnded`] means the log was asked to write to a run that has already ended. It and `UnknownRun` both carry the `RunId` and point away from the disk and the data.

A `Payload` failure works like step 3: downcast its source to `JsonSource` and call [`JsonSource::as_inner`] for the [`serde_json::Error`](https://docs.rs/serde_json/latest/serde_json/struct.Error.html). `is_eof` is true when the JSON ended early, a truncated record, and `is_syntax` when the text is not valid JSON, a malformed one. `is_data` is true when the JSON is valid but the wrong shape for an event. `classify` returns the same answer as one `Category` value.

You might expect to downcast the `Database` source straight to Turso's error type, since both print the same text. Instead, that downcast fails, because the source is a `DatabaseSource`; call `as_inner`.

Match the variant to learn what failed, then downcast the source to learn why. [The crate overview](crate#where-to-go-next) lists the other pages.

# Reference

## DatabaseSource

[`DatabaseSource`] holds Turso's own error as the cause of [`LogError::Database`], and displays and reports its source exactly as that error does. Use it to branch on what the database reported. Walking `source()` skips the database error itself, because `DatabaseSource` is transparent: its `source()` returns the Turso error's own cause, and a downcast straight to [`turso::Error`](https://docs.rs/turso/latest/turso/enum.Error.html) fails. Downcast the cause to `DatabaseSource`, then match on what [`DatabaseSource::as_inner`] borrows. [Tell history failures apart](#tell-history-failures-apart) shows the downcast.

- [`DatabaseSource::into_inner`] consumes the wrapper and returns the database error unchanged, with the same variant and text.
- Any Turso error converts into a `DatabaseSource` through `From`, so you can wrap database calls of your own.

## JsonSource

[`JsonSource`] holds the JSON error as the cause of [`LogError::Payload`], and displays and reports its source exactly as that error does. Use it to learn how a payload failed. Walking `source()` skips the JSON error itself, because `JsonSource` is transparent: its `source()` returns the JSON error's own cause. Downcast the cause to `JsonSource`, call [`JsonSource::as_inner`], and ask the [`serde_json::Error`](https://docs.rs/serde_json/latest/serde_json/struct.Error.html) for its classification, such as `is_syntax`. [Tell history failures apart](#tell-history-failures-apart) shows where it fits.

- [`JsonSource::into_inner`] consumes the wrapper and returns the JSON error unchanged, with the same text and classification.
- Any JSON error converts into a `JsonSource` through `From`, so you can wrap JSON calls of your own.

## LogError

[`LogError`] says why reading or writing a session's history failed. [`Session::transcript`](crate::Session::transcript) returns it directly, and [`Harness::launch`](crate::Harness::launch) returns it inside [`LaunchError::Log`](crate::LaunchError::Log) only when the run log cannot be opened, including a state directory that cannot be created, and each launch retries a failed open until one succeeds. Match the variant, then log the whole `source()` chain, which holds the cause. It is `#[non_exhaustive]`, so a match outside the crate needs a wildcard arm. [Tell history failures apart](#tell-history-failures-apart) teaches the match.

| Variant | Meaning |
|---|---|
| [`Database`](LogError::Database) | The database refused an operation. Its `source` is a [`DatabaseSource`]. |
| [`Io`](LogError::Io) | An I/O operation failed. It can be the log file, or something else, such as creating the state directory. Its `source` is a plain [`io::Error`](std::io::Error). |
| [`Payload`](LogError::Payload) | A payload would not serialize on the way in or parse on the way out. Its `source` is a [`JsonSource`]. |
| [`UnknownRun`](LogError::UnknownRun) | No run with this [`RunId`] was ever begun in this run log. |
| [`RunEnded`](LogError::RunEnded) | The run with this `RunId` has already ended, and nothing more may be written to it. |
| [`Corrupt`](LogError::Corrupt) | A stored row does not fit the run log's schema. It is about the row's columns, not the event JSON: a payload that parses as JSON but not as an event gives `Payload`. The string describes the expected shape and the value found. |

- `?` converts a [`turso::Error`](https://docs.rs/turso/latest/turso/enum.Error.html), an `io::Error`, or a [`serde_json::Error`](https://docs.rs/serde_json/latest/serde_json/struct.Error.html) straight into `LogError`, wrapping the first in `DatabaseSource` and the third in `JsonSource`.
- Every `io::Error` that reaches it through `?` becomes `Io`, whatever the operation was, so `Io` alone does not mean the log file was unreachable.

## RunId

[`RunId`] names one run within one run log, as the row number the log gave the run when it began. Every session of the same harness shares that log, and the id means nothing against any other run log. No public call accepts a `RunId`. Store its number only to recognize the run later, in [`Session::run_ids`](crate::Session::run_ids) or in an `UnknownRun` or `RunEnded` message. [Read a session's history](#read-a-sessions-history) shows the round trip.

- [`RunId::from_raw`] wraps any `i64` with no check, including zero and negative values, so a successful call does not mean the run exists.
- [`RunId::get`] returns the raw run number, the value to store.
- `Display` prints the bare number, the same number the `UnknownRun` and `RunEnded` messages show.
