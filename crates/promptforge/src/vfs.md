Give a run its files: a shared store, real directories beside it, and rules for what it may change.

You need this when your prompts read or write files, or several runs share a folder.

# Where this fits

[Run a prompt](crate#run-a-prompt) showed you how to step a run and answer each [effect](crate) it asks for, using the greeter prompt from [Before you start](crate#before-you-start). A prompt's file work reaches your program as one of those effects, an [`Effect::Vfs`](crate::effect::Effect::Vfs). This page shows what stands behind your answer: where a run's files live, what it may change, and how you see each file operation.

# Give a run its files

Your prompt reads and writes files, and your program wants a real folder beside the prompt's scratch store. A prompt reaches files through Lua calls in its code blocks, such as `store.write('note.md', 'hello')` and `store.read('note.md')`. Each `store.*` call reaches your program as one file operation on the *store*, the set of virtual files that every [section](crate) of a run shares. To write prompts like these, see the [PromptForge user guide](https://cppalliance.github.io/promptforge/).

Your program decides where those files live by building one *handle*, a [`VfsRef`], and sharing it with every run. The handle holds *mounts*, and each mount attaches a *backend* to a path prefix. A backend is the thing that holds files, such as a folder on disk or a map in memory. Building the handle feels like filling in a mount table: the longest matching prefix serves each path. Unlike an operating system, the table is fixed once you build the handle.

Before any backend sees an operation, the handle checks it, and the first thing it does is rewrite the path into one standard spelling, with repeated slashes collapsed and `.` and `..` resolved (see [`VfsPath`]).

The example mounts a temporary folder at `/` and a memory store at `/store`, prepares a run context for each of two greeter runs, and has your program write `shared.txt` through an access taken from each context's handle.

````
use promptforge::vfs::{MemoryBackend, Origin, RealBackend, VfsError, VfsRef};
# use promptforge::timestamp::Timestamp;
# use promptforge::{Environment, Prompt, RunContext};
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
# let prompt = parsed?;
# let environment = Environment::new();
# let context = |name: &str| RunContext::new(name, 7, Timestamp::UNIX_EPOCH);

// 1. Make a real folder, mount it at `/`, and declare a memory store at `/store`.
let dir = std::env::temp_dir().join(format!("vfs-tour-{}", std::process::id()));
std::fs::create_dir_all(&dir)?;
let vfs = VfsRef::builder()
    .mount("/", RealBackend::rooted(&dir)?)
    .store("/store", MemoryBackend::new())
    .build();

// 2. Prepare two greeter runs over clones of the one handle.
let (ctx_a, _requirements) = environment.prepare(&prompt, context("run-a").vfs(vfs.clone()));
let (ctx_b, _requirements) = environment.prepare(&prompt, context("run-b").vfs(vfs));

// 3. The first run's access writes `shared.txt` in the folder, and the write lands.
let access_a = ctx_a.vfs_handle().acquire(Origin::new("run-a"))?;
let first = access_a.write("/shared.txt", b"from a");
assert!(first.is_ok());

// 4. The second run's access writes the same file while the first still holds it, and it conflicts.
let access_b = ctx_b.vfs_handle().acquire(Origin::new("run-b"))?;
let second = access_b.write("/shared.txt", b"from b");
assert!(matches!(second, Err(VfsError::Conflict { .. })));
let on_disk = std::fs::read(dir.join("shared.txt"))?;
std::fs::remove_dir_all(&dir)?;
assert_eq!(on_disk, b"from a");
# Ok::<(), Box<dyn std::error::Error>>(())
````

The handle from step 1 holds this table of mounts, and every path goes to the mount whose prefix matches it longest:

````text
                ┌──────────────────────────────────────┐
                │  VfsRef: one handle, cloned per run  │
                └──────────┬────────────────┬──────────┘
   /shared.txt             │                │   /store/note.md
                           v                v
          ┌──────────────────────┐   ┌──────────────────────────┐
          │ mount "/"            │   │ mount "/store"           │
          │ RealBackend::rooted  │   │ MemoryBackend, the store │
          │ sees /shared.txt     │   │ sees /note.md            │
          └──────────────────────┘   └──────────────────────────┘
````

A prompt's `store.write('note.md', ...)` joins onto the store root, so it addresses `/store/note.md`. The mount at `/store` strips its own prefix, so the memory backend sees `/note.md`, which is why the diagram shows `/note.md` inside the `/store` box.

1. The first step builds the handle. [`VfsRef::builder`] starts it, [`VfsRefBuilder::mount`] attaches a backend at an absolute prefix, and [`VfsRefBuilder::build`] finishes it. A prefix that is not absolute, or is already taken, panics, so you choose every place your runs can reach before the first run starts.
   - [`RealBackend::rooted`] serves the temporary folder at `/`. It fails with [`VfsError::NotFound`] when the folder is missing and [`VfsError::NotADirectory`] when the path is not a directory, so a bad path shows up when you build the handle, not in the middle of a run.
   - [`VfsRefBuilder::store`] mounts a [`MemoryBackend`] at `/store` and also declares it the store, the place a prompt's `store.*` calls land.
   - When two mounts cover a path, the longest prefix wins, so a memory store mounted at `/store` hides any real file at the same path. Pick a store prefix that your real folder does not use.
   - A root that is already taken panics. A second call to `store` at a different root moves the store label to the new backend and leaves the first backend as a plain mount. The first backend's files stay reachable through its own mount, but a prompt's `store.*` calls no longer reach them.
2. The second step prepares a run context for each of two greeter runs over the one handle. `context` builds the greeter's [`RunContext`](crate::RunContext), as in [Run a prompt](crate#run-a-prompt), and [`RunContext::vfs`](crate::RunContext::vfs) sets the handle the run uses, exactly as you pass it: run a gets `vfs.clone()` and run b gets `vfs`. [`Environment::prepare`](crate::Environment::prepare) never replaces the handle, so both runs share the one set of mounts.
3. The third step writes `shared.txt` through an access taken from run a's context. [`RunContext::vfs_handle`](crate::RunContext::vfs_handle) returns the handle the run holds. [`VfsRef::acquire`] takes an [`Origin`], a label for who is asking that never allows or refuses anything, and gives run a its [`Access`], the object that reads and changes files. The write lands in the real folder through the mount at `/`.
   - Each `acquire` starts its own *scope*, which lasts as long as the access it returns.
   - The handle has no rule that puts work done in one scope before work done in another, so it cannot assume the second access has seen the first access's write.
   - The write now leaves a *claim* on `/shared.txt`, the mark an access leaves on a path when it reads or writes it. A claim counts only while its access is *live*, meaning the access has not been dropped and the run that owned it has not ended; once that run ends, its claims are ignored even if the access is still held. Of two live accesses, the one that touches the path second gets `Conflict` when at least one of the two is a write, and it does not matter which scope started first. Two reads never conflict.
4. The fourth step writes the same file through an access taken from run b's context, while run a's access is still alive. The write fails with [`VfsError::Conflict`], and it never reaches the backend, so the file on disk holds only `from a`. Two runs that share a real folder cannot silently overwrite each other.

A claim lasts only as long as the access that made it. A later run can use the same path once the first access is dropped or its run has ended, so run two runs on one file one after the other, or give them different paths.

A prompt's `store.*` calls reach only the backend that you declare as the store, so a prompt never touches the real folder in this example. Your own program reaches that folder through the handle, as steps 3 and 4 do. A prompt can write real files only if you mount a [`RealBackend`] as the store itself with `store`.

The builder is not the only way to make a handle. [`VfsRef::new`] builds a handle over a single backend that serves every path, and [`VfsRef::with_policy`] does the same and installs a policy, the rule the next tour explains. [`VfsRef::acquire_store`] is the sibling of `acquire` that opens the store view, an `Access` rooted at the declared store. Neither `new` nor `with_policy` declares a store, and neither does a builder that never calls `store`, so `acquire_store` fails on them with [`VfsError::Unsupported`]. [`VfsRef::default()`] is an empty in-memory store at `/` that declares one, the quickest handle for an offline test.

You might expect a second run that writes the same real file to replace the first run's content, the way two processes would. Instead, the second write fails with `VfsError::Conflict`, and the first run's content stays.

One handle, built once, decides where every path goes and which run may touch it. Next, [Keep a run from changing files](#keep-a-run-from-changing-files) adds a rule about what a run may change.

# Keep a run from changing files

You want a run to read your files, but to change them only when the current mode allows it. A *policy* is the rule the handle asks before every file operation, and it answers whether the operation may go ahead. A handle built without a policy never refuses on policy grounds.

[`ModePolicy`] is the ready-made policy that applies a *mode*, the setting that decides what the model may change right now. [`VfsRefBuilder::policy`] installs it, and [`ModePolicy::handle`] returns the [`ModeHandle`] you keep to change the mode later. Leave the policy alone unless you need to limit a run, and keep the `ModeHandle` if you will change the mode later.

The mode behaves like an `Arc<Mutex<_>>` flag that every file operation reads at the moment it runs, and the `ModeHandle` you keep is a second reference to it. Unlike a setting you pass in at the start, you can change it in the middle of a run, and the next operation sees the new value.

A change is write, append, delete, rename, mkdir, copy, symlink, chmod, or `str_replace`; reading, listing, `stat`, and `exists` are not changes. [`Mode::Agent`] allows every operation, [`Mode::Ask`] refuses every change, and [`Mode::Plan`] allows a change only when every path the change touches ends in `.md`. Reads flow in all three, which means no mode ever refuses them.

The example below gates the greeter's store handle with a mode policy that starts in `Agent`. It lets the note write and read succeed, switches to `Ask`, and shows the refused write.

````
use promptforge::vfs::{
    perform_vfs_op, MemoryBackend, Mode, ModePolicy, Origin, VfsOp, VfsOutcome, VfsError, VfsRef,
};
# use std::sync::Arc;
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
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
# let context = |name: &str| RunContext::new(name, 7, Timestamp::UNIX_EPOCH);

// 1. Keep the mode handle, then install a policy that starts in Agent on a handle with a store.
let policy = ModePolicy::new(Mode::Agent);
let mode = policy.handle();
let vfs = VfsRef::builder().store("/", MemoryBackend::new()).policy(policy).build();

// 2. Give the greeter's run a clone of the gated handle, and Agent mode lets its note calls succeed.
let ctx = context("greeter").vfs(vfs.clone());
# let mut run = Run::new(Arc::new(parsed?), "", ctx);
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 let answer = match effect {
#                     Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#                     _ => EffectAnswer::Dropped,
#                 };
#                 run.resume(id, answer);
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

// 3. Switch to Ask through the mode handle, and answer a second write of the note.
mode.set(Mode::Ask);
let store = vfs.acquire_store(Origin::new("host"))?;
let second = VfsOp::Write { path: "note.md".to_owned(), contents: "changed".to_owned() };
let refused = perform_vfs_op(&store, second);
assert!(matches!(&refused, Err(VfsError::PermissionDenied { reason, .. }) if reason.contains("Ask")));

// 4. Read the note back through the same store view, and it still holds the first write.
let read = VfsOp::Read { path: "note.md".to_owned(), start: None, end: None };
assert_eq!(perform_vfs_op(&store, read)?, VfsOutcome::Text("hello".to_owned()));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The first step keeps the `ModeHandle` and gates the handle. [`ModePolicy::new`] makes a policy that starts in `Agent`, and the example takes the `ModeHandle` from it before the builder moves the policy in. [`VfsRefBuilder::store`] mounts a [`MemoryBackend`] at `/` and declares it the store. The [`VfsRef`] is gated from the start, and the `ModeHandle` is your only way to switch the mode.
2. The second step gives the greeter's run a clone of the gated handle with [`RunContext::vfs`](crate::RunContext::vfs), and `Agent` mode lets its note calls succeed. The page hides the lines that build the run and drive it, as in [Run a prompt](crate#run-a-prompt). The loop answers each store effect with [`perform_vfs_op`], gives up on any other effect, and stops at `Done`. The assertion checks that the run returns `hello`, so both the note write and the note read succeed.
3. The third step switches the mode to `Ask` through the `ModeHandle`, and then answers a second write of the note. A [`VfsOp`] carries one `store.*` call from a prompt to your program, a [`VfsOutcome`] carries the result back, and `perform_vfs_op` runs the first against a store view and returns the second. [`ModeHandle::set`] changes the mode, [`VfsRef::acquire_store`] opens a store view for an [`Origin`] labeled `host`, and `perform_vfs_op` runs the [`VfsOp::Write`] against it. The write fails with [`VfsError::PermissionDenied`], and the policy's reason names the `Ask` mode. The example flips the mode after the run has finished, so it shows the next operation seeing `Ask`; it does not interrupt a run. The mode is read at every operation, so a flip during a run applies to that run's next file operation in the same way.
4. The fourth step reads the note back through the same store view with a [`VfsOp::Read`], and it still holds the first write. The refused write changed nothing, and reads flow even in `Ask` mode.

A `str_replace` counts as a write.

A prompt's `store.*` calls can only write, append, read, read_numbered, str_replace, delete, glob, and exists, while `rename`, `mkdir`, and `copy` are [`Access`] methods that only your program calls.

`symlink` and `chmod` count as changes, but no `Access` method issues them, so they matter only to a policy you write yourself. Pick the mode by what you want a run to be able to change.

The `.md` test in `Plan` is case-sensitive, so `NOTES.MD` is refused. A copy touches two paths and `Plan` checks both, so the source must end in `.md` as well as the destination. A copy that your program makes with [`Access::copy`], from a source that does not end in `.md` into a note, fails under `Plan`.

A refused change, including one from `Ask`, comes back as `PermissionDenied` with the policy's reason, and the file is untouched. You can show the user the reason, and nothing is half written.

A real folder mounted with [`RealBackend::with_read_only`]`(true)` refuses every change with `PermissionDenied` in any mode, because the flag belongs to the mount and not to the policy. Use it for a folder that must never change.

You might expect `Ask` mode to pause a write until the user approves it. Instead, it refuses the write at once with `PermissionDenied`, and any approval dialog is your program's job.

The mode is checked on every operation, so you can change it whenever you like. Next, [Watch every file operation](#watch-every-file-operation) shows how to log what a run does.

# Watch every file operation

You want a log of the file operations a run makes, and who asked for each one.

A *watcher* is a closure the handle calls for each file operation that the policy and the claims let through, just before the backend acts. An operation they let through is *admitted*, and a refused operation is never admitted. Installing one feels like registering a logging closure: it is a plain `Fn` that gets called for each event. Unlike a policy, which can refuse an operation, a watcher only looks - it returns nothing and cannot stop or change an operation.

[`VfsRefBuilder::on_op`] installs the closure. It receives an [`OpEvent`] for each operation, and the event gives you the kind of operation with `op`, the canonical path with `path`, and who asked with `origin`. Together they log each operation the rules let start.

An [`Origin`] carries a label, a file, and a line. The label is the most specific name the caller has. An origin only labels an event, and it never allows or refuses anything. [`Origin::new`] records the file and line of its caller because it carries `#[track_caller]`, a Rust attribute that makes [`Location::caller`](std::panic::Location::caller) report where a function was called from, instead of where it is written. A helper that builds origins must carry it too, or every origin names the helper, and a log full of one line points at your helper, not at the real caller.

The example installs a watcher that prints each operation and its origin, runs the greeter, and checks what the watcher saw for the note.

````
use std::sync::{Arc, Mutex};

use promptforge::vfs::{MemoryBackend, Op, OpEvent, VfsRef};
# use promptforge::effect::{Effect, EffectAnswer};
# use promptforge::timestamp::Timestamp;
# use promptforge::vfs::perform_vfs_op;
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
# let context = |name: &str| RunContext::new(name, 7, Timestamp::UNIX_EPOCH);

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

// 2. Run the greeter over the watched handle, and it still returns its note.
let ctx = context("greeter").vfs(vfs);
# let mut run = Run::new(Arc::new(parsed?), "", ctx);
# let result = loop {
#     match run.step() {
#         Step::Pending { effects, .. } => {
#             for (id, _provenance, effect) in effects {
#                 let answer = match effect {
#                     Effect::Vfs { access, op } => EffectAnswer::Vfs(perform_vfs_op(&access, op)),
#                     _ => EffectAnswer::Dropped,
#                 };
#                 run.resume(id, answer);
#             }
#         }
#         Step::Done { result, .. } => break result,
#     }
# };
assert!(matches!(result, RunResult::Ok(text) if text == "hello"));

// 3. The note saw a write and then a read, both asked for by the same origin.
let entries = log.lock().map_err(|_| "the watcher panicked")?;
let note: Vec<_> = entries.iter().filter(|(_, path, _)| path == "/note.md").collect();
assert!(matches!(note.as_slice(), [(Op::Write, _, first), (Op::Read, _, second)] if first == second));
# Ok::<(), Box<dyn std::error::Error>>(())
````

1. The first step installs the watcher. The closure prints each operation, its path, and its origin label, and copies those three values into a shared log. It must copy them, because an `OpEvent` only borrows its values and is not `Clone`, so a stored event would not outlive the call.
2. The second step gives the watched handle to the greeter's run. The page hides the lines that build the run and answer each store effect with [`perform_vfs_op`], as in [Keep a run from changing files](#keep-a-run-from-changing-files). The run still returns `hello`, so you can see the watcher only looks.
3. The third step filters the log to the note's path and matches it. The run's `store.write` fired one [`Op::Write`] and its `store.read` one [`Op::Read`], in that order. The run supplies the origin itself, so you never pass one for the run's calls. Its label is the section or pass that made the call, and the prompt's title stands in for the file name, so both note events carry the same label. A watcher can print `event.origin().label`, `file`, and `line` to see them.

The watcher fires after the policy and the claims admit an operation, and before the backend runs. A refused operation never reaches it, and an event does not mean the operation succeeded, so read the log as what was allowed to start, not as what finished.

Events do not match your calls one for one. `rename` and `copy` fire one event per path, and `str_replace` reports as `Write`, so count events by path and expect an edit to look like a write.

Keep the closure cheap. Store operations fire it inline with the operation, so a slow watcher slows the file operation itself. Keep slow work, such as disk or network writes, out of the closure.

You might expect the watcher to see a write that the mode refused, as a failed attempt. Instead, it never hears about it, because it fires only after the rules allow the operation.

A watcher sees what the rules allowed, not what happened. The [Reference](#reference) below has an entry for each item on this page.

# Reference

## Access

[`Access`] is what a run holds to read and change files. You get one from [`VfsRef::acquire`] or [`VfsRef::acquire_store`]. It fails with [`VfsError::PermissionDenied`] when the policy refuses and [`VfsError::Conflict`] when another live access holds the path. A `rename` or `copy` across two mounts is [`VfsError::Unsupported`]. Read the reason in the error, keep related work in one `Access`, and see [Give a run its files](#give-a-run-its-files).

- [`Access::read_range`]: reads lines `start..=end`, 1-based; an omitted `end` means the last line, and a `start` below 1 is [`VfsError::InvalidRange`].
- [`Access::str_replace`]: replaces the one occurrence of `old`, and a policy or watcher sees it as a `Write`; an `old` that is empty, missing, or repeated is [`VfsError::Anchor`].
- [`Access::remove`]: returns `Ok(true)` for a removal and `Ok(false)` for a missing path; removing the namespace root is `PermissionDenied`.
- [`Access::exists`]: returns `Ok(false)` only for a confirmed absence, so it is true for directories and the root.
- [`Access::glob`]: returns files only, or directories only when the pattern ends in `/`, and a relative pattern gives relative paths.

## AcquireContext

[`AcquireContext`] is what [`Vfs::acquire`] receives when a backend is asked for its access object. You meet it only when you write a custom backend that wraps another backend: pass it on unchanged to the wrapped backend's `acquire`, so the wrapped handle joins the caller's scope. It has no public constructor, so only a handle builds one.

- [`AcquireContext::id`]: returns the [`ExecId`] that every operation on the returned access object is attributed to.

## AllowAll

[`AllowAll`] is the policy that permits every operation. You get it by leaving the policy alone: [`VfsRef::new`] installs it, and a builder that never calls [`VfsRefBuilder::policy`] builds with it. Install a gate such as [`ModePolicy`] when runs must be limited.

## Entry

[`Entry`] is one row of the vector that [`Access::list`] returns, sorted by name. Read its [`Stat`] for the kind and size, and join the name onto the directory yourself when you need a full path. The real backend lists a symlink as the link, and alters a name that is not UTF-8, so a listed name may not match the name on disk. `Entry` is `#[non_exhaustive]`, so you cannot build one with a struct literal.

- [`Entry::name`]: the entry's name within its directory, not a full path.
- [`Entry::description`]: an optional annotation; both bundled backends leave it `None`.

## ExecId

[`ExecId`] is the identity that every file operation is attributed to. You see it in [`Vfs::release`] or through [`AcquireContext::id`] when you write a custom backend. Copy it and use it as a map key for any per-run state your backend keeps. It is unique within the process, and only a handle creates one, a fresh one for each [`VfsRef::acquire`]. A store view, the [`Access`] from [`VfsRef::acquire_store`], reuses the identity of the access it came from.

## RealBackend

[`RealBackend`] serves a real directory on disk behind the virtual namespace. Build it with [`RealBackend::rooted`]; only your own program reaches a `RealBackend` added with `mount`, because a prompt's `store.*` calls reach only the declared store, so mount it with `store` when prompts must read or write actual files. `rooted` fails with [`VfsError::NotFound`] when the directory is absent and [`VfsError::NotADirectory`] when it is not a directory, so create the directory first. On a read-only backend every change is [`VfsError::PermissionDenied`], and a failed `write` or `copy` leaves the source and destination files as they were. [Give a run its files](#give-a-run-its-files) mounts one.

- [`RealBackend::identity`]: makes virtual paths the real paths, so on Windows virtual `/C:/a/b` is real `C:\a\b`, with no containment.
- `RealBackend::rooted`: makes the directory the virtual root, and checks that every resolved path stays inside it.
- [`RealBackend::with_read_only`]: sets whether the backend refuses every change; the flag belongs to the mount, not to the policy.

## MemoryBackend

[`MemoryBackend`] keeps files in memory, with nothing on disk, so use it for a scratch store that goes with the run, or for tests. Clones share one storage, so every run over one backend sees the same files. A write creates every missing parent directory, and a rename onto an existing file replaces it silently. A missing path is [`VfsError::NotFound`] and `mkdir` on an existing path is [`VfsError::AlreadyExists`]; check with `exists` first. See [Give a run its files](#give-a-run-its-files).

## ModeHandle

[`ModeHandle`] changes the [`Mode`] while a run is going, for example when the user switches between `Ask`, `Plan`, and `Agent`. Get one from [`ModePolicy::handle`] and call [`ModeHandle::set`] whenever the user switches. The next operation sees the new mode, and clones share one mode cell, so setting it through any clone changes it for all. [Keep a run from changing files](#keep-a-run-from-changing-files) shows a switch.

## ModePolicy

[`ModePolicy`] gates every change by the mode and never gates a read. Use it when a run may read freely but change files only when the mode allows. In `Ask` mode every change is [`VfsError::PermissionDenied`], and in `Plan` mode so is a change to a path not ending in lowercase `.md`, including a copy from a non-markdown source. Switch to `Agent` with the [`ModeHandle`]. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`ModePolicy::new`]: starts in the mode you give; a change is write, append, delete, rename, mkdir, copy, symlink, or chmod, and every other operation flows.
- [`ModePolicy::handle`]: returns the `ModeHandle` that changes this policy's mode later.

## OpEvent

[`OpEvent`] describes one admitted file operation to your watcher. You receive one in the closure you pass to [`VfsRefBuilder::on_op`], and you copy out what you need, because it only borrows its values and is not `Clone`. It fires after the policy and claims pass and before the backend runs, so a denied operation never fires it and no outcome comes back. `rename` and `copy` fire one event per path. [Watch every file operation](#watch-every-file-operation) shows a watcher.

- [`OpEvent::path`]: returns the canonical path the operation acts on.
- [`OpEvent::origin`]: returns the [`Origin`] of the access that admitted the operation.

## Origin

[`Origin`] labels who asked for a file operation, and where in the code. Pass one to [`VfsRef::acquire`] or [`VfsRef::acquire_store`] so your watcher knows the source. It never allows or refuses an operation. [`Origin::new`] stamps the file and line of the code that called it, so a helper that builds origins reports its own location unless the helper is `#[track_caller]` too. [Watch every file operation](#watch-every-file-operation) shows origins in a log.

- `Origin::new`: stamps the Rust call site through [`Location::caller`](std::panic::Location::caller).
- [`Origin::at`]: sets an explicit label, file, and line, such as a prompt's name and line.
- [`Origin::label`]: the most specific name the caller has, such as a section name, a tool id, or a fixture name.
- [`Origin::file`]: the Rust source file for `new`, and the prompt's name for `at`.
- [`Origin::line`]: the 1-based line within `file`.

## Stat

[`Stat`] is the metadata for one path. You get it from [`Access::stat`] or from the `stat` of an [`Entry`]. A backend that does not track a field says `None` instead of inventing a value, so check each optional field before you use it. [`MemoryBackend`] leaves mode, modified, and created as `None`. `Stat` is `#[non_exhaustive]`, so you cannot build one with a struct literal.

- [`Stat::file_type`]: the node's [`FileType`].
- [`Stat::size`]: the size in bytes.
- [`Stat::mode`]: POSIX mode bits, when the backend tracks them.
- [`Stat::modified`] and [`Stat::created`]: the times of the last change and of creation, when tracked.

## VfsPath

[`VfsPath`] is the canonical path that [`Policy::check`] receives and [`OpEvent::path`] returns, so compare paths by this form. Backslashes count as separators, repeated separators collapse, `.` vanishes, `..` pops one segment, a trailing slash drops, and case is kept and matters. An empty path is [`VfsError::InvalidPath`] with [`PathReason::Empty`], and `..` past the root is `InvalidPath` with [`PathReason::Traversal`]. Log it with `Display`, because `Debug` prints `VfsPath("...")`.

## VfsPathBuf

[`VfsPathBuf`] is the owned form of a [`VfsPath`], for places that outlive a borrow, such as the target a backend's `read_link` returns. Get one with [`VfsPath::to_buf`] or `From<VfsPath>`; there is no public constructor from a string, so an owned path is always canonical. It is ordered, so it can key a [`BTreeMap`](std::collections::BTreeMap).

## VfsRef

[`VfsRef`] is the cloneable handle your program builds once and shares, and from which every run gets an [`Access`]. Build it with [`VfsRef::builder`]. [`VfsRef::default()`] is a memory store at `/` declared as the store, the quickest handle for an offline test. [`VfsRef::new`] and [`VfsRef::with_policy`] declare no store, so [`VfsRef::acquire_store`] fails on them with [`VfsError::Unsupported`]. [Give a run its files](#give-a-run-its-files) builds one.

- `VfsRef::new`: builds a handle over one backend with [`AllowAll`], so its policy refuses nothing.
- `VfsRef::with_policy`: takes the policy by value, so state you change later must live inside it, as the mode cell does in [`ModePolicy`].
- [`VfsRef::overlay`]: returns a handle with a backend at a prefix that only the overlay sees; it panics on a relative prefix or the root.
- [`VfsRef::acquire`]: vends an `Access` for a new scope, and while two scopes live, a path one wrote gives the other [`VfsError::Conflict`].
- `VfsRef::acquire_store`: vends an `Access` rooted at the declared store, whose operations reach the store's mount alone.

## VfsRefBuilder

[`VfsRefBuilder`] collects mounts, a store, a policy, and a watcher before it builds a [`VfsRef`]. Use it to combine a real directory with a store, add a gate, or watch operations. A path that no mount serves is [`VfsError::NotFound`]. Add every mount, policy, and watcher before [`VfsRefBuilder::build`], because the table is fixed after it. See [Give a run its files](#give-a-run-its-files).

- [`VfsRefBuilder::mount`]: mounts a backend at a prefix, and a longer prefix shadows a shorter one; it panics on a relative or taken prefix.
- [`VfsRefBuilder::store`]: mounts a backend at a root and declares it the store; a second call at a different root moves the store label to the new backend.
- [`VfsRefBuilder::on_op`]: installs the watcher, which fires on every admitted operation, must be cheap, and cannot change the outcome.

## FileType

[`FileType`] names the kind of a node, as held in [`Stat::file_type`]. Match on it to read what a [`Stat`] says a path is. [`Access::glob`] filters by it for you: files only by default, and directories only when the pattern ends in `/`. The enum is `#[non_exhaustive]`, so match [`FileType::File`] and [`FileType::Directory`] and add a wildcard arm for the rest. [`MemoryBackend`] reports only those two.

| Variant | Meaning |
|---|---|
| `File` | A regular file. |
| `Directory` | A directory. |
| `Symlink` | A symbolic link. |
| `Fifo` | A named pipe. |
| `Socket` | A socket. |
| `CharDevice` | A character device. |
| `BlockDevice` | A block device. |

## Mode

[`Mode`] says what the model may change right now. Pass one to [`ModePolicy::new`] or [`ModeHandle::set`]. A change the mode forbids is [`VfsError::PermissionDenied`], so pick `Agent` to allow every operation, or aim writes at `.md` paths while the mode is `Plan`. [Keep a run from changing files](#keep-a-run-from-changing-files) teaches the three modes.

- `Ask`: refuses every change, and reads flow. The refusal is immediate, and any approval dialog is your program's job.
- `Plan`: allows a change only when every path it touches ends in `.md`, and reads flow.
- `Agent`: allows every operation.

## Op

[`Op`] names the kind of operation that a [`Policy`] matches on and that [`OpEvent::op`] reports to a watcher. Expect [`Op::Write`] for `str_replace`, and expect [`Op::Rename`] and [`Op::Copy`] once per path, because each path is checked on its own. No [`Access`] method issues `Symlink`, `ReadLink`, or `Chmod`, yet [`ModePolicy`] counts `Symlink` and `Chmod` as changes, so decide on them in your own policy too. See [Watch every file operation](#watch-every-file-operation).

| Variant | Meaning |
|---|---|
| `Read` | Reads a file's bytes, issued by `read`, `read_string`, and `read_range`. |
| `Write` | Creates or overwrites a file, issued by `write` and `str_replace`. |
| `Append` | Appends to a file. |
| `Delete` | Removes a file, link, or directory, issued by `remove`. |
| `Rename` | Renames or moves a path. |
| `Mkdir` | Creates a directory. |
| `Copy` | Copies a file. |
| `Exists` | Tests for existence. |
| `Glob` | Matches paths against a pattern. |
| `List` | Lists a directory. |
| [`Stat`](Op::Stat) | Reads metadata. |
| `Symlink` | Creates a symbolic link. |
| `ReadLink` | Reads a symbolic link's target. |
| `Chmod` | Changes mode bits. |

## PathReason

[`PathReason`] names the rule that a path or glob pattern broke, held by every [`VfsError::InvalidPath`]. Use it to tell the reader what to fix, and keep a wildcard arm when you match, because the enum is `#[non_exhaustive]`. Most reasons come only from a store view, which reports the first rule broken, or from a glob pattern; a plain [`Access`] rejects only an empty path and `..` past the root.

- [`PathReason::tag`]: returns the short tag, such as `empty_segment`, that a store error's `rule` field holds.
- [`PathReason::from_tag`]: parses a tag back, and returns `None` for a tag outside the vocabulary.

| Variant | Meaning |
|---|---|
| `Empty` | An empty path. |
| `Absolute` | A path that began with `/`, so it addressed outside the run's namespace. |
| `Traversal` | A `.` or `..` segment; a store view refuses both. |
| `Control` | A control character, meaning a byte below `0x20` or the byte `0x7f`. |
| `EmptySegment` | A `//` run or a trailing `/`. |
| `Backslash` | A backslash, which is ambiguous across backends; a store view and a glob pattern refuse it. |
| `ReservedName` | A platform-reserved device name in some segment: `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, or `LPT1` to `LPT9`, matched without case on the base name before the first `.`. |
| `UnsafeSuffix` | A segment ending in `.` or a space, which some backends strip. |
| `TooLong` | A store path over 1024 bytes. |
| `Wildcard` | Invalid glob grammar, such as a run of three `*`; only glob reports it. |
| `IntoDescendant` | A rename into the source's own subtree; only rename reports it. |

## VfsOp

[`VfsOp`] carries one `store.*` call from a prompt's Lua to your program. Use it to answer a store effect yourself, or to log what a prompt asked. A [`VfsOp::StrReplace`] whose `old` is empty, missing, or repeated fails with [`VfsError::Anchor`]; make `old` occur exactly once. Match it with a wildcard arm, because it is `#[non_exhaustive]`. It is plain data with [serde](https://docs.rs/serde) support, so a recorded operation replays unchanged.

| Variant | Meaning |
|---|---|
| `Write { path, contents }` | `store.write(path, contents)`; `path` is the logical path the prompt gave. |
| `Append { path, contents }` | `store.append(path, contents)`; `contents` is the text added to the file. |
| `Read { path, start, end }` | `store.read(path, start?, end?)`. With neither `start` nor `end` it reads the whole file, a `start` slices a 1-based inclusive line range, and an `end` without a `start` is refused as [`VfsError::InvalidRange`], as is a zero or negative bound. |
| `ReadNumbered { path, start, end }` | `store.read_numbered(path, start?, end?)`; the same read, with absolute line numbers under the same bounds. |
| `StrReplace { path, old, new }` | `store.str_replace(path, old, new)`; `old` is the anchor text and must occur exactly once, and `new` replaces it. |
| `Delete { path }` | `store.delete(path)`. Deleting a missing path is not an error and answers `Unit`, but delete never removes recursively, so a non-empty directory fails with [`VfsError::DirectoryNotEmpty`]. |
| `Glob { pattern }` | `store.glob(pattern)`. A pattern follows the same strict path rules as a path: `/etc/*` is `InvalidPath` with `Absolute`, and `../*.txt` and `a/./b` are `InvalidPath` with `Traversal`. `**` as a whole segment is accepted, and results come back in the logical form, such as `a.txt`, never under the store root. |
| `Exists { path }` | `store.exists(path)`. |

## VfsOutcome

[`VfsOutcome`] carries the result of one store operation back to the prompt. You read it from [`perform_vfs_op`], or build one to answer a [`VfsOp`] yourself. Unlike `VfsOp` and [`VfsError`], it is not `#[non_exhaustive]`, so you can match all four cases without a wildcard, and a new variant would break your build. Its serde form is the run log's success payload for a store answer, so the variant names are part of the log shape.

| Variant | Meaning |
|---|---|
| `Unit` | The operation succeeded with no return value, so the prompt gets nil; changes produce it. |
| `Text(String)` | `read` and `read_numbered`: the file text, possibly bounded. |
| `Paths(Vec<String>)` | `glob`: the matching paths, sorted. |
| `Bool(bool)` | `exists`: the presence flag. |

## Verdict

[`Verdict`] tells a handle whether one operation on one path may proceed, must be refused, or needs the user. A [`Policy`] returns it. Both refusals fail with [`VfsError::PermissionDenied`], and `reason` holds the verdict's text, so read it to see which rule fired. It is not `Copy` and not `#[non_exhaustive]`, so match all three cases with no wildcard arm. [Keep a run from changing files](#keep-a-run-from-changing-files) shows `Ask` at work.

- `Allow`: the operation may proceed.
- `Deny(String)`: the operation is refused, and the string reaches the model as the tool error, so write it as a way to recover.
- `Ask(String)`: the operation needs user approval, but a handle refuses it at once with the string; change the policy, such as the mode, and retry.

## VfsError

[`VfsError`] is the one error every virtual filesystem operation returns. Match on it to learn why a file operation failed. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm; it has no helper methods, so read the variant's public fields. `Display` leaves `path` out for `PermissionDenied`, `Unsupported`, and `Conflict`, so add the path from the field when you log. A custom backend builds variants as struct literals.

| Variant | Meaning |
|---|---|
| `NotFound { path }` | The path does not exist in the serving backend; `path` is the canonical path that did not resolve. When no mount serves a path, `path` names that unrouted path. |
| `AlreadyExists { path }` | Creation needed the path to be absent. `mkdir` raises it for any existing path, even with `recursive`, in both bundled backends. |
| `NotADirectory { path }` | A directory operation named a non-directory. When a file sits above the target, a memory write names that ancestor file in `path`. |
| `IsADirectory { path }` | A file operation named a directory, such as reading one or writing at its path in the memory backend. A rejected write leaves the tree untouched. |
| `DirectoryNotEmpty { path }` | A removal without `recursive` named a non-empty directory, and nothing changes. A directory rename onto a non-empty directory raises it too. |
| `NotUtf8 { path }` | Text that is not UTF-8 appeared where UTF-8 was required; the default `str_replace` raises it for a binary file. |
| `InvalidPath { path, reason }` | The path or glob pattern is malformed or escapes the namespace root. `path` is the rejected text as supplied, and `reason` is a [`PathReason`]. |
| `InvalidRange { path, reason }` | A line range was rejected. `reason` is a short text naming the bad bound, and its wording differs between [`Access::read_range`] and a store read, so do not match on it. |
| `Anchor { path, anchor, count }` | A `str_replace` anchor did not occur exactly once. `count` is 0 for no match and 2 or more when the edit is ambiguous, an empty `anchor` is refused before any search, and the file is unchanged. |
| `PermissionDenied { path, reason }` | A read-only mount or a policy denial; `reason` is the verdict's text or the mount's refusal and names the rule that fired. Removing the namespace root fails with it even with `recursive`. |
| `Unsupported { path, detail }` | The serving backend does not implement the operation. A `rename` or `copy` across two mounts fails with it, and so does opening the store on a handle that declared no store. |
| `Conflict { path, detail }` | The operation conflicts with a claim made by another live access; `detail` names both identities and both claim kinds. Step 4 of [Give a run its files](#give-a-run-its-files) shows one, and the refused write never partly applies. |
| `Backend { message }` | The serving backend failed for any other reason, and `message` is its own diagnosis. The real backend turns an I/O error kind it does not recognize into it. |

## Policy

[`Policy`] is the rule a handle asks about every operation, before the claims check. Write one when you need rules about what a run may do to files. [`AllowAll`] and [`ModePolicy`] implement it: `AllowAll` never changes its answer, `ModePolicy` changes it through its mode cell, and a policy of your own can do the same through shared state. A refusal is [`VfsError::PermissionDenied`], and `reason` holds the verdict's text; installing a policy takes `Policy + Sync + 'static`. See [Keep a run from changing files](#keep-a-run-from-changing-files).

- [`Policy::check`]: a verdict that changes mid-run must come from state the policy shares, as the mode cell does in `ModePolicy`.

## Vfs

[`Vfs`] is one storage backend behind the virtual namespace; it is synchronous, and its operations work on bytes. Implement it when you write a custom backend to mount. [`MemoryBackend`], [`RealBackend`], and [`VfsRef`], a handle mounted as a backend, implement it. A backend whose `acquire` fails refuses that session, and the operation that first reached it fails with the error the backend's `acquire` returned, passed through unchanged, and never panics. A backend that returns [`VfsError::Backend`] surfaces as `Backend` with its message, so return a [`VfsError`] from `acquire` that says why.

- [`Vfs::acquire`]: returns an access object bound to the identity in `cx`; a wrapping backend passes `cx` through unchanged.
- [`Vfs::release`]: ends the backend session for an identity, once per mount it touched, when the access drops; claims end with the scope.
- [`Vfs::read_only`]: defaults to `false`; return `true` to reject every change.

## VfsAccess

[`VfsAccess`] is one identity's session with a backend; it holds the [`ExecId`] and declares every filesystem operation. Implement it in a custom backend. A backend does not re-check a path, because paths arrive validated and canonicalized; a [`VfsPath`] is a canonical string shared by reference counting, with no global table. A glob pattern arrives as plain text, and both bundled backends check its grammar themselves.

- [`VfsAccess::symlink`], `read_link`, and `chmod`: return [`VfsError::Unsupported`] unless your backend overrides them; neither bundled backend does.
- [`VfsAccess::glob`]: returns files and directories together, sorted; `**` matches across segments and `*` stays within one.
- [`VfsAccess::mkdir`]: fails with `AlreadyExists` for any existing path even with `recursive`; without `recursive`, a missing parent raises `NotFound` naming the target.
- [`VfsAccess::str_replace`]: the default reads, counts non-overlapping matches, replaces, and writes; an empty `old` is refused first as `Anchor`.
- [`VfsAccess::read_range`]: the default reads the whole file and slices it; an offset past the end gives an empty vec, not an error.

## perform_vfs_op

[`perform_vfs_op`] runs one [`VfsOp`] against the run's store view and returns its [`VfsOutcome`]; it is the work behind an [`Effect::Vfs`](crate::effect::Effect::Vfs). It is synchronous, so run it off your async executor. On failure it returns the store's [`VfsError`], and you resume the run with that error as the answer. The run raises it at the call site in the prompt, except [`VfsError::Conflict`], which ends the run with [`RunErrorKind::Determinism`](crate::RunErrorKind::Determinism) and no Lua `pcall` can catch it.

## OpSink

[`OpSink`] is the callback type a handle fires for every admitted file operation, with the kind, the canonical path, and the [`Origin`]. Install one with [`VfsRefBuilder::on_op`], which wraps any matching closure in an [`Arc`](std::sync::Arc). It fires after the policy and claims pass, so a denied operation never fires it. Keep it cheap, because store operations fire it inline with the operation. [Watch every file operation](#watch-every-file-operation) shows one.
