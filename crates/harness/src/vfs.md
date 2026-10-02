Give a session files of your own, read back what it writes, and guard and watch every file operation.

You need this when an agent must read your program's files, or when you want to keep what it writes after the session closes.

# Where this fits

[Launch an agent](crate#launch-an-agent) starts a [`Session`](crate::Session) with no store of your own. This page replaces that with a handle you build and keep. You seed files before the run, read what it wrote after, and guard and watch every operation along the way.

# Before you start

Every example on this page is code from `desk`, a small Host program that embeds the Harness and runs agents for one operator.

A prompt keeps its working files in a *store*. The store is the set of files a session reads and writes. The prompt's Lua code calls `store.read` and `store.write` on relative paths such as `notes.md`. Every *section* of the prompt shares the same files, and the files stay after the run ends.

A section is one `##` heading of the prompt with the prose and Lua under it, named by its heading text, so the `summarize` prompt below has one section, `Summary`. A deeper heading such as `###` starts a section nested inside it. The [PromptForge language guide](https://cppalliance.github.io/promptforge/language/) covers the store from the prompt's side.

Here is the one prompt the first tour needs.

````
# use harness::{Harness, HarnessConfig};
# let desk = std::env::temp_dir().join("desk-vfs-before-you-start");
# let _ = std::fs::remove_dir_all(&desk);
# let agents = desk.join("agents");
# std::fs::create_dir_all(&agents)?;
// 1. Write `summarize.md`: it declares `notes.md` as its input and `summary.md` as its output.
let summarize = concat!(
    "---\nname: summarize\ndescription: Summarizes the operator's notes\npromptforge: 0\n",
    "models: { writer: {} }\n",
    "input: { path: notes.md, description: The operator's notes }\n",
    "output: { path: summary.md, description: The summary }\n",
    "---\n\n# Summarize\n\n## Summary\n\n```lua\n",
    "store.write('summary.md', models.infer(writer, store.read('notes.md')))\n",
    "```\n",
);
std::fs::write(agents.join("summarize.md"), summarize)?;

// 2. desk launches it by name, like any agent in its folder.
# let harness = Harness::new(HarnessConfig { agents_path: agents, state_dir: desk.join("state") });
assert_eq!(harness.discover(), ["chat", "summarize"]);
# Ok::<(), std::io::Error>(())
````

1. Step 1 writes `summarize.md`. Its frontmatter declares `notes.md` under `input:` as the file it reads, and `summary.md` under `output:` as the file it writes. Its one Lua block reads the notes with `store.read`, passes them to the model, and writes the reply with `store.write`. A prompt reaches its declared files by plain relative path, like any other store file.
2. Step 2 asserts that [`Harness::discover`](crate::Harness::discover) lists `summarize` beside the built-in `chat`. An agent that works through the store launches by name like any other.

# Give a session your own files

Your agent must read files your program provides, and you want to keep what it writes. The store outlives the session: files you place before launch wait for the agent, and files it writes stay for you. Every file operation goes through an *access*, a value you acquire from the store's handle and drop when you are done.

Handing a session a [`VfsRef`] feels like handing a spawned task an `Arc<Mutex<HashMap<String, String>>>`: you keep a clone and look inside when the task is done. Unlike a mutex, the store remembers which files each access touched, so you fill it before launch and read it after close, never mid-run.

A handle attaches each *backend*, the thing that holds files, at a path, and that attachment is a *mount*. The *declared store* is the one mount that the prompt's `store.*` calls and [`VfsRef::acquire_store`] reach, with relative paths such as `notes.md` joined onto its root. The example's hidden lines point the gateway at a local test model server on `127.0.0.1:8080`, whose canned reply echoes the prompt it receives.

````
use harness::vfs::{Origin, VfsRef};
use harness::{LaunchOptions, LaunchRequest, SessionState};
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot};
# use std::error::Error;
# let desk = std::env::temp_dir().join("desk-give-a-session-your-own-files");
# let _ = std::fs::remove_dir_all(&desk);
# let agents = desk.join("agents");
# std::fs::create_dir_all(&agents)?;
# let source = concat!(
#     "---\nname: summarize\ndescription: Summarizes the operator's notes\npromptforge: 0\n",
#     "models: { writer: {} }\n",
#     "input: { path: notes.md, description: The operator's notes }\n",
#     "output: { path: summary.md, description: The summary }\n",
#     "---\n\n# Summarize\n\n## Summary\n\n```lua\n",
#     "store.write('summary.md', models.infer(writer, store.read('notes.md')))\n",
#     "```\n",
# );
# std::fs::write(agents.join("summarize.md"), source)?;
# let harness = Harness::new(HarnessConfig { agents_path: agents, state_dir: desk.join("state") });
# harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
# harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
# harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });

// 1. Build an in-memory store, and keep a clone for desk.
let vfs = VfsRef::default();
let kept = vfs.clone();

// 2. Seed `notes.md` at the exact path `summarize` declares, then drop the view.
let seed = kept.acquire_store(Origin::new("desk seed"))?;
seed.write("notes.md", b"Ship on Friday.")?;
assert!(!seed.exists("Notes.md")?);
drop(seed);

// 3. Move the other clone into the launch options.
let options = LaunchOptions { vfs: Some(vfs), ..LaunchOptions::default() };

// 4. Launch `summarize` with no input text, and wait for `Closed`.
async fn summarize(harness: &Harness, options: LaunchOptions, kept: &VfsRef) -> Result<(), Box<dyn Error>> {
    let request = LaunchRequest { agent: "summarize".into(), args: String::new(), input_text: None };
    let session = harness.launch_with(request, options).await?;
    session.subscribe_state().wait_for(|state| *state == SessionState::Closed).await?;

    // 5. Read the declared output back through a fresh store view.
    let summary = kept.acquire_store(Origin::new("desk collect"))?.read_string("summary.md")?;
    assert_eq!(summary, "You said: Ship on Friday.");
    Ok(())
}
# Ok::<(), Box<dyn Error>>(())
````

1. Step 1 builds [`VfsRef::default()`](VfsRef::default), a memory backend mounted at `/` and declared as the store, and clones it. Clones share the same files, so `kept` is how `desk` reaches them after the session takes the other.
2. Step 2 calls `acquire_store` with an [`Origin`], a label for who is asking, and gets an `Access`: a store view that takes the prompt's own paths and reports errors in those names. Seed each input at exactly the declared path, because paths are case-sensitive and never rewritten, so `Notes.md` and `notes.md` are two files. Then drop the view. An access holds a *claim*, the store's record that it touched a file, on each such file until it drops. A claim clashes with the run's when both touch the same file, at least one writes, and both accesses are still alive. When claims clash, the access that touches the file second fails and never reaches it. A seed view still alive at launch makes the Harness's own check or write of `notes.md` come second, so the run fails before it starts with *run error kind* `Input`. A run error kind is the failure class the Harness records in the run log's row for an ended run. Your own call that comes second returns [`VfsError::Conflict`], as the next tour shows.
3. Step 3 moves the other clone into the `vfs` field of [`LaunchOptions`](crate::LaunchOptions). With `vfs: None`, the default, each run gets a fresh memory store at `/` that your program never holds. You cannot write files into it yourself, but `input_text: Some` still places the declared input file, and you read back only through [`Session::output_text`](crate::Session::output_text).
4. Step 4 defines `summarize`, which launches with [`Harness::launch_with`](crate::Harness::launch_with) and `input_text: None`, then waits for [`SessionState::Closed`](crate::SessionState::Closed). The doc test never calls `summarize`, because it needs the server.
5. Step 5 reads `summary.md` through a fresh view and expects the echo, `You said: Ship on Friday.` Collect only after `Closed`, because until the run ends the agent may rewrite that file, and its claims on it last as long.

No public call returns a run error kind. [`Session::subscribe_errors`](crate::Session::subscribe_errors) reports a run that ends with any kind as one [`SessionFailure`](crate::SessionFailure) whose `kind` is [`FailureKind::RunFailed`](crate::FailureKind::RunFailed) and whose `message` is display text only, and the session then closes. The language guide's [How a failed run is classified](https://cppalliance.github.io/promptforge/language/16-limits-and-errors.html#how-a-failed-run-is-classified) says what each kind means. With no input text, the Harness only checks that the declared input file exists, and fails the run before it starts with run error kind `Input` when it does not. With `input_text: Some`, it writes that text over any file you seeded, so do one or the other, never both.

`Session::output_text` reads the same file for you. The Harness reads it once, as the run completes and before `Closed`, so calling it after `Closed` never races the run. For what each of its errors means, see [`OutputError`](crate::OutputError). Two come from the store:

- [`OutputError::Missing`](crate::OutputError::Missing) when the run completed without writing the file; the run still counts as a success.
- [`OutputError::Vfs`](crate::OutputError::Vfs) when the store refused the read for another reason, such as a refusal by a policy you install, as the next tour shows; only then does [`Error::source`](std::error::Error::source) give the [`VfsError`].

A reused store still holds an earlier run's output file. When the new run never writes it, `Session::output_text` returns the old text, not `OutputError::Missing`, so delete or check the output path before you launch again.

You might expect any `VfsRef` to work as a session's store. Instead, [`VfsRef::new`] and [`VfsRef::with_policy`] declare no store, so `acquire_store` on them returns [`VfsError::Unsupported`]. A prompt with an `input:` file, like `summarize`, then fails at once with run error kind `Input`, because the Harness places that file through `acquire_store`. A prompt with no `input:` file fails with run error kind `Vfs` before any section runs. Use the default handle, or declare a store through [`VfsRef::builder`].

Seed before launch, read after close, and never hold a store access across the run. Next, [Guard and watch a session's files](#guard-and-watch-a-sessions-files) limits and records what the run touches.

# Guard and watch a session's files

Your program shares files with a running session, and you want to limit what the agent touches, see each operation, and keep your accesses from colliding with the run's. A *policy* allows or refuses each operation. An *operation sink* is a callback that hears about each one just before it touches the files.

A policy and a sink feel like middleware: each request passes a check, gets logged, and reaches the backend. Unlike middleware, the store also tracks claims. Each `acquire` or `acquire_store` call starts its own *scope*, the group of accesses a single acquire starts. Yours holds only the `Access` the call returns, since `Access` has no `Clone`; a run's scope holds the run's first access plus the new one it gives each task it spawns. Accesses in one scope can be ordered by spawn and join, and accesses in two different scopes never are.

The prompt's Lua starts a *task* with `tasks.spawn`, which runs a named section beside the code that spawned it, and each arm of a `fanout` call is a task too. Inside a run, one access is *ordered* before another only by a task spawn or a join. A spawn puts everything the spawner did before it ahead of the task, and a join, such as `tasks.join` delivering the task's result, puts everything the task did ahead of the spawner's next step. The language guide's [store chapter](https://cppalliance.github.io/promptforge/language/09-the-store.html#sharing-the-store-across-calls-and-tasks) covers the rest.

Nothing spawns or joins across scopes, so when your access and the run's touch one path, at least one writes, and both are alive, they conflict. Spawn and join, not timing, decide whether two accesses conflict, so an overlap between two live accesses fails on every run instead of only under unlucky timing. Which side fails does depend on timing, because the access that touches the path second is the one refused.

Each operation passes through its `Access` in order: the policy, then the claims, then the sink, then the files. This module re-exports only the three types the example's first `use` line names. `MemoryBackend`, `Op`, `Policy`, `Verdict`, and `VfsPath` live in [`promptforge::vfs`](promptforge::vfs), the module those three come from. To name them, add the `promptforge` crate to your program's dependencies beside `harness`.

````
use harness::vfs::{Origin, VfsError, VfsRef};
use promptforge::vfs::{MemoryBackend, Op, Policy, Verdict, VfsPath};
use std::sync::{Arc, Mutex};
# use harness::{CatalogBinding, GatewayBinding, Harness, HarnessConfig, HostSnapshot};
# use harness::{LaunchOptions, LaunchRequest, Session, SessionState, WaitFrame};
# use std::error::Error;
# let desk = std::env::temp_dir().join("desk-guard-and-watch-a-sessions-files");
# let _ = std::fs::remove_dir_all(&desk);
# let agents = desk.join("agents");
# std::fs::create_dir_all(&agents)?;
# let source = concat!(
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
# std::fs::write(agents.join("review.md"), source)?;
# let harness = Harness::new(HarnessConfig { agents_path: agents, state_dir: desk.join("state") });
# harness.set_gateway(GatewayBinding { base_url: "http://127.0.0.1:8080".into(), key: "desk-key".into(), generation: 1 });
# harness.set_catalog(CatalogBinding { generation: 1, models: vec![[("id", "stub-model")].into_iter().collect()] });
# harness.set_host(HostSnapshot { selected_model: Some("stub-model".into()), ..HostSnapshot::default() });
# fn review() -> LaunchRequest {
#     LaunchRequest { agent: "review".into(), args: String::new(), input_text: None }
# }
# async fn question(session: &Session) -> Result<String, Box<dyn Error>> {
#     let mut waits = session.subscribe_waits();
#     session.resend_waits();
#     loop {
#         if let WaitFrame::Required { token } = waits.recv().await? {
#             return Ok(token);
#         }
#     }
# }

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

// 4. Launch `review`, and wait until it has read the notes and asks the operator.
async fn guard(harness: &Harness, vfs: &VfsRef, seen: &Mutex<Vec<(String, String)>>) -> Result<(), Box<dyn Error>> {
    let options = LaunchOptions { vfs: Some(vfs.clone()), ..LaunchOptions::default() };
    let session = harness.launch_with(review(), options).await?;
    let token = question(&session).await?;

    // 5. desk's write to the path the run has claimed conflicts and never lands.
    let edit = vfs.acquire_store(Origin::new("desk edit"))?.write("notes.md", b"Ship on Monday.");
    assert!(matches!(edit, Err(VfsError::Conflict { .. })));

    // 6. Answer, wait for `Closed`, then check the notes and the run's origin.
    session.send_input(&token, "Approved.".into(), || {})?;
    session.subscribe_state().wait_for(|state| *state == SessionState::Closed).await?;
    assert_eq!(vfs.acquire_store(Origin::new("desk check"))?.read_string("notes.md")?, "Ship on Friday.");
    assert!(seen.lock().unwrap().iter().any(|(_, file)| file == "Review"));
    Ok(())
}
# Ok::<(), Box<dyn Error>>(())
````

1. Step 1 defines `DeskFiles`, a `Policy` that allows only the two declared files. It sees full paths, so `notes.md` arrives as `/notes.md`.
2. Step 2 builds the handle with [`VfsRef::builder`], the only constructor that declares a store and installs a sink. The sink receives an `OpEvent` with three getters: `op()` for the operation kind, `path()` for the full path, and `origin()` for the [`Origin`] of the access that made it. The sink hears only operations that passed the policy and the claims, once per path, so a rename reports twice.
3. Step 3 seeds `notes.md`, and the stray write to `secret.md` gets [`VfsError::PermissionDenied`]. Each view there is a temporary, dropped at the end of its statement. The sink holds only the seed, stamped with this file by [`Origin::new`], because a denied operation registers no claim, fires no event, and changes nothing.
4. Step 4 defines `guard`, which launches the hidden `review` agent and waits for its question. At its question, `review` has read `notes.md`, so the run holds a live claim on it. The doc test never calls `guard`.
5. Step 5 writes `notes.md` and gets [`VfsError::Conflict`]. `review` only read `notes.md`, but a read claims its path too, so what a run reads never depends on timing. Without that claim, `review`'s summary would be built from `Ship on Friday.` or `Ship on Monday.` depending on whether this edit landed before or after its read. With it, the overlap fails whichever side comes second, so never write a file the run has read, not only the files it writes, until `Closed`. Clones share claims, so cloning does not help.
6. Step 6 answers, waits for `Closed`, and finds `notes.md` unchanged. It then finds a sink event whose file is `Review`, the prompt's H1 title, not a path, since a prompt may never exist on disk. The run's `label` is that title too, except in a task the run spawns, which uses its section's name. Your operations hold a Rust file path in `file`, and so do the Harness's own `input: <path>` and `output: <path>` operations, because the Harness labels them with `Origin::new` in its own source file. Tell the run's operations by `file`, which holds the prompt's title, and tell the Harness's from yours by `label`.

Steps 4 and 5 look like this:

````text
   run's access                 notes.md                   desk edit view
        │                          │                              │
        │  store.read('notes.md')  │                              │
        ├─────────────────────────>│  claimed by the run          │
        │  input.ask() waits       │                              │
        │                          │  write("notes.md")           │
        │                          │<─────────────────────────────┤
        │                          │  VfsError::Conflict          │
        │                          ├─────────────────────────────>│
        │                          │  (the file stays as it was)  │
````

Which access reports a conflict does depend on timing: the one that touches the path second fails and never reaches the files. Yours returns the conflict, whose `detail` names both sides; a run access ends the run with run error kind `Determinism`, which the prompt cannot catch. The first write stays either way, so check the file before you retry.

A denial reaches the prompt as a store error with reason `permission_denied`, which it can catch with `pcall`, Lua's protected call; uncaught, it ends the run with kind `Vfs`. Your own `Access` gets `PermissionDenied` with the verdict text in `reason`. A third variant, `Verdict::Ask(String)`, for an operation that needs user approval, arrives exactly as [`Verdict::Deny`](promptforge::vfs::Verdict::Deny) does, so read `reason` to tell them apart.

The builder takes the policy by value, so to change it mid-run, have it read shared state such as an `Arc<Mutex<Verdict>>` field and keep a clone; the next operation sees the change.

The Harness's own file work, labeled `input: <path>` before the run and `output: <path>` after it, passes through your policy and sink too. A policy that refuses the input fails the run with kind `Input`, and one that refuses the output read makes [`Session::output_text`](crate::Session::output_text) return [`OutputError::Vfs`](crate::OutputError::Vfs).

`Access` has no `Clone`, so each `acquire` gives you exactly one `Access`, and dropping it ends its scope and its claims; keep each one short.

You might expect a Host write to wait for the run's access, the way a `Mutex` would. Instead, a store operation fails at once with `VfsError::Conflict`, so retry only after the other access drops.

The policy decides, the sink watches, `Origin` labels, and claims catch overlaps. The [Reference](#reference) covers only `Origin`, [`VfsError`], and [`VfsRef`], the three types this module exports. For [`Access`](promptforge::vfs::Access), `Policy`, `Verdict`, `Op`, and `OpEvent`, see `promptforge::vfs`; `Access` lists the operations the `VfsError` table names, such as `str_replace`, `read_range`, and `remove` with its `recursive` flag.

# Reference

## Origin

[`Origin`] labels who asked for an access. Each operation that passes the policy and the claims reaches the operation sink with that label and a source position; a refused one fires nothing. `Origin` never decides whether an operation is allowed. Pass one to [`VfsRef::acquire`] or [`VfsRef::acquire_store`], one per caller you want to tell apart, because an access keeps its origin for life. The struct is `#[non_exhaustive]`, so build it only through the two constructors below, as taught in [Guard and watch a session's files](#guard-and-watch-a-sessions-files).

- [`Origin::new`]: takes the label from its argument and the file and line from your call site; mark any wrapper `#[track_caller]` too.
- [`Origin::at`]: stores the label, file, and line exactly as given, for a position that is not a Rust call site, such as a prompt line.
- `label`: the most specific label you have, such as a section name, a tool id, or a fixture name.
- `file`: your Rust file for `Origin::new`, the given document for `Origin::at`, or the prompt's H1 title, not a path, for the run's operations.
- `line`: the 1-based line within `file`.

## VfsError

[`VfsError`] is the one error every file operation returns, and its variant says what kind of failure it was. Match on it to decide what to do next, and keep a wildcard arm, because the enum is `#[non_exhaustive]`. Its variants are not, so a custom backend builds them as struct literals, and `source()` always returns `None`. Through a store view, path fields hold the name you passed, not the full path, as in [Give a session your own files](#give-a-session-your-own-files).

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

[`VfsRef`] is the handle to a session's filesystem: its mounts, possibly including the declared store, plus its policy, operation sink, and claims, shared by every clone. Hand it to a session through the `vfs` field of [`LaunchOptions`](crate::LaunchOptions), and keep a clone to seed and collect the same files. [`VfsRef::acquire_store`] returns [`VfsError::Unsupported`] on a handle from [`VfsRef::new`] or [`VfsRef::with_policy`], because neither declares a store. Use [`VfsRef::default()`](VfsRef::default), a memory store mounted at `/`, or declare one through [`VfsRef::builder`], as [Give a session your own files](#give-a-session-your-own-files) shows.

- `VfsRef::new` and `VfsRef::with_policy`: no store or sink; `new` allows everything, and `with_policy` checks `policy` before claims, so a denial leaves no claim, event, or change.
- `VfsRef::builder`: returns a builder for mounts, a store, a policy, and an operation sink; the mounts are fixed once `build` runs.
- [`VfsRef::overlay`]: `overlay(prefix, backend)` returns a new handle with `backend` mounted at `prefix`, hiding this handle's files there; other paths reach this handle, which stays unchanged, and both share claims, policy, sink, and store. Panics unless `prefix` is absolute and not `/`.
- [`VfsRef::acquire`]: returns an `Access` resolving paths from `/` across every mount; its scope's claims clash with other live scopes, clones included, until it drops.
- `VfsRef::acquire_store`: `acquire` plus the store view, joining relative paths onto the store's root within its mount and path rules, with errors in your names.
