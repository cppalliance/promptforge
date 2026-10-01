Give your runs their files: a shared store, real directories beside it, and rules about what a run may change.

You need this when your prompts read or write files, or several runs share a folder. A run reaches files only through one handle your program builds, and the handle checks every operation before any backend sees it. By the end of this page, you build that handle, gate it by mode, and log every file operation it lets through.

# Where this fits

The tours on the [crate page](crate) walk through the greeter, a small prompt whose Lua writes a note to the store and reads it back. They answer its store calls with [`perform_store_op`], the function your program calls to carry out each store call a run asks for, against the default in-memory store.
This page builds the handle those answers go through, so you decide which files a run sees and what it may change.

# Give a run its files

You want your prompts to see a real directory and a store you control, shared by several runs. The run's set of virtual files, shared by every [section](crate#before-you-start) of a prompt, is the *store*.

Building a handle feels like setting up mount points on Unix: each backend serves the paths under its prefix. Unlike a real filesystem, every run's file work goes through this one handle, and the handle remembers which run touched which path. The handle is a [`VfsRef`], one of its mounts is the store, and it refuses file work from two runs that would race.

````
use promptforge::timestamp::Timestamp;
use promptforge::vfs::{MemoryBackend, Origin, RealBackend, VfsError, VfsRef};
use promptforge::{Environment, Prompt, RunContext};

// 1. Make a real folder, mount it at `/`, and declare a memory store at `/my/store`.
let dir = std::env::temp_dir().join(format!("vfs-tour-{}", std::process::id()));
std::fs::create_dir_all(&dir)?;
let vfs = VfsRef::builder()
    .mount("/", RealBackend::rooted(&dir)?)
    .store("/my/store", MemoryBackend::new())
    .build();

// 2. Parse the greeter, and prepare two runs over clones of the one handle.
let source = concat!(
    "---\n",
    "name: greeter\n",
    "description: Writes a note to the store and reads it back.\n",
    "promptforge: 0\n",
    "---\n\n",
    "# Greeter\n\n",
    "## Greet\n\n",
    "```lua\n",
    "store.write('note.md', 'hello')\n",
    "return store.read('note.md')\n",
    "```\n",
);
let (parsed, _parse_events) = Prompt::parse(source, "greeter");
let prompt = parsed?;
let environment = Environment::new();
let run_a = RunContext::new("run-a", 7, Timestamp::UNIX_EPOCH).vfs(vfs.clone());
let run_b = RunContext::new("run-b", 7, Timestamp::UNIX_EPOCH).vfs(vfs);
let (ctx_a, _requirements) = environment.prepare(&prompt, run_a);
let (ctx_b, _requirements) = environment.prepare(&prompt, run_b);

// 3. The first run writes the real file `greeting.txt`, and the write lands.
let access_a = ctx_a.vfs_handle().acquire(Origin::new("run-a"))?;
access_a.write("/greeting.txt", b"from a")?;

// 4. The second run writes the same file while the first still holds it.
let access_b = ctx_b.vfs_handle().acquire(Origin::new("run-b"))?;
let second = access_b.write("/greeting.txt", b"from b");
assert!(matches!(second, Err(VfsError::Conflict { .. })));
assert_eq!(std::fs::read(dir.join("greeting.txt"))?, b"from a");
std::fs::remove_dir_all(&dir)?;
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. [`VfsRef::builder`] starts a [`VfsRefBuilder`]. [`VfsRefBuilder::mount`] puts [`RealBackend::rooted`] over the folder at `/`, and [`VfsRefBuilder::store`] mounts a [`MemoryBackend`] at `/my/store` and declares it the store. Each path goes to the mount with the longest matching prefix, which sees it with the prefix stripped, so `/my/store/note.md` reaches the store as `/note.md`.
2. Step 2 prepares two runs of the crate page's greeter. [`RunContext::new`](crate::RunContext::new) takes the run's name, the seed `7`, and the `sys.when` instant [`Timestamp::UNIX_EPOCH`](crate::timestamp::Timestamp::UNIX_EPOCH); [Run a prompt](crate#run-a-prompt) explains the seed. [`RunContext::vfs`](crate::RunContext::vfs) hands each context a clone of the one handle, and [`Environment::prepare`](crate::Environment::prepare) keeps it, so both runs share the same mounts and claims.
3. [`RunContext::vfs_handle`](crate::RunContext::vfs_handle) returns the clone you gave the context, so `acquire` here is [`VfsRef::acquire`] on the one shared handle. Step 3 acquires an [`Access`], labeled by an [`Origin`], standing in for the first run's file work. A claim is the handle's record that an access read or wrote a path, and a scope is the group of file work the handle treats as ordered, so its own claims never clash. Every `acquire` starts its own scope, and two live scopes touching one path conflict. This access's claim on `/greeting.txt` lasts until it drops, since the two contexts are prepared but never run.
4. The second context writes the same path. The write fails with [`VfsError::Conflict`], never reaches the backend, and the file keeps `from a`. Without this check, the final text would depend on timing. Match `Conflict` to tell a race from a backend failure.

The diagram shows where each path goes.

````text
                ┌──────────────────────────────────────┐
                │  VfsRef: one handle, cloned per run  │
                └──────────┬────────────────┬──────────┘
   /greeting.txt           │                │   /my/store/note.md
                           v                v
          ┌──────────────────────┐   ┌──────────────────────────┐
          │ mount "/"            │   │ mount "/my/store"        │
          │ RealBackend::rooted  │   │ MemoryBackend, the store │
          │ sees /greeting.txt   │   │ sees /note.md            │
          └──────────────────────┘   └──────────────────────────┘

     each path goes to the mount with the longest matching prefix,
     and that backend sees the path with the prefix stripped
````

Reads claim paths too, so handle `Conflict` on `exists`, `stat`, and reads as well as writes.

An access a run hands you inside an [`Effect::Store`](crate::effect::Effect::Store) loses its claims when the run reaches [`Step::Done`](crate::Step::Done) or is dropped, and every later operation on it fails with `PermissionDenied`.

Share the one handle, never just one backend, because two handles over clones of one `MemoryBackend` share files but not claims, so their races go undetected.

[`VfsRef::new`] and [`VfsRef::with_policy`] declare no store, so [`VfsRef::acquire_store`] on them fails with [`VfsError::Unsupported`]. When your prompts use the store, call `store` on the builder, or start from [`VfsRef::default()`](VfsRef::default), a memory store mounted at `/` and declared the store.

A plain `acquire` access names files by their full virtual path, such as `/my/store/note.md`. A store view from `VfsRef::acquire_store`, and every `store.*` call in a prompt, names the same file relative to the store root, as `note.md`, so prompts never name your mount layout. A store path such as `/draft.md` fails with [`VfsError::InvalidPath`] and [`PathReason::Absolute`]. A path no mount covers returns [`VfsError::NotFound`], so mount `/` when every path should be served.

You might expect the second run's write to the same real file to replace the first, as [`std::fs::write`] would. Instead, while the first run's scope is live, the second write fails with `VfsError::Conflict`, and the file keeps the first run's text.

Build one handle, mount your folders, declare one store, and let the handle refuse races between runs. Next, [Keep a run from changing files](#keep-a-run-from-changing-files) adds rules about what a run may change.

# Keep a run from changing files

You want a run to read freely, but change files only when the mode your user picked allows it. The setting that decides what the model may change right now is the *mode*.

A rule that sees every operation first works like middleware in a web server: it sees every request before the handler does. Unlike middleware, it only answers allow, deny, or ask, and it never changes the operation. That rule is a [`Policy`], and it answers every operation before any backend sees it. [`ModePolicy`] is a policy that answers from a mode your program can switch at any moment.

````
use std::sync::Arc;

use promptforge::effect::{Effect, EffectAnswer};
use promptforge::vfs::{perform_store_op, Mode, ModePolicy, VfsError};
use promptforge::{Run, RunResult, Step};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::{MemoryBackend, Origin, VfsRef};
# use promptforge::{Prompt, RunContext};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
    // 1. The greeter's Lua now writes `note.txt`, a path Plan mode refuses, and reads it back.
    "store.write('note.txt', 'hello')\n",
    "return store.read('note.txt')\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");

// 2. Keep the mode handle, then install the policy on a handle that declares a memory store.
let policy = ModePolicy::new(Mode::Agent);
let mode = policy.handle();
let vfs = VfsRef::builder().store("/", MemoryBackend::new()).policy(policy).build();

// 3. Run the greeter over the gated handle, and Agent mode lets its write land.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).vfs(vfs.clone());
let mut run = Run::new(Arc::new(parsed?), "", ctx);
let result = loop {
    match run.step() {
        Step::Pending { effects, .. } => {
            for (id, _provenance, effect) in effects {
                let answer = match effect {
                    Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
                    _ => EffectAnswer::Dropped,
                };
                run.resume(id, answer);
            }
        }
        Step::Done { result, .. } => break result,
    }
};
assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

// 4. Switch to Plan mode, and the next write to `note.txt` is refused.
mode.set(Mode::Plan);
let store = vfs.acquire_store(Origin::new("host"))?;
let refused = store.write("note.txt", b"changed");
assert!(matches!(&refused, Err(VfsError::PermissionDenied { reason, .. }) if reason.contains("Plan")));
assert_eq!(store.read_string("note.txt")?, "hello");
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. Step 1 changes only the greeter's two Lua lines, so it writes and reads `note.txt` instead of `note.md`. Nothing in the prompt names a mode, so the same store calls meet whatever rules the handle carries.
2. [`ModePolicy::new`] takes the starting mode, here [`Mode::Agent`]. [`Mode`] has no default, so the run starts under the rules you chose, never a hidden default. [`ModePolicy::handle`] returns a [`ModeHandle`]. Take it before [`VfsRefBuilder::policy`] moves the policy in by value, because once installed, the policy is out of reach, and the `ModeHandle` is your only way to switch. The builder also declares a memory store at `/`. [`VfsRef::with_policy`] gates a handle the same way, but it declares no store, so this run would fail at its first step.
3. Step 3 runs the greeter over a clone of the gated handle and answers each store effect with [`perform_store_op`]. Agent mode allows every operation, so the run's write of `note.txt` lands and its read returns `hello`.
4. [`ModeHandle::set`] switches to [`Mode::Plan`], and [`VfsRef::acquire_store`] gives a fresh store view. Its write of `note.txt` fails at once with [`VfsError::PermissionDenied`], whose reason names Plan mode, and the file keeps the run's text. The reason names the operation, the path, and the mode, and it is the text the model reads to recover. A refused change writes nothing, so there is no partial change to clean up, and a refused write to a new path leaves no file at all.

Pick the mode by what the model may change right now. `Mode::Agent` allows every operation, `Mode::Plan` allows changes only to paths ending in `.md`, and [`Mode::Ask`] refuses every change. Reads, `exists`, `glob`, `list`, and `stat` always pass. `str_replace` reaches a policy as [`Op::Write`], since [`Op`] has no separate edit kind, so a policy that allows writes also allows edits.

In Plan mode, name files and folders with a lowercase `.md` on every side. The markdown test is case-sensitive, so `NOTES.MD` is refused. A `mkdir` passes only when the directory name itself ends in `.md`. A copy or rename is checked for each path, so a non-markdown source, or either side of a rename, is refused.

A mode switch applies mid-run, at once. The policy reads the mode fresh on every operation, so a `set` takes effect on the very next operation of every access already acquired, with no help from the run.

You might expect `Mode::Ask` to hold a write until the user approves it. Instead, the write fails at once with `VfsError::PermissionDenied`, the same error a denial gives, told apart only by the reason text. Build any approval flow in your own program, outside the handle.

The policy checks every operation first, and your mode switch counts from the very next one. Next, [Watch every file operation](#watch-every-file-operation) logs what the policy lets through.

# Watch every file operation

You want a log of every file operation a run makes, with who asked for it.

A closure that sees each file operation as it happens is a *watcher*. It works like a [`tracing` subscriber](https://docs.rs/tracing/latest/tracing/trait.Subscriber.html), which sees each event as it happens. Unlike a subscriber, it sees only the operations the rules let through, and it never learns their outcome. The watcher fires once for each admitted operation, after the policy and the race check pass and before the backend runs.

````
use std::sync::{Arc, Mutex};

use promptforge::vfs::{MemoryBackend, Op, OpEvent, VfsRef};
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_store_op;
# use promptforge::{Prompt, Run, RunContext, RunResult, Step};
# let source = concat!(
#     "---\n",
#     "name: greeter\n",
#     "description: Writes a note to the store and reads it back.\n",
#     "promptforge: 0\n",
#     "---\n\n",
#     "# Greeter\n\n",
#     "## Greet\n\n",
#     "```lua\n",
#     "store.write('note.md', 'hello')\n",
#     "return store.read('note.md')\n",
#     "```\n",
# );
# let (parsed, _parse_events) = Prompt::parse(source, "greeter");

// 1. Install a watcher that prints each event and records its kind, path, and origin label.
let log = Arc::new(Mutex::new(Vec::new()));
let sink_log = Arc::clone(&log);
let vfs = VfsRef::builder()
    .store("/", MemoryBackend::new())
    .on_op(move |event: OpEvent<'_>| {
        println!("{:?} {} by {}", event.op(), event.path(), event.origin().label);
        if let Ok(mut entries) = sink_log.lock() {
            entries.push((event.op(), event.path().to_string(), event.origin().label.clone()));
        }
    })
    .build();

// 2. Run the store-only greeter over the watched handle.
let ctx = RunContext::new("greeter", 7, Timestamp::UNIX_EPOCH).vfs(vfs);
let mut run = Run::new(Arc::new(parsed?), "", ctx);
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 let answer = match effect {
#                     Effect::Store { access, op } => EffectAnswer::Store(perform_store_op(&access, op)),
#                     _ => EffectAnswer::Dropped,
#                 };
#                 run.resume(id, answer);
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

// 3. The note's path saw a write and then a read, both asked for by the same origin.
let entries = log.lock().map_err(|_| "the watcher panicked")?;
let note: Vec<_> = entries.iter().filter(|(_, path, _)| path == "/note.md").collect();
assert!(matches!(note.as_slice(), [(Op::Write, _, first), (Op::Read, _, second)] if first == second));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. [`VfsRefBuilder::on_op`] installs a closure that receives an [`OpEvent`] for each admitted operation. A handle has no watcher unless you install one. [`OpEvent::op`], [`OpEvent::path`], and [`OpEvent::origin`] give the kind, the canonical path, and the [`Origin`] label passed to `acquire`. The event borrows the values of the access that ran the operation, not the handle's, so firing allocates nothing, and the event is valid only until the watcher returns. A watcher that keeps events must clone out of them, which is why this one copies the path with `to_string` and the label with `clone`.
2. Step 2 hands the watched handle to the context with [`RunContext::vfs`](crate::RunContext::vfs) and drives the greeter with the same loop as the previous tour. The run still returns `hello`, so the watcher changes nothing about the run.
3. Step 3 filters the log to `/note.md`. The run's `store.write` fired one [`Op::Write`] and its `store.read` one [`Op::Read`], in that order, under the same origin label. The origin only labels the event and never decides what is allowed. Give each run its own origin label, so the log says who asked for each operation.

A slow watcher slows every file operation, because it runs on whatever thread performs the operation, before the operation continues. For a store call, that is the thread that calls [`perform_store_op`], which the reference advises be a blocking thread rather than your async executor. Send events to a channel, and do the heavy work elsewhere.

Some operations report under another kind, so match these when you filter the log. `read_range` and `read_range_numbered` report as `Op::Read`, `str_replace` as `Op::Write`, and `remove` as [`Op::Delete`]. A `rename` or `copy` fires one event for each path, so expect two lines for one move. Nesting handles never duplicates lines: an overlay, the handle [`VfsRef::overlay`] returns, adds one mount over an existing handle and shares its claims, policy, and watcher. A [`VfsRef`] is itself a backend, so an operation through a handle passed to another's `mount` fires once, under the outer caller's origin.

You might expect the watcher to see a refused write, as a request log records failed requests. Instead, only operations that passed the rules fire, and the watcher never learns whether the backend then succeeded. The watcher records what the handle let through, so log failures from the errors your program gets back.

The watcher sees what the rules let through, as it happens, so keep it cheap. Next, [`cancel`](crate::cancel) shows how to stop runs from any thread.

# Reference

## Access

[`Access`] is a run's capability to touch storage, from [`VfsRef::acquire`] or [`VfsRef::acquire_store`]. Its claims, reads included, last until it drops, or, for an access a run hands you in a store effect, until the run reaches [`Step::Done`](crate::Step::Done) or is dropped. A clash with another live access fails with [`VfsError::Conflict`] before the backend. A policy refusal, or any operation after its run ends, fails with `PermissionDenied`; drop the claiming access, or switch modes. See [Give a run its files](#give-a-run-its-files).

- [`Access::read_range`]: lines `start` through `end`, counted from 1, joined with `\n`; `start` 0 or `end < start` is `InvalidRange` before the file is read.
- [`Access::str_replace`]: replaces the one non-overlapping match of `old`; an empty, absent, or repeated `old` returns `Anchor` and leaves the file unchanged.
- [`Access::remove`]: returns `Ok(false)` for a missing path instead of `NotFound`, and `Ok(true)` when it removed something.
- [`Access::rename`]: like `copy`, returns `Unsupported` between two different mounts; move such data by reading and writing it yourself.
- [`Access::mkdir`]: fails with `AlreadyExists` when the path exists, even with `recursive` set, unlike [`std::fs::create_dir_all`].

## AcquireContext

[`AcquireContext`] tells a custom backend's [`Vfs::acquire`] which identity is being acquired and, opaquely, the scope it belongs to. You meet it when you write or wrap a backend and attribute operations to an identity. A backend that wraps another [`Vfs`] passes the context through unchanged, so the wrapped session joins the caller's scope. It has no public constructor, because only the handle builds one, so you can pass one along but cannot make one for a test.

- [`AcquireContext::id`]: every operation on the session this acquire returns is attributed to this id.

## AllowAll

[`AllowAll`] is the policy that allows every operation, for a handle you do not want gated. [`VfsRef::new`] installs it, and [`VfsRefBuilder::build`] installs it when you set no policy, so leave it alone unless you need gating.

## Entry

[`Entry`] is one directory entry from [`Access::list`]: a name within its directory paired with its metadata. You read it when you walk a directory's listing. Both shipped backends return entries sorted by name, but a custom backend may not, so sort them yourself if order matters. It is `#[non_exhaustive]` with public fields and no constructor, so you read entries but cannot build one.

- `Entry.name`: the entry's name within its directory, not a full path.
- `Entry.stat`: the entry's [`Stat`]; the real-filesystem backend takes it without following symlinks.
- `Entry.description`: an annotation column that both shipped backends leave `None`.

## ExecId

[`ExecId`] identifies one serial thread of execution, unique within the process. A custom backend sees it through [`AcquireContext::id`] and [`Vfs::release`]. It has no public constructor, and every acquire and every spawn gets a fresh id, so you cannot reuse an id to rejoin an earlier scope.

## RealBackend

[`RealBackend`] serves real directories behind the virtual namespace, as in `.mount("/", RealBackend::rooted(&dir)?)`. [`RealBackend::rooted`] fails with `NotFound` when `dir` is absent and `NotADirectory` when it is not a directory, spelling the path as you gave it, so create the folder before building the backend. Writes, copies, and renames are failure-atomic, so a failed write leaves the old file whole. See [Give a run its files](#give-a-run-its-files).

- [`RealBackend::identity`]: applies no containment, so virtual paths are real paths; on Windows, virtual `/C:/a/b` is the real path `C:\a\b`.
- `RealBackend::rooted`: checks every resolved path against the folder, denies a followed symlink that resolves outside it, and refuses removing or renaming the mounted root.
- [`RealBackend::with_read_only`]: `true` makes every mutation fail with `PermissionDenied` naming the path; a new backend is writable.

## MemoryBackend

[`MemoryBackend`] is an in-memory backend holding bytes keyed by canonical path, for a store or a scratch mount with no disk. Writing where a directory sits fails with `IsADirectory`, and writing beneath a file fails with `NotADirectory` naming that file; remove or rename what is in the way. Clones share the same storage, but each [`VfsRef`] built over a clone gets its own claims table. See [Give a run its files](#give-a-run-its-files).

- [`MemoryBackend::new`]: an empty backend holding only the root directory, the same as `Default`.

## ModeHandle

[`ModeHandle`] is your program's half of the mode gate: it reads and switches the mode a [`ModePolicy`] enforces. Use it when the user switches between Ask, Plan, and Agent while runs are live. A [`ModeHandle::set`] takes effect on the very next operation of every access already acquired, with no executor involvement, so a switch mid-run needs no restart. Clones share one mode cell, so hand a clone to each part of your UI. See [Keep a run from changing files](#keep-a-run-from-changing-files).

## ModePolicy

[`ModePolicy`] gates mutations by the current [`Mode`] and never gates reads; install it as in `VfsRef::with_policy(VfsRef::default(), ModePolicy::new(Mode::Plan))`. In `Ask`, every mutation fails with `PermissionDenied`, whose reason names the operation, the path, and the mode. In `Plan`, a mutation of a path not ending in `.md` fails the same way, and nothing is written. Switch the mode with [`ModeHandle::set`], or write to a markdown path. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`ModePolicy::handle`]: returns a [`ModeHandle`] sharing this policy's mode cell; take it before you hand the policy over by value.

## OpEvent

[`OpEvent`] is one admitted operation as a watcher sees it: its kind, its canonical path, and the origin of the access that admitted it. Your sink, installed with [`VfsRefBuilder::on_op`], reads it to log or display file activity. It fires after the policy and claims checks pass and before the backend runs, so an event means allowed, not succeeded. The sink runs inline, so keep it cheap, and clone what you keep, because the event borrows the access's values. See [Watch every file operation](#watch-every-file-operation).

- [`OpEvent::op`]: `rename` and `copy` fire one event per path, and an operation routed through a mounted handle fires once, under the outer caller's origin.

## Origin

[`Origin`] says who asked for an operation: a label and the most precise source position the caller knows. You pass one to every acquire, as in `vfs.acquire(Origin::new("run-a"))`. It never gates an operation or appears in a claim; it only labels events, so two accesses with the same label still conflict. Its fields are public, but it is `#[non_exhaustive]`, so read the fields, and build one only through `new` or `at`. See [Watch every file operation](#watch-every-file-operation).

- [`Origin::new`]: stamps the calling Rust file and line through `#[track_caller]`.
- [`Origin::at`]: holds an explicit position, such as a prompt's name and line.
- `Origin.label`: should be the most specific label you have: a section name, a tool id, or a fixture name.
- `Origin.file`: the Rust source file for `new`, and the prompt's name for `at`.
- `Origin.line`: counted from 1.

## Stat

[`Stat`] is the metadata for one path: its node kind, its size, and an optional mode and timestamps. You get one from [`Access::stat`] or `Entry.stat`. A backend that does not track a field reports `None`, so handle `None` for every optional field. The real-filesystem backend stats a final-component symlink as the link itself, so a link to a directory reports `Symlink`. It is `#[non_exhaustive]` with public fields and no constructor.

- `Stat.size`: in bytes; a directory reports 0 on the memory backend, which also leaves `mode`, `modified`, and `created` all `None`.

## VfsPath

[`VfsPath`] is a canonical virtual path, rooted and `/`-separated, which a sink reads from [`OpEvent::path`]. It has no public constructor, so a path you receive already came through canonicalization. Case is significant, so `/ReadMe.md` and `/readme.md` are different paths; compare paths exactly. Clones share one allocation, so cloning a path out of an event is cheap. See [Watch every file operation](#watch-every-file-operation).

- [`VfsPath::to_buf`]: gives an owned [`VfsPathBuf`] that outlives the event it came from.

## VfsPathBuf

[`VfsPathBuf`] is an owned canonical virtual path, for keeping a path past an event's borrow. Get one from [`VfsPath::to_buf`] or `From<VfsPath>`. Unlike [`VfsPath`], it is `Ord`, so you can sort it or key a [`BTreeMap`](std::collections::BTreeMap) with it.

## VfsRef

[`VfsRef`] is the cloneable handle over your backends and their claims ledger; clones share the backends, the claims, and the policy. Build one, clone it into each run's context, and acquire accesses from it. Every [`VfsRef::acquire`] starts a new scope, and two live scopes' claims conflict. [`VfsRef::acquire_store`] starts its own scope too, and fails with `Unsupported`, detail "the handle declares no store", when no store was declared; declare one with [`VfsRefBuilder::store`]. See [Give a run its files](#give-a-run-its-files).

- [`VfsRef::new`]: installs [`AllowAll`], no op sink, and no store.
- [`VfsRef::with_policy`]: takes the policy by value; to change behavior mid-run, keep shared state the policy reads, as [`ModeHandle`] does.
- [`VfsRef::overlay`]: shares the base's claims, policy, sink, and store; routes paths outside `prefix` through the base; panics when `prefix` is `/` or not absolute.
- `VfsRef::acquire_store`: reaches the store's own mount alone, refuses every path [`PathReason`] names for store paths, and reports errors in your relative form.
- [`VfsRef::default()`](VfsRef::default): a memory store mounted at `/` and declared the store, so relative paths address the store directly.

## VfsRefBuilder

[`VfsRefBuilder`] installs mounts, a store, a policy, and an op sink, then builds a [`VfsRef`]. The longest matching prefix wins, each backend sees paths with its prefix stripped, and a path no mount covers returns `NotFound`. [`VfsRefBuilder::mount`] and [`VfsRefBuilder::store`] panic when the prefix is not an absolute virtual path or a mount already sits there, so give each mount its own absolute prefix. See [Give a run its files](#give-a-run-its-files).

- `VfsRefBuilder::store`: declares the store every `acquire_store` call is scoped to; a second call keeps only the last declaration, while both stay mounted.
- [`VfsRefBuilder::on_op`]: installs the sink that receives an [`OpEvent`] for every admitted operation.
- [`VfsRefBuilder::build`]: fixes the mounts, and installs [`AllowAll`] and no sink when none was set.

## FileType

[`FileType`] names the node kind in a [`Stat`], with all seven POSIX kinds named rather than lumped together. Match `Stat.file_type` to tell files from directories. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm, because new kinds can arrive. The memory backend reports only `File` or `Directory`, so the other arms matter only for real directories.

| Variant | Meaning |
|---|---|
| `File` | a regular file |
| `Directory` | a directory |
| `Symlink` | a symbolic link, reported as the link itself |
| `Fifo` | a named pipe |
| `Socket` | a socket |
| `CharDevice` | a character device |
| `BlockDevice` | a block device |

## Mode

[`Mode`] is the editor mode: what a run may change right now. Pick the starting mode for [`ModePolicy::new`], or switch it with [`ModeHandle::set`]. It has no `Default`, so the starting mode is always explicit; choose one on purpose. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`Mode::Ask`]: refuses every mutation; reads flow.
- [`Mode::Plan`]: allows mutations only to paths ending in lowercase `.md`; reads flow.
- [`Mode::Agent`]: allows every operation.

## Op

[`Op`] names the operation being attempted, for a [`Policy`] to decide and a sink to report through [`OpEvent::op`]. `read_string`, `read_range`, and `read_range_numbered` report `Read`, `str_replace` reports `Write`, and `remove` reports `Delete`, so a policy that blocks `Write` also blocks `str_replace`. [`Access`] never produces `Symlink`, `ReadLink`, or `Chmod`. Unlike [`FileType`] and [`PathReason`], it is not `#[non_exhaustive]`, so an exhaustive `match` compiles without a wildcard arm. See [Watch every file operation](#watch-every-file-operation).

| Variant | Meaning |
|---|---|
| `Read` | reads a file's contents, whole or by line range |
| `Write` | writes or replaces a file, including `str_replace` |
| `Append` | appends to a file |
| `Delete` | removes a path |
| `Rename` | renames a path; fires for both the source and the destination |
| `Mkdir` | creates a directory |
| `Copy` | copies a file; fires for both paths |
| `Exists` | checks whether a path exists |
| `Glob` | matches a pattern |
| `List` | lists a directory |
| [`Stat`](Op::Stat) | reads a path's metadata |
| `Symlink` | creates a symbolic link |
| `ReadLink` | reads a symbolic link's target |
| `Chmod` | changes a path's permissions |

## PathReason

[`PathReason`] says which rule a path or glob pattern broke when [`VfsError::InvalidPath`] rejected it before any backend saw it. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm. For an ordinary path, a plain access reports only `Empty` and `Traversal`, normalizing `.`, `..`, and `///`, which a store access refuses; validate plain paths yourself for the strict rules. Any glob pattern can also fail `TooLong`, `Control`, `Backslash`, or `Wildcard`, and a rename into its own descendant `IntoDescendant`.

- [`PathReason::tag`]: returns a snake-case word such as `"empty_segment"` or `"into_descendant"`.
- [`PathReason::from_tag`]: parses a tag back, returning `None` for a word outside the vocabulary.

| Variant | Meaning |
|---|---|
| `Empty` | the path is zero length |
| `Absolute` | a store path starts with `/` |
| `Traversal` | `..` climbs above `/`, or a store path holds a `.` or `..` segment |
| `Control` | the path holds a byte below `0x20` or `0x7f` |
| `EmptySegment` | a store path has an empty segment between separators |
| `Backslash` | a store path, or a glob pattern on any access, contains a backslash |
| `ReservedName` | a store path segment is a Windows reserved name |
| `UnsafeSuffix` | a store path segment ends in `.` or a space |
| `TooLong` | a store path, or a glob pattern on any access, is over 1024 bytes |
| `Wildcard` | a glob pattern has three or more `*`, or a `**` that is not a whole segment |
| `IntoDescendant` | a rename would move a path into its own descendant |

## StoreOp

[`StoreOp`] is one `store.*` call a prompt made, handed to your program inside a store effect, with every path relative to the declared store root. To answer an [`Effect::Store`](crate::effect::Effect::Store), pass its op, with the effect's own access, to [`perform_store_op`]. An absolute path such as `/draft.md` is refused with [`VfsError::InvalidPath`] and [`PathReason::Absolute`]. The error reaches the prompt at its call site, and the author writes the path relative to the store root, as `draft.md`. See [Give a run its files](#give-a-run-its-files).

| Variant | Meaning |
|---|---|
| `Write` | `store.write(path, contents)`; creates or overwrites the file, and `contents` is a `String`, so a store call writes only UTF-8 text |
| `Append` | `store.append(path, contents)`; appends UTF-8 text, creating the file if absent |
| `Read` | `store.read(path, start?, end?)`; no `start` reads the whole file, and a `start` reads a 1-based inclusive line range; an `end` without a `start`, or a zero or negative bound, is refused as an invalid range |
| `ReadNumbered` | `store.read_numbered(path, start?, end?)`; the same read with absolute line numbers, under the same bounds |
| `StrReplace` | `store.str_replace(path, old, new)`; `old` is the anchor and must occur exactly once |
| `Delete` | `store.delete(path)`; documented as idempotent |
| `Glob` | `store.glob(pattern)`; returns the matching paths |
| `Exists` | `store.exists(path)`; returns whether the path is present |

## StoreOutcome

[`StoreOutcome`] is what one performed store op gives back to the prompt. You build one yourself only when you answer a store effect without [`perform_store_op`], such as a test stub that answers every op with `Ok(StoreOutcome::Unit)`. Return the variant that matches the op, because nothing at the type level stops a `Unit` answer to a `Read`.

| Variant | Meaning |
|---|---|
| `Unit` | the op succeeded with no value; every mutating op returns it, and the prompt receives nil |
| `Text` | the file text, possibly bounded to a line range, from `read` or `read_numbered` |
| `Paths` | the matching paths from `glob`, sorted |
| `Bool` | the presence flag from `exists` |

## Verdict

[`Verdict`] is a policy's answer for one operation on one path, returned from [`Policy::check`]. A `Deny` or an `Ask` fails the operation at once with [`VfsError::PermissionDenied`], whose `reason` is the verdict text, and nothing is applied. That text goes back to the model as its tool error, so write `Deny` text that tells the model what to do instead. Both arrive as the same variant, so tell an `Ask` from a `Deny` by its reason text. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`Verdict::Allow`]: the operation may proceed.
- [`Verdict::Deny`]: the operation is refused; the string is the model's recovery hint.
- [`Verdict::Ask`]: the operation needs user approval, and the string is the approval prompt, but the operation still fails at once, as with `Deny`.

## VfsError

[`VfsError`] is the one error every file operation returns; match on the variant and read its public fields, or build one as a literal in a custom backend. Match `Conflict` to tell a race from a backend failure. Through a store view, a missing `absent.txt` reports `absent.txt`, not the full store path. Read `reason` to learn which rule fired; `Display` for `PermissionDenied`, `Unsupported`, and `Conflict` prints only the reason or detail, so log the `path` field yourself. See [Give a run its files](#give-a-run-its-files).

| Variant | Meaning |
|---|---|
| `NotFound` | the path does not exist in the serving backend, or no mount serves it; the path is canonical, so `/mnt/../secret.txt` reports `/secret.txt` |
| `AlreadyExists` | the path exists where creation required absence; `mkdir` on an existing path returns it even when `recursive` is set |
| `NotADirectory` | a directory operation named a non-directory, or a file sits where an ancestor directory should be |
| `IsADirectory` | a file operation such as read, write, or copy named a directory |
| `DirectoryNotEmpty` | a removal without `recursive` named a non-empty directory |
| `NotUtf8` | an operation that needs UTF-8 text, such as `str_replace`, found bytes that are not UTF-8 |
| `InvalidPath` | the path or glob pattern is malformed or escapes the namespace root; `reason` is a [`PathReason`], and a store view reports only the first broken rule, with the path as supplied |
| `InvalidRange` | a line range was rejected; `reason` is a `&'static str`, so a custom backend can supply only a fixed literal |
| `Anchor` | a `str_replace` anchor did not occur exactly once; `count` is 0 when not found and 2 or more when ambiguous, and an empty `anchor` was refused before any search |
| `PermissionDenied` | the operation is not permitted, from a policy, a read-only mount, an operation after the run's scope ended, removing or renaming the namespace root, or an OS permission error; `reason` names the rule that fired |
| `Unsupported` | the serving backend does not implement the operation, a rename or copy spans two mounts, or a store view was asked of a handle with no declared store |
| `Conflict` | the operation clashes with a claim it is not ordered after, either one held by another live scope, since two live scopes are never ordered, or one made by a parallel branch of the same run that this operation does not follow; `detail` names both identities and both claim kinds, and never the store root |
| `Backend` | the serving backend failed for any other reason, including refusing to open a session; the only variant with no path |

## Policy

[`Policy`] decides, for every operation on one handle, whether it may proceed; install one with [`VfsRefBuilder::policy`], or use the shipped [`ModePolicy`] for editor modes. The trait requires only `Send`, but `VfsRefBuilder::policy` takes `impl Policy + Sync + 'static`, so a policy that is not `Sync` cannot be installed. Keep the policy's changing state behind an [`Arc`](std::sync::Arc) shared with your UI; the very next operation sees a change, even mid-run. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`Policy::check`]: runs before the claims check; a copy is checked once per path, both as [`Op::Copy`], so a policy cannot tell source from destination.

## Vfs

[`Vfs`] is one storage backend behind the handle; plug in your own with [`VfsRefBuilder::mount`] or `store`, or wrap one with [`VfsRef::new`]. It sees paths with its mount prefix stripped, so a write to `/a/b/f.txt` on a mount at `/a/b` arrives as `/f.txt`. A mounted backend is acquired only on the first operation that touches its mount, so its `acquire` error arrives from that read or write. Wrap it with `VfsRef::new` to get the error from [`VfsRef::acquire`] up front. See [Give a run its files](#give-a-run-its-files).

- [`Vfs::acquire`]: attributes every operation on the returned session to [`AcquireContext::id`]; a backend that wraps another `Vfs` passes `cx` through unchanged.
- [`Vfs::release`]: called once per touched mount when the access drops, so cancel, panic, and early return cannot skip it; it never ends the scope's claims.
- [`Vfs::read_only`]: defaults to `false`; `true` refuses every mutation before the backend runs, and refuses copy only when this mount is the destination.

## VfsAccess

[`VfsAccess`] is one identity's session with a backend, where every file operation of a custom backend lives; your [`Vfs::acquire`] returns it. Paths arrive validated, canonicalized, and interned, so never re-validate them. Report absence as `NotFound` from `read` and `remove`, and as `Ok(false)` only from `exists`, where a backend failure is `Err`. The handle turns a `NotFound` from `remove` into `Ok(false)` for its callers.

- [`VfsAccess::read_range`]: the default reads the whole file and slices it, clipping past the end; override it to avoid loading whole files.
- [`VfsAccess::glob_kind`]: `dirs_only: false` returns files only, and links are dropped either way; the default fails the whole call if one match's `stat` fails.
- [`VfsAccess::str_replace`]: the default refuses an empty anchor, returns `Anchor` for zero or several matches and `NotUtf8` for non-UTF-8, leaving the file unchanged.
- [`VfsAccess::symlink`]: defaults to `Unsupported`, as do `read_link` and `chmod`, and neither shipped backend overrides them.

## perform_store_op

[`perform_store_op`] performs one store op as one call on the effect's store view and returns the outcome to resume the run with. Answer an [`Effect::Store`](crate::effect::Effect::Store) with [`EffectAnswer::Store`](crate::effect::EffectAnswer::Store)`(perform_store_op(&access, op))`, on a blocking thread rather than your async executor. A write that races another live scope's claim on the same path fails with [`VfsError::Conflict`] and never lands. Pass the error back as the answer unchanged; the run raises it at the prompt author's call site. See [Give a run its files](#give-a-run-its-files).

## OpSink

[`OpSink`] is the shared closure that sees each operation that passed policy and claims, just before the backend runs, with its kind, canonical path, and caller's [`Origin`], but not its outcome. Install a plain closure with [`VfsRefBuilder::on_op`], which builds it. A slow sink slows every operation it fires for, store or not, because it runs inline on the operation's thread. Keep it cheap, for example by sending events to a channel, and clone what you keep. See [Watch every file operation](#watch-every-file-operation).
