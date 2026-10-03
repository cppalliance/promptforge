Give a run files of your own, read back what it writes, and guard and watch every file operation.

You need this when a prompt must read your program's files, or when you want to keep what it writes after the run ends.

# Where this fits

[Run a prompt](crate#run-a-prompt) hands a run a fresh memory store. This page replaces that with a handle you build and keep. You seed files before the run, read what it wrote after, and guard and watch every operation along the way.

# Before you start

Every example on this page is code from `desk`, a small Host program that drives the Harness for one operator.

A prompt keeps its working files in a *store*. The store is the set of files a run reads and writes. The prompt's Lua code calls `store.read` and `store.write` on relative paths such as `notes.md`. Every *section* of the prompt shares the same files, and the files stay after the run ends.

A section is one `##` heading of the prompt with the prose and Lua under it, named by its heading text. A deeper heading such as `###` starts a section nested inside it. The [PromptForge language guide](https://cppalliance.github.io/promptforge/language/) covers the store from the prompt's side.

# Give a run your own files

Your prompt must read files your program provides, and you want to keep what it writes. The store outlives the run: files you place before the run wait for it, and files it writes stay for you. Every file operation goes through an *access*, a value you acquire from the store's handle and drop when you are done.

Handing a run a [`VfsRef`] feels like handing a spawned task an `Arc<Mutex<HashMap<String, String>>>`: you keep a clone and look inside when the task is done. Unlike a mutex, the store remembers which files each access touched, so you fill it before the run and read it after, never during.

A handle attaches each *backend*, the thing that holds files, at a path, and that attachment is a *mount*. The *declared store* is the one mount that the prompt's `store.*` calls and [`VfsRef::acquire_store`] reach, with relative paths such as `notes.md` joined onto its root.

````
use harness::capability::{CapabilityRegistry, HostServices};
use harness::record::MemoryRecorder;
use harness::vfs::{Origin, VfsRef};
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

// 1. desk's `shout` prompt reads `notes.md` and writes `summary.md`.
let shout = concat!(
    "---\nname: shout\ndescription: Shouts the operator's notes\npromptforge: 0\n",
    "input: { path: notes.md, description: The operator's notes }\n",
    "output: { path: summary.md, description: The shouted notes }\n",
    "---\n\n# Shout\n\n## Summary\n\n```lua\n",
    "store.write('summary.md', string.upper(store.read('notes.md')))\n",
    "```\n",
);

// 2. Build an in-memory store, and keep a clone for desk.
let vfs = VfsRef::default();
let kept = vfs.clone();

// 3. Seed `notes.md` at the exact path `shout` declares, then drop the view.
let seed = kept.acquire_store(Origin::new("desk seed"))?;
seed.write("notes.md", b"Ship on Friday.")?;
assert!(!seed.exists("Notes.md")?);
drop(seed);

// 4. Run `shout` over the store, with no input text.
let harness = Harness::new(Arc::new(MemoryRecorder::new()), Arc::new(Offline), Arc::new(Clock), CapabilityRegistry::new(), HostServices::new());
let request = RunRequest {
    name: "desk-shout-1".into(),
    source: shout.into(),
    args: String::new(),
    input_text: None,
    vfs,
    host: HostSnapshot::default(),
};
let report = harness.run(request).await?;

// 5. The report carries the declared output, and desk's clone still reaches it.
assert_eq!(report.output?, "SHIP ON FRIDAY.");
let summary = kept.acquire_store(Origin::new("desk collect"))?.read_string("summary.md")?;
assert_eq!(summary, "SHIP ON FRIDAY.");
# Ok(())
# }
````

1. Step 1 writes `shout`. Its frontmatter declares `notes.md` under `input:` as the file it reads, and `summary.md` under `output:` as the file it writes. A prompt reaches its declared files by plain relative path, like any other store file.
2. Step 2 builds [`VfsRef::default()`](VfsRef::default), a memory backend mounted at `/` and declared as the store, and clones it. Clones share the same files, so `kept` is how `desk` reaches them after the run takes the other.
3. Step 3 calls `acquire_store` with an [`Origin`], a label for who is asking, and gets an `Access`: a store view that takes the prompt's own paths and reports errors in those names. Seed each input at exactly the declared path, because paths are case-sensitive and never rewritten, so `Notes.md` and `notes.md` are two files. Then drop the view. An access holds a *claim*, the store's record that it touched a file, on each such file until it drops. A seed view still alive when the run starts makes the Harness's own check of `notes.md` come second, so the run fails before it starts with *run error kind* `Input`, the failure class the Harness reports in a failed outcome.
4. Step 4 moves the other clone into the request's `vfs` field. With no input text, the Harness only checks that the declared input file exists.
5. Step 5 reads the output twice: from the report's `output`, which the Harness read as the run completed, and through a fresh view of `desk`'s clone. Collect only after the run ends, because until then the prompt may rewrite that file, and its claims on it last as long.

With `input_text: Some`, the Harness writes that text over any file you seeded, so do one or the other, never both. With no input text and no such file in the store, the run fails before it starts with run error kind `Input`. The language guide's [How a failed run is classified](https://cppalliance.github.io/promptforge/language/16-limits-and-errors.html#how-a-failed-run-is-classified) says what each kind means.

The report's `output` is read once, as the run completes. For what each of its errors means, see [`OutputError`](crate::OutputError). Two come from the store:

- [`OutputError::Missing`](crate::OutputError::Missing) when the run completed without writing the file; the run still counts as a success.
- [`OutputError::Vfs`](crate::OutputError::Vfs) when the store refused the read for another reason, such as a refusal by a policy you install, as the next tour shows; only then does [`Error::source`](std::error::Error::source) give the [`VfsError`].

A reused store still holds an earlier run's output file. When the new run never writes it, the report's `output` holds the old text, not `OutputError::Missing`, so delete or check the output path before you run again.

You might expect any `VfsRef` to work as a run's store. Instead, [`VfsRef::new`] and [`VfsRef::with_policy`] declare no store, so `acquire_store` on them returns [`VfsError::Unsupported`]. A prompt with an `input:` file then fails at once with run error kind `Input`, because the Harness places that file through `acquire_store`. A prompt with no `input:` file fails with run error kind `Vfs` before any section runs. Use the default handle, or declare a store through [`VfsRef::builder`].

Seed before the run, read after it, and never hold a store access across it. Next, [Guard and watch a run's files](#guard-and-watch-a-runs-files) limits and records what the run touches.

# Guard and watch a run's files

Your program shares files with a running prompt, and you want to limit what the prompt touches, see each operation, and keep your accesses from colliding with the run's. A *policy* allows or refuses each operation. An *operation sink* is a callback that hears about each one just before it touches the files.

A policy and a sink feel like middleware: each request passes a check, gets logged, and reaches the backend. Unlike middleware, the store also tracks claims. Each `acquire` or `acquire_store` call starts its own *scope*, the group of accesses a single acquire starts. Yours holds only the `Access` the call returns, since `Access` has no `Clone`; a run's scope holds the run's first access plus the new one it gives each task it spawns. Accesses in one scope can be ordered by spawn and join, and accesses in two different scopes never are.

Nothing spawns or joins across scopes, so when your access and the run's touch one path, at least one writes, and both are alive, they conflict. Spawn and join, not timing, decide whether two accesses conflict, so an overlap between two live accesses fails on every run instead of only under unlucky timing. Which side fails does depend on timing, because the access that touches the path second is the one refused.

Each operation passes through its `Access` in order: the policy, then the claims, then the sink, then the files. This module re-exports only the three types the example's first `use` line names. `MemoryBackend`, `Op`, `Policy`, `Verdict`, and `VfsPath` live in [`promptforge::vfs`](promptforge::vfs), the module those three come from. To name them, add the `promptforge` crate to your program's dependencies beside `harness`.

The example's hidden lines define `Offline` and `Clock`, and an input broker, `Edit`, that stands in for the operator: when the run asks its question, `Edit` first tries `desk`'s own write to `notes.md`, notes how it went, and then answers.

````
use harness::vfs::{Origin, VfsError, VfsRef};
use promptforge::vfs::{MemoryBackend, Op, Policy, Verdict, VfsPath};
use std::sync::{Arc, Mutex};
# use async_trait::async_trait;
# use harness::capability::{CapabilityRegistry, HostServices, INPUT_BROKER, InputBroker, InputError, UserInput};
# use harness::record::MemoryRecorder;
# use harness::{BoxFuture, Harness, HostSnapshot, InferenceBroker, OnDelta, RunRequest, Timer};
# use promptforge::model::{Completion, CompletionError, CompletionErrorKind, CompletionOptions, Message, ModelBinding, ModelCatalog, ToolSchema};
# use std::error::Error;
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
# struct Edit {
#     vfs: VfsRef,
#     outcome: Arc<Mutex<Option<Result<(), VfsError>>>>,
# }
# #[async_trait]
# impl InputBroker for Edit {
#     async fn wait(&self) -> Result<String, InputError> {
#         let edit = self.vfs.acquire_store(Origin::new("desk edit")).and_then(|view| view.write("notes.md", b"Ship on Monday."));
#         *self.outcome.lock().expect("desk's slot is healthy") = Some(edit);
#         Ok("Approved.".to_owned())
#     }
# }
# #[tokio::main(flavor = "current_thread")]
# async fn main() -> Result<(), Box<dyn Error>> {
# let review = concat!(
#     "---\nname: review\ndescription: Reads the notes, then asks the operator\npromptforge: 0\n",
#     "capabilities:\n  - promptforge/user-input\n",
#     "input: { path: notes.md, description: The operator's notes }\n",
#     "output: { path: summary.md, description: The approved notes }\n",
#     "---\n\n# Review\n\n## Approve\n\n```lua\n",
#     "local notes = store.read('notes.md')\n",
#     "local answer = input.ask()\n",
#     "store.write('summary.md', notes .. ' ' .. answer)\n",
#     "```\n",
# );

// 1. A policy that refuses every path except the two files desk declares.
struct DeskFiles;
impl Policy for DeskFiles {
    fn check(&self, _op: Op, path: &VfsPath) -> Verdict {
        let declared = ["/notes.md", "/summary.md"].contains(&path.as_str());
        if declared { Verdict::Allow } else { Verdict::Deny(format!("{path} is not a desk file")) }
    }
}

// 2. Build the store with the policy and a sink that records each event's origin.
let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
let sink = Arc::clone(&seen);
let vfs = VfsRef::builder()
    .store("/", MemoryBackend::new())
    .policy(DeskFiles)
    .on_op(move |event| sink.lock().unwrap().push((event.origin().label.clone(), event.origin().file.clone())))
    .build();

// 3. Seed `notes.md`; a stray write is denied and never reaches the sink.
vfs.acquire_store(Origin::new("desk seed"))?.write("notes.md", b"Ship on Friday.")?;
let stray = vfs.acquire_store(Origin::new("desk stray"))?.write("secret.md", b"x");
assert!(matches!(stray, Err(VfsError::PermissionDenied { .. })));
assert_eq!(seen.lock().unwrap()[..], [("desk seed".to_owned(), file!().to_owned())]);

// 4. Run `review`; while it asks its question, desk's edit of the notes it read conflicts.
let edited = Arc::new(Mutex::new(None));
let mut services = HostServices::new();
let operator: Arc<dyn InputBroker> = Arc::new(Edit { vfs: vfs.clone(), outcome: Arc::clone(&edited) });
services.provide(&INPUT_BROKER, operator)?;
let mut capabilities = CapabilityRegistry::new();
capabilities.register(Arc::new(UserInput::new()))?;
let harness = Harness::new(Arc::new(MemoryRecorder::new()), Arc::new(Offline), Arc::new(Clock), capabilities, services);
let request = RunRequest { name: "desk-review-1".into(), source: review.into(), args: String::new(), input_text: None, vfs: vfs.clone(), host: HostSnapshot::default() };
let report = harness.run(request).await?;
assert!(matches!(edited.lock().unwrap().take(), Some(Err(VfsError::Conflict { .. }))));

// 5. The notes are unchanged, the run's own operations name the prompt, and the output is the run's.
assert_eq!(vfs.acquire_store(Origin::new("desk check"))?.read_string("notes.md")?, "Ship on Friday.");
assert!(seen.lock().unwrap().iter().any(|(_, file)| file == "Review"));
assert_eq!(report.output?, "Ship on Friday. Approved.");
# Ok(())
# }
````

1. Step 1 defines `DeskFiles`, a `Policy` that allows only the two declared files. It sees full paths, so `notes.md` arrives as `/notes.md`.
2. Step 2 builds the handle with [`VfsRef::builder`], the only constructor that declares a store and installs a sink. The sink receives an `OpEvent` with three getters: `op()` for the operation kind, `path()` for the full path, and `origin()` for the [`Origin`] of the access that made it. The sink hears only operations that passed the policy and the claims, once per path, so a rename reports twice.
3. Step 3 seeds `notes.md`, and the stray write to `secret.md` gets [`VfsError::PermissionDenied`]. Each view there is a temporary, dropped at the end of its statement. The sink holds only the seed, stamped with this file by [`Origin::new`], because a denied operation registers no claim, fires no event, and changes nothing.
4. Step 4 runs the hidden `review` prompt, which reads `notes.md`, asks the operator, and writes `summary.md`. At its question the run holds a live claim on `notes.md`, so the hidden `Edit` broker's write to it gets [`VfsError::Conflict`]. `review` only read `notes.md`, but a read claims its path too, so what a run reads never depends on timing. Without that claim, the summary would be built from `Ship on Friday.` or `Ship on Monday.` depending on whether the edit landed before or after the read. With it, the overlap fails whichever side comes second, so never write a file the run has read, not only the files it writes, until the run ends. Clones share claims, so cloning does not help.
5. Step 5 finds `notes.md` unchanged and a sink event whose file is `Review`, the prompt's H1 title, not a path, since a prompt may never exist on disk. The run's `label` is that title too, except in a task the run spawns, which uses its section's name. Your operations hold a Rust file path in `file`, and so do the Harness's own `input: <path>` and `output: <path>` operations. Tell the run's operations by `file`, which holds the prompt's title, and tell the Harness's from yours by `label`.

Which access reports a conflict does depend on timing: the one that touches the path second fails and never reaches the files. Yours returns the conflict, whose `detail` names both sides; a run access ends the run with run error kind `Determinism`, which the prompt cannot catch. The first write stays either way, so check the file before you retry.

A denial reaches the prompt as a store error with reason `permission_denied`, which it can catch with `pcall`, Lua's protected call; uncaught, it ends the run with kind `Vfs`. Your own `Access` gets `PermissionDenied` with the verdict text in `reason`. A third variant, `Verdict::Ask(String)`, for an operation that needs user approval, arrives exactly as [`Verdict::Deny`](promptforge::vfs::Verdict::Deny) does, so read `reason` to tell them apart.

The builder takes the policy by value, so to change it mid-run, have it read shared state such as an `Arc<Mutex<Verdict>>` field and keep a clone; the next operation sees the change.

The Harness's own file work, labeled `input: <path>` before the run and `output: <path>` after it, passes through your policy and sink too. A policy that refuses the input fails the run with kind `Input`, and one that refuses the output read leaves [`OutputError::Vfs`](crate::OutputError::Vfs) in the report's `output`.

`Access` has no `Clone`, so each `acquire` gives you exactly one `Access`, and dropping it ends its scope and its claims; keep each one short.

You might expect a Host write to wait for the run's access, the way a `Mutex` would. Instead, a store operation fails at once with `VfsError::Conflict`, so retry only after the other access drops.

The policy decides, the sink watches, `Origin` labels, and claims catch overlaps. The [Reference](#reference) covers only `Origin`, [`VfsError`], and [`VfsRef`], the three types this module exports. For [`Access`](promptforge::vfs::Access), `Policy`, `Verdict`, `Op`, and `OpEvent`, see `promptforge::vfs`; `Access` lists the operations the `VfsError` table names, such as `str_replace`, `read_range`, and `remove` with its `recursive` flag.

# Reference

## Origin

[`Origin`] labels who asked for an access. Each operation that passes the policy and the claims reaches the operation sink with that label and a source position; a refused one fires nothing. `Origin` never decides whether an operation is allowed. Pass one to [`VfsRef::acquire`] or [`VfsRef::acquire_store`], one per caller you want to tell apart, because an access keeps its origin for life. The struct is `#[non_exhaustive]`, so build it only through the two constructors below, as taught in [Guard and watch a run's files](#guard-and-watch-a-runs-files).

- [`Origin::new`]: takes the label from its argument and the file and line from your call site; mark any wrapper `#[track_caller]` too.
- [`Origin::at`]: stores the label, file, and line exactly as given, for a position that is not a Rust call site, such as a prompt line.
- `label`: the most specific label you have, such as a section name, a tool id, or a fixture name.
- `file`: your Rust file for `Origin::new`, the given document for `Origin::at`, or the prompt's H1 title, not a path, for the run's operations.
- `line`: the 1-based line within `file`.

## VfsError

[`VfsError`] is the one error every file operation returns, and its variant says what kind of failure it was. Match on it to decide what to do next, and keep a wildcard arm, because the enum is `#[non_exhaustive]`. Its variants are not, so a custom backend builds them as struct literals, and `source()` always returns `None`. Through a store view, path fields hold the name you passed, not the full path, as in [Give a run your own files](#give-a-run-your-own-files).

| Variant | When it happens |
|---|---|
| `NotFound { path }` | The path does not exist in the backend that serves it. |
| `AlreadyExists { path }` | The path exists where creation required it to be absent. |
| `NotADirectory { path }` | A directory operation named something that is not a directory. |
| `IsADirectory { path }` | A file operation named a directory. |
| `DirectoryNotEmpty { path }` | A removal without `recursive` named a directory that is not empty. |
| `NotUtf8 { path }` | The file's contents are not UTF-8 where UTF-8 was required. |
| `InvalidPath { path, reason }` | The path or glob pattern is malformed or escapes the root. `path` is the input exactly as supplied, and `reason` is a `PathReason` naming the broken rule. `Wildcard` comes only from glob patterns, and `IntoDescendant` only from renaming into the source's own subtree. |
| `InvalidRange { path, reason }` | A line range for a read was rejected. `reason` is a fixed `&'static str`, so a custom backend cannot build one at runtime. |
| `Anchor { path, anchor, count }` | A `str_replace` anchor did not occur exactly once. `count` is 0 when not found and 2 or more when ambiguous, and `anchor` is empty when the anchor was refused before any search. |
| `PermissionDenied { path, reason }` | A read-only mount or the policy refused the operation, and `reason` names the rule that fired. A verdict that only asks for user approval arrives here too, so read `reason` before you tell the user they were refused. |
| `Unsupported { path, detail }` | The serving backend does not implement the operation, and `detail` says what and why. |
| `Conflict { path, detail }` | The operation touched a path claimed by an access it is not ordered with, such as another live scope. `path` is the path or pattern both claimed, and `detail` names both sides and both claim kinds. |
| `Backend { message }` | The backend failed for any other reason. It is the only variant with no path, and `message` is the only place the backend's cause survives. |

## VfsRef

[`VfsRef`] is the handle to a run's filesystem: its mounts, possibly including the declared store, plus its policy, operation sink, and claims, shared by every clone. Hand it to a run through the `vfs` field of [`RunRequest`](crate::RunRequest), and keep a clone to seed and collect the same files. [`VfsRef::acquire_store`] returns [`VfsError::Unsupported`] on a handle from [`VfsRef::new`] or [`VfsRef::with_policy`], because neither declares a store. Use [`VfsRef::default()`](VfsRef::default), a memory store mounted at `/`, or declare one through [`VfsRef::builder`], as [Give a run your own files](#give-a-run-your-own-files) shows.

- `VfsRef::new` and `VfsRef::with_policy`: no store or sink; `new` allows everything, and `with_policy` checks `policy` before claims, so a denial leaves no claim, event, or change.
- `VfsRef::builder`: returns a builder for mounts, a store, a policy, and an operation sink; the mounts are fixed once `build` runs.
- [`VfsRef::overlay`]: `overlay(prefix, backend)` returns a new handle with `backend` mounted at `prefix`, hiding this handle's files there; other paths reach this handle, which stays unchanged, and both share claims, policy, sink, and store. Panics unless `prefix` is absolute and not `/`.
- [`VfsRef::acquire`]: returns an `Access` resolving paths from `/` across every mount; its scope's claims clash with other live scopes, clones included, until it drops.
- `VfsRef::acquire_store`: `acquire` plus the store view, joining relative paths onto the store's root within its mount and path rules, with errors in your names.
