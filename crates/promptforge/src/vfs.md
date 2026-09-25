The virtual filesystem behind a run's store: handles, backends, mounts, policies, and the answer to a store effect.

Every file a prompt reads or writes through its `store` table lives in one virtual namespace of absolute, POSIX-style paths, served by backends mounted at path prefixes. This module lets the host build that namespace, seed it before a run, read it after, and answer the run's store effects against it. The host can also plug in storage backends and access policies of its own. The filesystem is synchronous by design and works on bytes.

# Where this fits

Before a run, the host decides what the run's filesystem holds. [`Environment::base_vfs`](crate::Environment::base_vfs) mounts host directories under `/` for every run. [`Environment::prepare`](crate::Environment::prepare) then gives the [`RunContext`](crate::RunContext) a per-run handle that adds a fresh store at `/_promptforge/store`. A host that activates capabilities builds that per-run handle first with [`Environment::run_vfs`](crate::Environment::run_vfs), hands it to the capabilities, and sets it with [`RunContext::vfs`](crate::RunContext::vfs). [`Environment::prepare`](crate::Environment::prepare) keeps a handle set this way, so the capabilities and the run share one store. Either way, the host seeds files through [`RunContext::vfs_handle`](crate::RunContext::vfs_handle) before [`Run::new`](crate::Run::new), and reads output through it after the run.

During the run, every `store.*` call in a prompt reaches the host from [`Run::step`](crate::Run::step) inside [`Step::Pending`](crate::Step::Pending) as an [`Effect::Store`](crate::effect::Effect::Store). Its [`access`](crate::effect::Effect#variant.Store.field.access) field is an [`Arc`](std::sync::Arc) of an [`Access`] already scoped to the run's store, and its [`op`](crate::effect::Effect#variant.Store.field.op) field is a [`StoreOp`]. The host performs the operation with [`perform_store_op`] and hands the whole [`Result`] back through [`Run::resume`](crate::Run::resume) as an [`EffectAnswer::Store`](crate::effect::EffectAnswer::Store). The crate page's host loop does exactly this.

Three rules apply to the [`Access`] in a store effect. The host uses it as given and never derives or widens store scope from it. The host drops it after the operation and before resuming, because an [`Access`] keeps its hold on every path it touched until it is dropped. And because [`perform_store_op`] is synchronous, an async host runs it off its executor, for example on a blocking pool.

A [`StoreError`] in the answer is raised in the prompt at the `store.*` call when the answer is resumed, so the prompt author can handle it. The exception is [`StoreError::WriteRace`], which ends the run with [`RunErrorKind::Determinism`](crate::RunErrorKind::Determinism). For a log, [`Effect::record`](crate::effect::Effect::record) keeps the [`StoreOp`] as [`EffectRecord::Store`](crate::effect::EffectRecord::Store) and drops the access, and [`EffectAnswer::record`](crate::effect::EffectAnswer::record) keeps the outcome as [`AnswerRecord::Store`](crate::effect::AnswerRecord::Store). The run also reports each store outcome on its [`Event`](crate::event::Event) stream, which the [`event`](crate::event) page lists.

# A first filesystem

This program builds an in-memory filesystem, writes and edits a few files, and reads them back several ways.

````
use promptforge::vfs::{FileType, MemoryBackend, Origin, VfsRef};

let vfs = VfsRef::new(MemoryBackend::new());
let access = vfs.acquire(Origin::new("memory backend example"))?;
access.write("/notes.md", b"todo")?;
assert_eq!(access.read("/notes.md")?, b"todo");

access.append("/log/today.txt", b"one\n")?;
access.append("/log/today.txt", b"two\nthree\n")?;
assert_eq!(access.read_string("/log/today.txt")?, "one\ntwo\nthree\n");
assert_eq!(access.read_range("/log/today.txt", 2, None)?, "two\nthree");
assert_eq!(access.read_range_numbered("/log/today.txt", 2, Some(3))?, "2| two\n3| three");

access.str_replace("/notes.md", "todo", "done")?;
assert_eq!(access.read_string("/notes.md")?, "done");

assert!(access.exists("/log")?);
assert_eq!(access.glob("/log/*.txt")?, ["/log/today.txt"]);

let entries = access.list("/")?;
let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
assert_eq!(names, ["log", "notes.md"]);

let stat = access.stat("/notes.md")?;
assert!(matches!(stat.file_type, FileType::File));
assert_eq!(stat.size, 4);
assert!(stat.mode.is_none());
# Ok::<(), Box<dyn std::error::Error>>(())
````

Here is what each part does.

1. **Build the handle.** [`VfsRef::new`] wraps one backend that serves the whole namespace. [`MemoryBackend::new`] returns an empty in-memory backend that holds only the root directory `/`.
2. **Acquire an access.** Every operation goes through an [`Access`], and [`VfsRef::acquire`] is the only way to get one. The [`Origin`] labels the operations for observers and never allows or refuses anything.
3. **Write and append.** [`Access::write`] creates or replaces a file with the given bytes. [`Access::append`] adds bytes at the end and creates the file when it is absent. The built-in backends create missing parent directories, so `/log` exists without a separate [`Access::mkdir`].
4. **Read.** [`Access::read`] returns the bytes as stored. [`Access::read_string`] returns them as UTF-8 text. [`Access::read_range`] returns a 1-based, inclusive line range, and [`Access::read_range_numbered`] adds line numbers for a model to navigate by.
5. **Edit in place.** [`Access::str_replace`] replaces the one occurrence of an anchor text. It refuses when the anchor occurs zero times or more than once.
6. **Look around.** [`Access::exists`] tests a path, [`Access::glob`] finds paths by pattern, [`Access::list`] lists a directory as [`Entry`] values, and [`Access::stat`] returns a [`Stat`]. The memory backend tracks no mode bits or timestamps, so it reports them as [`None`].

Every method returns a [`Result`] whose error is a [`VfsError`]. The [Reference](#reference) section covers each method's arguments and failures.

# Paths

A virtual path is absolute, starts with `/`, and uses `/` between segments. Each operation canonicalizes its path when it receives it. Duplicate slashes collapse, so `/a//b///c` becomes `/a/b/c`. A `.` segment vanishes, and a `..` segment removes the segment before it, so `/a/b/../c` becomes `/a/c`. A backslash counts as a separator, and a trailing `/` is dropped. Case is preserved and significant. An empty path, a relative path, or a path that climbs above the root, such as `/..`, fails with [`VfsError::InvalidPath`].

The canonical form is a [`VfsPath`], or a [`VfsPathBuf`] when it must be owned. Policies and custom backends receive paths in that form, so they never check a path again.

````
use promptforge::vfs::{MemoryBackend, Origin, VfsError, VfsRef};

let vfs = VfsRef::new(MemoryBackend::new());
let access = vfs.acquire(Origin::new("path example"))?;
access.write("/drafts//plan/./today.md", b"ship it")?;
assert_eq!(access.read_string("/drafts/plan/today.md")?, "ship it");
assert_eq!(access.read_string("/drafts/old/../plan/today.md")?, "ship it");
assert_eq!(access.read_string("/drafts\\plan\\today.md")?, "ship it");

assert!(matches!(access.read("drafts/plan/today.md"), Err(VfsError::InvalidPath(_))));
assert!(matches!(access.read("/.."), Err(VfsError::InvalidPath(_))));
assert!(matches!(access.read("/Drafts/plan/today.md"), Err(VfsError::NotFound(_))));
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Identities and claims

Each [`VfsRef::acquire`] vends a fresh [`ExecId`], a process-unique identity, and binds the new [`Access`] to it. Each operation through that access registers a *claim* on its canonical path under that identity. An operation that only looks registers a read claim, and one that changes the path registers a write claim. Claims keep concurrent work from interleaving on one path.

- A write conflicts with another identity's read or write claim on the path.
- A read conflicts only with another identity's write claim.
- Two reads never conflict, and an identity never conflicts with itself.

A conflicting operation fails with [`VfsError::Conflict`] and never reaches the backend. Its message names the path, both identities, and both claim kinds, in the form `"{kind} on {path} by {id:?} conflicts with a {other_kind} claim by {other:?}"`. Claims last for the life of the [`Access`], not for one call. So an identity that read a path blocks another live identity's write to it until the reader's [`Access`] is dropped. Clones of a [`VfsRef`] share one claims table, so a claim made through one clone conflicts with operations through another.

Dropping an [`Access`] releases every claim its identity holds, so cancellation, panics, and early returns cannot leak claims. The drop then calls the backend's [`Vfs::release`] and ignores its error.

The [`Origin`] passed to [`VfsRef::acquire`] is for observability only, and it never appears in a claim. [`Origin::new`] takes a label and records the Rust call site as the position. [`Origin::at`] takes a label, a file, and a line, and records exactly those. Use it when the host knows a more useful position, such as a section name, the prompt's name, and a line in the prompt. Use the most specific label available: a section name for a chain, a tool id for a tool, or a fixture name for a test.

````
use promptforge::vfs::{MemoryBackend, Origin, VfsError, VfsRef};

let vfs = VfsRef::new(MemoryBackend::new());
let writer = vfs.acquire(Origin::at("## Draft", "notes", 12))?;
writer.write("/plan.md", b"step one")?;

let reader = vfs.acquire(Origin::new("claims example"))?;
assert!(matches!(reader.read("/plan.md"), Err(VfsError::Conflict(_))));

drop(writer);
assert_eq!(reader.read("/plan.md")?, b"step one");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Backends

A backend stores the files. [`Vfs`] is the trait every backend implements, and the crate ships two backends.

**[`MemoryBackend`]** keeps files in memory, keyed by canonical path. Writes create their parent directories, and removals are strict. Clones share one storage, so a host can seed content through one clone and mount another clone elsewhere. It ignores identities, so every session sees the same files.

**[`HostBackend`]** serves host directories through direct filesystem calls. [`HostBackend::identity`] maps virtual paths straight onto host paths with no containment. On Windows, the virtual spelling of `C:\Users\x` is `/C:/Users/x`, and the leading slash before the drive letter is stripped. [`HostBackend::rooted`] confines the backend under one host directory, chroot-style. It checks the directory when it builds the backend, and fails with [`VfsError::NotFound`] when the directory is absent or with [`VfsError::NotADirectory`] when the path is not a directory. Every resolved path is then checked against the root. A path that escapes it, for example through a link, fails with [`VfsError::PermissionDenied`] and a message ending in "escapes the mounted root". Host writes, copies, and renames are failure-atomic. A write goes to a sibling temporary file, is synced, and is renamed into place, and a failed operation leaves both paths unchanged with no temporary file left behind.

````
use promptforge::vfs::{HostBackend, VfsError};

let missing = HostBackend::rooted("this-directory-does-not-exist");
assert!(matches!(missing, Err(VfsError::NotFound(_))));

let _whole_disk = HostBackend::identity().with_read_only(true);
````

# Mounting

[`VfsRef::builder`] returns a [`VfsRefBuilder`]. Each [`VfsRefBuilder::mount`] call installs a backend at a prefix, and [`VfsRefBuilder::build`] freezes the mount table into a [`VfsRef`]. A handle built this way is a *router*, because it routes each path to one mount. The longest matching prefix serves each path, so a longer mount shadows the same prefix of a shorter one. Each backend sees paths relative to its own mount, so `/a/b/f.txt` reaches the backend mounted at `/a/b` as `/f.txt`. A path that no mount serves fails with [`VfsError::NotFound`] and the message "no mount serves {path}".

A router acquires a mounted backend lazily, on the first operation that touches the mount. So a backend that refuses an identity reports the error from that operation, not from [`VfsRef::acquire`].

A rename or a copy must stay within one mount, because a backend's atomicity stops at its mount boundary. Crossing mounts fails with [`VfsError::Unsupported`] instead of running as a silent non-atomic operation.

````
use promptforge::vfs::{MemoryBackend, Origin, VfsError, VfsRef};

let data = MemoryBackend::new();
let vfs = VfsRef::builder()
    .mount("/", MemoryBackend::new())
    .mount("/data", data.clone())
    .build();
let access = vfs.acquire(Origin::new("mount example"))?;
access.write("/data/report.md", b"q3")?;
access.write("/notes.md", b"draft")?;

let direct = VfsRef::new(data).acquire(Origin::new("mount example"))?;
assert_eq!(direct.read("/report.md")?, b"q3");
assert!(!direct.exists("/notes.md")?);

assert!(matches!(access.rename("/notes.md", "/data/notes.md"), Err(VfsError::Unsupported(_))));
assert!(access.exists("/notes.md")?);
# Ok::<(), Box<dyn std::error::Error>>(())
````

[`VfsRef::overlay`] returns a new handle with one more backend mounted at a prefix over an existing handle's namespace. The overlay shares the base's claims table, so conflicts are caught across both views. That is right only for two views of the same storage. The base does not see the overlay's mount:

````
use promptforge::vfs::{MemoryBackend, Origin, VfsError, VfsRef};

let base = VfsRef::builder().mount("/", MemoryBackend::new()).build();
let with_scratch = base.overlay("/scratch", MemoryBackend::new());

let writer = with_scratch.acquire(Origin::new("overlay example"))?;
writer.write("/notes.md", b"kept")?;
writer.write("/scratch/tmp.txt", b"throwaway")?;
drop(writer);

let reader = base.acquire(Origin::new("overlay example"))?;
assert_eq!(reader.read("/notes.md")?, b"kept");
assert!(matches!(reader.read("/scratch/tmp.txt"), Err(VfsError::NotFound(_))));
# Ok::<(), Box<dyn std::error::Error>>(())
````

A [`VfsRef`] itself implements [`Vfs`], so one handle can be mounted inside another builder or overlay. The inner handle's policy and claims then apply under the caller's identity. For example, when an outer handle mounts a base handle at `/base`, a second identity reading `/base/f.txt` through the outer handle conflicts with a first identity's write to it.

# Read-only mounts

A backend whose [`Vfs::read_only`] returns `true` is a read-only mount. A router refuses every mutation on it with [`VfsError::PermissionDenied`] before the backend is touched, while reads work as usual. The message is "the mount at {prefix} is read-only, so {path} cannot be mutated". A copy is refused when its destination is read-only, and a rename when either end is. The flag is a property of the mount, independent of any access rules the handle adds.

Only a router enforces the flag. A backend wrapped directly by [`VfsRef::new`] has no router in front of it, so it must reject mutations itself. [`HostBackend::with_read_only`] covers both cases. With `true`, the host backend refuses every mutation itself with "the host backend is read-only, so {path} cannot be mutated" before touching disk, and it reports the flag through [`Vfs::read_only`] so a router refuses too.

This backend wraps a memory backend and declares itself read-only. The example seeds its storage through a clone first:

````
use promptforge::vfs::{ExecId, MemoryBackend, Origin, Vfs, VfsAccess, VfsError, VfsRef};

/// Serves a memory backend's files and refuses every mutation.
struct Sealed(MemoryBackend);

impl Vfs for Sealed {
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        self.0.acquire(id)
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.0.release(id)
    }

    fn read_only(&self) -> bool {
        true
    }
}

let reference = MemoryBackend::new();
VfsRef::new(reference.clone())
    .acquire(Origin::new("seed"))?
    .write("/style.md", b"be brief")?;

let vfs = VfsRef::builder()
    .mount("/", MemoryBackend::new())
    .mount("/reference", Sealed(reference))
    .build();
let access = vfs.acquire(Origin::new("read-only example"))?;
assert_eq!(access.read("/reference/style.md")?, b"be brief");
assert!(matches!(access.write("/reference/style.md", b"x"), Err(VfsError::PermissionDenied(_))));
access.write("/notes.md", b"writable")?;
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Policies

A *policy* decides, per operation and path, whether the operation may proceed. The handle consults it on every operation after the path is canonicalized and before any claim is registered, so a refused operation registers no claim. [`VfsRef::with_policy`] installs a policy on a single-backend handle, and [`VfsRefBuilder::policy`] installs one on a router. The default is [`AllowAll`]. An overlay shares its base's policy.

A custom policy implements [`Policy`]. Its one method, [`Policy::check`], takes the [`Op`] being attempted and the canonical [`VfsPath`], and returns a [`Verdict`]:

- [`Verdict::Allow`] lets the operation proceed to the claims check and the backend.
- [`Verdict::Deny`] refuses it. Its text goes back to the model as the tool error, so it should say how to recover.
- [`Verdict::Ask`] means the operation needs user approval, and its text is the approval dialog. At this layer it fails the same way as [`Verdict::Deny`], and the approval flow belongs to the host above.

Both refusals reach the caller as [`VfsError::PermissionDenied`] carrying the verdict's text. [`Policy::check`] takes `&self`, so a policy that changes behavior mid-run keeps shared state, such as an [`Arc`](std::sync::Arc) of a [`Mutex`](std::sync::Mutex) holding a [`Verdict`]. The very next operation sees the change. A rename is checked once for each of its two paths, and so is a copy. A policy cannot tell a copy's source check from its destination check.

````
use promptforge::vfs::{MemoryBackend, Op, Origin, Policy, Verdict, VfsError, VfsPath, VfsRef};

/// Lets every operation read, and lets mutations touch only `/drafts`.
struct DraftsOnly;

impl Policy for DraftsOnly {
    fn check(&self, op: Op, path: &VfsPath) -> Verdict {
        let reads = matches!(op, Op::Read | Op::Exists | Op::Glob | Op::List | Op::Stat | Op::Grep | Op::ReadLink);
        if reads || path.as_str().starts_with("/drafts/") {
            Verdict::Allow
        } else {
            Verdict::Deny(format!("{op:?} on {path} is refused: write under /drafts instead"))
        }
    }
}

let vfs = VfsRef::builder()
    .mount("/", MemoryBackend::new())
    .policy(DraftsOnly)
    .build();
let access = vfs.acquire(Origin::new("policy example"))?;
access.write("/drafts/plan.md", b"step one")?;
assert!(matches!(access.write("/plan.md", b"step one"), Err(VfsError::PermissionDenied(_))));
# Ok::<(), Box<dyn std::error::Error>>(())
````

**The editor mode gate.** [`ModePolicy`] gates model mutations by the editor's current [`Mode`], and never gates reads. It treats [`Op::Write`], [`Op::Append`], [`Op::Delete`], [`Op::Rename`], [`Op::Mkdir`], [`Op::Copy`], [`Op::Symlink`], and [`Op::Chmod`] as mutations.

- [`Mode::Ask`] routes every mutation to user approval with [`Verdict::Ask`]. It does not refuse outright, but at this layer the call still fails with [`VfsError::PermissionDenied`], carrying the dialog text.
- [`Mode::Plan`] allows mutations only to paths ending in `.md`, checked case-sensitively, so `.MD` does not count. A copy from a non-`.md` source is refused, because the source path is checked too.
- [`Mode::Agent`] allows every operation.

The UI flips the mode mid-run through a [`ModeHandle`]. Take it from [`ModePolicy::handle`] before installing the policy, because installing moves the policy into the handle. [`ModeHandle::set`] takes effect on the next operation.

````
use promptforge::vfs::{MemoryBackend, Mode, ModePolicy, Origin, VfsError, VfsRef};

let policy = ModePolicy::new(Mode::Ask);
let mode = policy.handle();
let vfs = VfsRef::with_policy(MemoryBackend::new(), policy);
let access = vfs.acquire(Origin::new("mode example"))?;
assert!(matches!(access.write("/plan.md", b"x"), Err(VfsError::PermissionDenied(_))));

mode.set(Mode::Plan);
access.write("/plan.md", b"step one")?;
assert!(matches!(access.write("/plan.txt", b"x"), Err(VfsError::PermissionDenied(_))));
assert_eq!(access.read("/plan.md")?, b"step one");

mode.set(Mode::Agent);
access.write("/plan.txt", b"anything")?;
assert!(mode.mode() == Mode::Agent);
# Ok::<(), Box<dyn std::error::Error>>(())
````

# Observing operations

[`VfsRefBuilder::on_op`] installs a callback, the *op sink*, that fires on every admitted operation with an [`OpEvent`]. The event reports the operation kind through [`OpEvent::op`], the canonical path through [`OpEvent::path`], and the caller's [`Origin`] through [`OpEvent::origin`]. The sink fires after the policy and the claims pass, and before the backend executes. It is fire-and-forget, so no outcome flows back, and a refused operation never fires it. A rename or a copy fires once per path. The sink runs inline with each operation, including store operations that a host performs on a blocking pool, so it must be cheap. An overlay of the built handle shares the sink. The op sink is separate from the run's [`Event`](crate::event::Event) stream.

````
use std::sync::{Arc, Mutex};

use promptforge::vfs::{MemoryBackend, Op, OpEvent, Origin, VfsRef};

let seen = Arc::new(Mutex::new(Vec::new()));
let sink_seen = Arc::clone(&seen);
let vfs = VfsRef::builder()
    .mount("/", MemoryBackend::new())
    .on_op(move |event: OpEvent<'_>| {
        if let Ok(mut log) = sink_seen.lock() {
            log.push((event.op(), event.path().to_string(), event.origin().label.clone()));
        }
    })
    .build();

let access = vfs.acquire(Origin::new("observer example"))?;
access.write("/a.txt", b"x")?;
drop(access);

let log = seen.lock().map_err(|_| "the sink panicked")?;
assert_eq!(log.len(), 1);
assert!(matches!(log[0].0, Op::Write));
assert_eq!(log[0].1, "/a.txt");
assert_eq!(log[0].2, "observer example");
# Ok::<(), Box<dyn std::error::Error>>(())
````

# The run's store

A run's store is a mount at `/_promptforge/store` inside the run's handle, and the prompt's `store` table is scoped to it. One handle serves every section of a run, so store files persist from section to section even though each section's Lua state does not. [`RunContext::new`](crate::RunContext::new) starts with a fresh memory backend at the store mount. [`Environment::run_vfs`](crate::Environment::run_vfs) builds a router with the environment's base mounted at `/` plus a fresh memory backend at the store mount, so every run shares the base and gets its own store. Several concurrent runs can share one host-backed base this way. When a second run writes a path such as `/shared.txt` while the first run's claim on it is live, the write fails with [`VfsError::Conflict`], and the file keeps the first run's contents.

A [`StoreOp`] names its paths logically, relative to the store mount. So `notes.md` means `/_promptforge/store/notes.md`, and a [`StoreOp`] can reach only files inside the run's store. The host seeds and extracts through the full virtual path. [`perform_store_op`] validates each logical path before any backend sees it, and reports a broken rule as [`StoreError::InvalidPath`] with a [`PathReason`]. The checks run in this order, and the first rule broken is reported:

1. The path is empty: [`PathReason::Empty`].
2. The path is over 1024 bytes: [`PathReason::TooLong`].
3. The path starts with `/`: [`PathReason::Absolute`].
4. The path contains a control character: [`PathReason::Control`].
5. The path contains a backslash: [`PathReason::Backslash`].
6. Then, for each segment between slashes: an empty segment gives [`PathReason::EmptySegment`], a `.` or `..` segment gives [`PathReason::Traversal`], a trailing `.` or space gives [`PathReason::UnsafeSuffix`], and a reserved device name gives [`PathReason::ReservedName`]. The reserved names are `CON`, `PRN`, `AUX`, `NUL`, `COM1` to `COM9`, and `LPT1` to `LPT9`.

[`StoreError::kind`] classifies a failure with a stable [`StoreErrorKind`]. Match on the kind rather than on the error's variants, as the error's own documentation directs. [`StoreError::is_not_found`] tells whether the file was absent, and [`StoreError::path`] recovers the logical path. A host store performer that fails for its own reasons wraps its error with [`StoreError::backend`].

This example seeds a file through the context's handle, then calls [`perform_store_op`] against it, as a host loop does for each store effect:

````
use promptforge::RunContext;
use promptforge::timestamp::Timestamp;
use promptforge::vfs::{
    perform_store_op, Origin, PathReason, StoreError, StoreErrorKind, StoreOp, StoreOutcome,
};

let ctx = RunContext::new("store example", 7, Timestamp::UNIX_EPOCH);
let seed = ctx.vfs_handle().acquire(Origin::new("seed"))?;
seed.write("/_promptforge/store/brief.md", b"one\ntwo\n")?;
drop(seed);

let access = ctx.vfs_handle().acquire(Origin::new("store example"))?;
let read = StoreOp::Read { path: "brief.md".to_owned(), start: Some(2), end: None };
let StoreOutcome::Text(text) = perform_store_op(&access, read)? else {
    panic!("a read answers with text");
};
assert_eq!(text, "two");

let missing = StoreOp::Read { path: "missing.md".to_owned(), start: None, end: None };
let Err(error) = perform_store_op(&access, missing) else {
    panic!("the file is absent");
};
assert!(error.kind() == StoreErrorKind::NotFound);
assert!(error.is_not_found());
assert_eq!(error.path(), Some("missing.md"));

let absolute = StoreOp::Write { path: "/brief.md".to_owned(), contents: "x".to_owned() };
let Err(error) = perform_store_op(&access, absolute) else {
    panic!("store paths are relative");
};
assert!(matches!(error, StoreError::InvalidPath { reason: PathReason::Absolute, .. }));

let own = StoreError::backend(std::io::Error::other("disk gone"));
assert!(own.kind() == StoreErrorKind::Backend);
# Ok::<(), Box<dyn std::error::Error>>(())
````

[`StoreOp`] implements serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) and [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html), so a host can write store operations into an effect record or a replay log and read them back.

# Implementing a backend

A backend implements two traits. [`Vfs`] is the backend itself. [`Vfs::acquire`] opens a session for one [`ExecId`] and returns it boxed, and [`Vfs::release`] ends the session. [`Vfs::read_only`] defaults to `false`. A [`Vfs`] must be [`Send`] but need not be [`Sync`], because the handle serializes access to it. [`VfsAccess`] is one identity's session, and it declares every filesystem operation. Paths arrive validated, canonical, and relative to the backend's mount, so a backend never checks them again.

- Eleven methods are required: [`VfsAccess::read`], [`VfsAccess::write`], [`VfsAccess::append`], [`VfsAccess::remove`], [`VfsAccess::exists`], [`VfsAccess::glob`], [`VfsAccess::list`], [`VfsAccess::stat`], [`VfsAccess::mkdir`], [`VfsAccess::rename`], and [`VfsAccess::copy`].
- Six have default bodies, and a backend overrides any of them to push work down: [`VfsAccess::read_range`], [`VfsAccess::str_replace`], [`VfsAccess::grep`], [`VfsAccess::symlink`], [`VfsAccess::read_link`], and [`VfsAccess::chmod`]. For example, a backend can add regex search by overriding [`VfsAccess::grep`], or seeking reads by overriding [`VfsAccess::read_range`], as [`HostBackend`] does.

**[`Stat`] and [`Entry`] have no public constructor, so a custom backend cannot build them.** Both are `#[non_exhaustive]`, which rules out a struct literal, and the crate offers no other way to make one. For [`VfsAccess::stat`] and [`VfsAccess::list`], a custom backend can only pass through values obtained from another backend, for example by delegating to a wrapped [`MemoryBackend`] session, as the example below does. [`GrepMatch`] has no public constructor either, so an overriding [`VfsAccess::grep`] can fill its results only with matches from another backend. [`GrepResults`] is the exception: [`GrepResults::default`] builds an empty value, and its public fields can then be assigned.

This backend refuses any single write or append over a byte limit, and delegates everything else to a memory backend session:

````
use promptforge::vfs::{
    Entry, ExecId, MemoryBackend, Origin, Stat, Vfs, VfsAccess, VfsError, VfsPath, VfsRef,
};

/// Refuses any single write or append larger than `limit` bytes.
struct Capped {
    inner: MemoryBackend,
    limit: usize,
}

struct CappedSession {
    inner: Box<dyn VfsAccess>,
    limit: usize,
}

impl Vfs for Capped {
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError> {
        let inner = self.inner.acquire(id)?;
        Ok(Box::new(CappedSession { inner, limit: self.limit }))
    }

    fn release(&mut self, id: ExecId) -> Result<(), VfsError> {
        self.inner.release(id)
    }
}

impl CappedSession {
    fn within_limit(&self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        if contents.len() > self.limit {
            return Err(VfsError::PermissionDenied(format!(
                "{path} would take more than {} bytes in one call",
                self.limit
            )));
        }
        Ok(())
    }
}

impl VfsAccess for CappedSession {
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError> {
        self.inner.read(path)
    }

    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.within_limit(path, contents)?;
        self.inner.write(path, contents)
    }

    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError> {
        self.within_limit(path, contents)?;
        self.inner.append(path, contents)
    }

    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner.remove(path, recursive)
    }

    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError> {
        self.inner.exists(path)
    }

    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError> {
        self.inner.glob(pattern)
    }

    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError> {
        self.inner.list(path)
    }

    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError> {
        self.inner.stat(path)
    }

    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError> {
        self.inner.mkdir(path, recursive)
    }

    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.rename(from, to)
    }

    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError> {
        self.inner.copy(from, to)
    }
}

let vfs = VfsRef::new(Capped { inner: MemoryBackend::new(), limit: 8 });
let access = vfs.acquire(Origin::new("capped example"))?;
access.write("/small.txt", b"fits")?;
assert!(matches!(access.write("/big.txt", b"far too long"), Err(VfsError::PermissionDenied(_))));

let entries = access.list("/")?;
assert_eq!(entries.len(), 1);
assert_eq!(entries[0].stat.size, 4);
# Ok::<(), Box<dyn std::error::Error>>(())
````

Hosts implement [`Vfs`], [`VfsAccess`], and [`Policy`], so any method added to one of them later must have a default body. Adding one without a default would break every host implementation.

# Reference

This part covers every item in the module, grouped by task: handles and access first, then backends and their metadata, then policies and observation, then the store, and last the backend traits.

## VfsRef

[`VfsRef`] is the cloneable handle over one virtual namespace. Clones share the backends, the policy, the op sink, and the claims table. It is [`Send`] and [`Sync`]. The host builds one with the constructors below, or receives the run's handle from [`RunContext::vfs_handle`](crate::RunContext::vfs_handle). [`VfsRef::new`], [`VfsRef::with_policy`], and [`VfsRef::builder`] cannot fail.

- [`VfsRef::new`] takes `backend`, any `'static` [`Vfs`], which serves the whole namespace. It returns a handle with the [`AllowAll`] policy, no op sink, and a fresh claims table. The handle has no router, so it does not enforce [`Vfs::read_only`].
- [`VfsRef::with_policy`] takes `backend`, as for [`VfsRef::new`], and `policy`, any [`Policy`] that is also [`Sync`] and `'static`. It returns a handle with that policy, no op sink, and a fresh claims table.
- [`VfsRef::builder`] returns an empty [`VfsRefBuilder`], with no mounts, no policy, and no sink.
- [`VfsRef::overlay`] takes `&self`, a `prefix` of type [`&str`](str), and a `backend` that is any `'static` [`Vfs`]. The prefix is the absolute virtual path to mount the backend at. It returns a new handle whose namespace is this handle's namespace with the backend mounted at the prefix, sharing this handle's claims table, policy, and op sink. Operations outside the prefix route through this handle. It panics with "invalid overlay prefix {prefix:?}: {err}" when the prefix does not canonicalize, and with "an overlay at / would replace the base entirely; use VfsRef::new instead" for `/`. [`Environment::prepare`](crate::Environment::prepare) builds per-run stores with a fresh router instead of an overlay, because concurrent runs' stores are different storage.
- [`VfsRef::acquire`] takes `&self` and an [`Origin`], and returns a new [`Access`] bound to a fresh [`ExecId`]. The origin labels every operation the access performs. It fails with the backend's own error when a directly wrapped backend's [`Vfs::acquire`] refuses the identity. A router acquires each mount lazily, so a mount's refusal surfaces from the first operation on that mount.

[`VfsRef`] implements [`Vfs`], and its [`Vfs::read_only`] reports the wrapped backend's flag.

## VfsRefBuilder

[`VfsRefBuilder`] collects mounts, a policy, and an op sink, then freezes them into a router. [`VfsRef::builder`] is the only way to get one. Each method takes the builder by value and returns it, so calls chain. The mount table is fixed at [`VfsRefBuilder::build`], so it is immutable and cheap to share.

- [`VfsRefBuilder::mount`] takes a `prefix` of type [`&str`](str) and a `backend` that is any `'static` [`Vfs`]. The prefix is the absolute virtual path of the mount point. `/` serves the whole namespace, and a longer prefix shadows the same prefix of a shorter one. The backend sees paths relative to its mount, and the mount point itself is its `/`. It panics with "invalid mount prefix {prefix:?}: {err}" when the prefix does not canonicalize, and with "a mount already sits at {prefix:?}" when two mounts share a canonical prefix.
- [`VfsRefBuilder::policy`] takes `policy`, any [`Policy`] that is also [`Sync`] and `'static`, which the router consults on every operation. The default is [`AllowAll`]. A second call replaces the first.
- [`VfsRefBuilder::on_op`] takes `sink`, any closure that implements [`Fn`] of an [`OpEvent`] and is [`Send`], [`Sync`], and `'static`. The router calls it on every admitted operation. There is no sink by default, and a second call replaces the first.
- [`VfsRefBuilder::build`] returns a [`VfsRef`] over the frozen mount table, with a fresh claims table, the installed policy or [`AllowAll`], and the installed sink or none. It cannot fail. A builder with no mounts builds a handle where every path fails with [`VfsError::NotFound`].

## Origin

[`Origin`] says who asked for an operation: a label plus the most precise source position the caller knows. It is for observability only, and it never gates an operation or appears in a claim. The host builds one and passes it to [`VfsRef::acquire`], and an [`OpEvent`] hands it back through [`OpEvent::origin`]. It is `#[non_exhaustive]`, so there is no struct literal, and its fields are public for reading.

- [`Origin::new`] takes `label`, anything that converts [`Into`] a [`String`]. It records the Rust call site as the file and line, because it is `#[track_caller]`.
- [`Origin::at`] takes `label` as for [`Origin::new`], `file`, anything that converts [`Into`] a [`String`], and `line`, a [`u32`]. It records exactly those values. Use it for a prompt position: the prompt's name as the file and a 1-based line in the prompt.

Neither constructor can fail. [Identities and claims](#identities-and-claims) says which label to pass.

- [`Origin::label`], a [`String`], is the caller's label.
- [`Origin::file`], a [`String`], is the source file or document that the line refers to: a Rust source file from [`Origin::new`], or whatever was passed to [`Origin::at`].
- [`Origin::line`], a [`u32`], is the 1-based line within the file.

## Access

[`Access`] is the capability that every filesystem operation goes through. It is bound to one [`ExecId`] and one [`Origin`]. The host receives one from [`VfsRef::acquire`], or inside an [`Effect::Store`](crate::effect::Effect::Store), and never builds one. It is [`Send`] and [`Sync`]. It is `#[must_use]`, because an access dropped at once releases its identity before any work is done.

Each method canonicalizes its path arguments, consults the policy, registers a claim, fires the op sink, and then calls the backend, in that order. So every method can fail with [`VfsError::InvalidPath`] for a malformed path, [`VfsError::PermissionDenied`] when the policy refuses, [`VfsError::Conflict`] when a claim conflicts, and [`VfsError::NotFound`] when no mount serves the path. A mutation also fails with [`VfsError::PermissionDenied`] on a read-only mount. Any other backend error passes through. The entries below give each method's arguments, its return value, the [`Op`] it reports, and its other failures. Every path argument is a [`&str`](str) holding an absolute virtual path, such as `"/notes.md"`.

**Reading.**

- [`Access::read`] takes `path` and returns the file's bytes as stored, as a [`Vec`] of [`u8`]. It registers a read claim and reports [`Op::Read`]. It fails with [`VfsError::NotFound`] when the file is absent, and with [`VfsError::IsADirectory`] on a directory in the built-in backends.
- [`Access::read_string`] takes `path` and returns the contents as a UTF-8 [`String`]. It reads through [`Access::read`], so it claims and reports the same way. Content that is not UTF-8 fails with [`VfsError::Backend`] and the message "read_string requires UTF-8 text: {path}: {source}".
- [`Access::read_range`] takes `path`, a `start` of type [`usize`], and an `end` of type [`Option`] of [`usize`], and returns a [`String`]. The range is 1-based and inclusive. `start` must be at least 1. An `end` of [`None`] means the last line, and an `end` past the last line clamps to it. The selected lines are joined by `"\n"` with no trailing newline, and a `start` past the last line returns `""`. So `read_range("/f.txt", 2, None)` on `"one\ntwo\nthree\n"` returns `"two\nthree"`. Lines are split with [`str::lines`]. It reads the whole file through [`Access::read`], so it claims and reports the same way. A `start` of 0 fails with [`VfsError::Backend`] and "invalid line range for {path}: start {start} is below 1", checked before the file is read. A clamped `end` before `start` fails with "invalid line range for {path}: end {end} is before start {start}". Content that is not UTF-8 also fails.
- [`Access::read_range_numbered`] takes the same arguments as [`Access::read_range`] and fails the same ways. Each selected line starts with its absolute line number, right-aligned to the width of the largest number shown and followed by `"| "`. So `read_range_numbered("/f.txt", 9, Some(10))` returns `" 9| line9\n10| line10"`. Models use the numbers to navigate.

**Writing.**

- [`Access::write`] takes `path` and `contents`, a [`&[u8]`](slice) holding the complete new file, and creates or overwrites the file. It registers a write claim and reports [`Op::Write`]. It fails with [`VfsError::IsADirectory`] when a directory sits at the path, and with [`VfsError::NotADirectory`] in the memory backend when an ancestor is a file. The built-in backends create missing ancestor directories.
- [`Access::append`] takes `path` and `contents`, a [`&[u8]`](slice) of bytes to add at the end. It creates the file when it is absent, and the built-in backends also create missing ancestors. It registers a write claim, reports [`Op::Append`], and fails as [`Access::write`] does.
- [`Access::str_replace`] takes `path`, `old`, and `new`, each a [`&str`](str). `old` is the anchor text and must occur exactly once in the file, and `new` replaces it. The policy and claims treat it as a write, so it registers a write claim and reports [`Op::Write`]. With the default backend body it fails with [`VfsError::Backend`] and one of "str_replace requires UTF-8 text: {path}: {source}", "str_replace found no occurrence of {old:?} in {path}", or "str_replace found {count} occurrences of {old:?} in {path}; exactly one is required". A missing file fails with [`VfsError::NotFound`]. A backend can override the default body.
- [`Access::remove`] takes `path` and `recursive`, a [`bool`]. With `true` it removes a directory with its whole subtree. With `false` it removes only a file, a link, or an empty directory. On a symlink it removes the link, never the target. It registers a write claim and reports [`Op::Delete`]. It fails with [`VfsError::NotFound`] when the path is absent, and with [`VfsError::DirectoryNotEmpty`] for a non-empty directory without `recursive`. Removing the backend's root fails with [`VfsError::PermissionDenied`], as "the namespace root cannot be removed" in the memory backend and "the mounted root cannot be removed" in the host backend.
- [`Access::mkdir`] takes `path` and `recursive`, a [`bool`]. With `true` it also creates missing ancestors, and with `false` the parent must exist. It registers a write claim and reports [`Op::Mkdir`]. It fails with [`VfsError::AlreadyExists`] when anything already sits at the path. In the memory backend it fails with [`VfsError::NotADirectory`] when an ancestor is a file, and with [`VfsError::NotFound`] and "the parent of {path} does not exist" when the parent is missing without `recursive`.
- [`Access::rename`] takes `from` and `to`, and renames or moves a file or directory. `to` must be served by the same mount as `from`, and must not be inside `from`. Both paths are checked by the policy and claimed as writes under [`Op::Rename`], `from` first, and the op sink fires once per path. It fails with [`VfsError::InvalidPath`] and "cannot rename {from} into its own descendant {to}", with [`VfsError::Unsupported`] across mounts, and with [`VfsError::NotFound`] when the source is absent. Renaming a backend's root, or onto it, fails with [`VfsError::PermissionDenied`]. The memory backend moves a directory's whole subtree and can fail with [`VfsError::NotADirectory`] or [`VfsError::DirectoryNotEmpty`] for an incompatible directory destination. The rename is atomic where the backend allows.
- [`Access::copy`] takes `from` and `to`, and copies one file. The source is claimed as a read and the destination as a write, both under [`Op::Copy`], and the op sink fires once per path. It fails with [`VfsError::IsADirectory`] when either path is a directory, with [`VfsError::NotFound`] when the source is absent, and with [`VfsError::Unsupported`] across mounts. Only a read-only destination mount refuses it.

**Looking around.**

- [`Access::exists`] takes `path` and returns a [`bool`]: `true` when a file or directory exists there, and `false` only for a confirmed absence. A lookup that cannot decide is an error. For example, the host backend fails with [`VfsError::NotADirectory`] for a path through a file ancestor, and counts a dangling symlink as existing. It registers a read claim and reports [`Op::Exists`].
- [`Access::glob`] takes `pattern`, a [`&str`](str) holding an absolute glob over virtual paths, and returns the matching full virtual paths, sorted, as a [`Vec`] of [`String`]. A pattern holds literal bytes, `*` for zero or more bytes within one segment, and `**` for any number of whole segments. `**` must occupy a whole segment, as in `**`, `**/...`, `.../**`, or `.../**/...`. There are no escapes. Backslashes and runs of three or more `*` are rejected. The built-in backends return directories as well as files, and reject a pattern over 1024 bytes with [`VfsError::InvalidPath`] and "glob pattern exceeds 1024 bytes", or a malformed one with "invalid glob pattern {pattern:?}: {reason}". The claim, the policy check, and the op sink all use the canonicalized pattern as the path, with a read claim and [`Op::Glob`]. A router sends the pattern to the longest-prefix mount, strips the prefix, and joins it back onto each result.
- [`Access::list`] takes the `path` of a directory and returns its immediate children as a [`Vec`] of [`Entry`], sorted by name in the built-in backends. It registers a read claim and reports [`Op::List`]. It fails with [`VfsError::NotADirectory`] on a file and [`VfsError::NotFound`] when the path is absent.
- [`Access::stat`] takes `path` and returns its [`Stat`]. It registers a read claim and reports [`Op::Stat`]. It fails with [`VfsError::NotFound`] when the path is absent. The host backend does not follow symlinks here, so a link reports [`FileType::Symlink`].
- [`Access::grep`] takes `query`, a reference to a [`GrepQuery`], and returns a [`GrepResults`]. Through a router, each match's [`GrepMatch::path`] is the full virtual path. The query's [`GrepQuery::root`] is canonicalized, checked by the policy under [`Op::Grep`], and read-claimed. The claim and the op sink use the root only, not each searched file. It fails with [`VfsError::Unsupported`] when [`GrepQuery::is_regex`] is `true` and the backend uses the default body, and with any error from the backend's glob or reads. A host cannot build a [`GrepQuery`] of its own, as its entry explains, so in practice a host calls [`Access::grep`] only with a query it received and cloned.

Dropping an [`Access`] releases its claims and its backend session, as [Identities and claims](#identities-and-claims) describes.

## ExecId

[`ExecId`] is the opaque identity of one serial thread of execution, and every operation and claim is attributed to one. The handle vends each one from a process-wide counter, so each is unique in the process. Hosts never build one. A backend receives it as the `id` argument of [`Vfs::acquire`] and [`Vfs::release`], and can use it to tell sessions apart. It is [`Copy`](std::marker::Copy) and [`Hash`](std::hash::Hash), so it works as a map key for per-identity state. Its [`Debug`](std::fmt::Debug) form appears in [`VfsError::Conflict`] messages.

## VfsError

[`VfsError`] is the one error type every filesystem operation returns. The host receives it from [`Access`], [`VfsRef::acquire`], and [`HostBackend::rooted`]. A custom backend builds variants directly, for example `VfsError::NotFound(path.to_string())`. Every variant holds a [`String`] with a human-readable message. The enum is `#[non_exhaustive]`, so a `match` needs a wildcard arm.

- [`VfsError::NotFound`]: the path does not exist in the serving backend. A router also returns it when no mount serves the path, [`HostBackend::rooted`] returns it for an absent directory, and the memory backend returns it for a non-recursive [`Access::mkdir`] with a missing parent. Through the store it becomes [`StoreError::NotFound`], or success for [`StoreOp::Delete`].
- [`VfsError::PermissionDenied`]: the operation is not permitted. The causes are a policy [`Verdict::Deny`] or [`Verdict::Ask`], whose text is the message, a read-only mount, a host path escaping its rooted directory, removing or renaming a backend's root, or a host OS permission error. Show the message to the model or the user. Retrying unchanged fails again.
- [`VfsError::AlreadyExists`]: the path already exists where creation required absence, for example [`Access::mkdir`] on an existing path.
- [`VfsError::InvalidPath`]: the path is malformed or escapes the root. The messages are "empty path", "relative path is not in the virtual namespace: {path:?}", and "path escapes the namespace root: {path:?}". It also covers an invalid or over-long glob pattern, and a rename into the source's own descendant.
- [`VfsError::NotADirectory`]: a directory operation named a non-directory. That covers [`Access::list`] of a file, a path through a file ancestor, and [`HostBackend::rooted`] on a file.
- [`VfsError::IsADirectory`]: a file operation named a directory, such as [`Access::read`], [`Access::write`], [`Access::append`], or [`Access::copy`] on a directory.
- [`VfsError::DirectoryNotEmpty`]: a removal without `recursive` named a non-empty directory. The memory backend also returns it for a directory renamed onto a non-empty directory. Retry with `recursive` set to `true` if that was intended.
- [`VfsError::Unsupported`]: the serving backend does not implement the operation. That covers a rename or copy across mounts, whose message is "{op} across mounts is unsupported: {from} and {to} are served by different mounts", a regex grep against the default body, and the default [`VfsAccess::symlink`], [`VfsAccess::read_link`], and [`VfsAccess::chmod`].
- [`VfsError::Conflict`]: the operation conflicts with another live identity's claim, and it never reached the backend. The message is `"{kind} on {path} by {id:?} conflicts with a {other_kind} claim by {other:?}"`. During a run, a conflict on a store path becomes [`StoreError::WriteRace`] and ends the run.
- [`VfsError::Backend`]: the serving backend failed for any other reason. That covers text that is not UTF-8 in [`Access::read_string`], [`Access::read_range`], or the default [`VfsAccess::str_replace`], an invalid line range in [`Access::read_range`], a default [`VfsAccess::str_replace`] match count other than one, a backend refusing an identity, and an unmapped host I/O error.

[`VfsError`] implements [`std::error::Error`]. Its [`Display`](std::fmt::Display) form puts a fixed prefix before the message: "not found: ", "permission denied: ", "already exists: ", "invalid path: ", "not a directory: ", "is a directory: ", "directory not empty: ", "unsupported operation: ", "conflicting claim: ", or "backend failure: ".

## MemoryBackend

[`MemoryBackend`] is an in-memory backend that stores bytes keyed by canonical path. Build one with [`MemoryBackend::new`], which takes no arguments, cannot fail, and returns an empty backend holding only the root directory `/`. [`MemoryBackend::default`] returns the same thing. A clone shares the same storage, so a clone mounted elsewhere or wrapped in another handle sees the same files. It implements [`Vfs`], and its [`Vfs::read_only`] is `false`. The struct is `#[non_exhaustive]`.

It ignores identities, so every session shares one map. Its behavior differs from a host directory in a few places:

- Reading a directory fails with [`VfsError::IsADirectory`], and so does copying one.
- Removing an absent path fails with [`VfsError::NotFound`], removing a non-empty directory without `recursive` fails with [`VfsError::DirectoryNotEmpty`], and removing `/` fails with [`VfsError::PermissionDenied`].
- [`Access::mkdir`] on any existing path fails with [`VfsError::AlreadyExists`].
- [`Access::glob`] returns files and directories, sorted, and [`Access::list`] sorts by name.
- [`Access::stat`] reports [`FileType::File`] with the byte length as the size, or [`FileType::Directory`] with a size of 0. The mode and both timestamps are [`None`].
- [`Access::rename`] moves a directory's whole subtree, and refuses `/` as the source or as a directory's destination.

## HostBackend

[`HostBackend`] serves host OS directories behind the virtual namespace through direct [`std::fs`] calls. It accepts and ignores identities. Build one with [`HostBackend::identity`] or [`HostBackend::rooted`], and optionally chain [`HostBackend::with_read_only`]. The struct is `#[non_exhaustive]` with private fields.

- [`HostBackend::identity`] takes no arguments, cannot fail, and returns a writable backend whose virtual paths are host paths. Virtual `/a/b` is host `/a/b`, and on Windows virtual `/C:/a/b` is host `C:\a\b`. No containment applies.
- [`HostBackend::rooted`] takes `dir`, anything that implements [`AsRef`] of [`Path`](std::path::Path). The directory becomes the virtual root `/`. It must exist and be a directory, and it is canonicalized when the backend is built. It returns a writable, chroot-style backend in a [`Result`]. It fails with [`VfsError::NotFound`] when the directory is absent, with [`VfsError::NotADirectory`] when the path is not a directory, and with other I/O failures mapped by kind. Every later path is checked by canonicalizing its nearest existing ancestor. A path that resolves outside the root fails with [`VfsError::PermissionDenied`] and "{path} escapes the mounted root". A dangling symlink inside the root is not itself resolved.
- [`HostBackend::with_read_only`] takes the backend by value and `read_only`, a [`bool`], and returns the backend with the flag set. It cannot fail. With `true`, [`Access::write`], [`Access::append`], [`Access::remove`], [`Access::mkdir`], [`Access::rename`] on its source, and [`Access::copy`] on its destination all fail with [`VfsError::PermissionDenied`] and "the host backend is read-only, so {path} cannot be mutated" before touching disk. Reads still work. The default from both constructors is `false`. [`Vfs::read_only`] reports the flag.

Host I/O errors map by kind onto the same-named [`VfsError`] variant: not found, permission denied, already exists, is a directory, not a directory, and directory not empty. Every other error becomes [`VfsError::Backend`] with "{path}: {err}".

The backend overrides [`VfsAccess::read_range`] with a seek. [`Access::list`] sorts by name, and [`Access::glob`] walks without following links and sorts its results. [`Access::stat`] does not follow symlinks. Off Unix, [`Stat::mode`] is [`None`] and only [`FileType::File`], [`FileType::Directory`], and [`FileType::Symlink`] are reported. Writes, appends, copies, and renames create missing parent directories and are failure-atomic.

## Entry

[`Entry`] is one directory entry: a name within its directory plus that child's metadata. The host receives entries from [`Access::list`], and a backend returns them from [`VfsAccess::list`]. It is `#[non_exhaustive]` and has no public constructor, so no host code can build one, including a custom backend.

- [`Entry::name`], a [`String`], is the entry's name within its directory. It is a single segment, not a full path.
- [`Entry::stat`], a [`Stat`], is the entry's metadata. The host backend's listing does not follow symlinks.
- [`Entry::description`], an [`Option`] of [`String`], is an optional annotation shown beside the entry. Its documentation says it is [`None`] outside `/_promptforge`. The built-in backends always set it to [`None`].

## Stat

[`Stat`] is the metadata for one path. A field the backend does not track is [`None`] rather than invented. The host receives one from [`Access::stat`] and inside [`Entry::stat`]. It is `#[non_exhaustive]` and has no public constructor, so a custom backend implementing [`VfsAccess::stat`] cannot build one.

- [`Stat::file_type`], a [`FileType`], is what kind of node the path is.
- [`Stat::size`], a [`u64`], is the size in bytes. The memory backend reports 0 for a directory.
- [`Stat::mode`], an [`Option`] of [`u32`], holds the POSIX mode bits. It is [`Some`] from the host backend on Unix, and [`None`] on Windows and from the memory backend.
- [`Stat::modified`], an [`Option`] of [`SystemTime`](std::time::SystemTime), is the last modification time. It is [`None`] from the memory backend.
- [`Stat::created`], an [`Option`] of [`SystemTime`](std::time::SystemTime), is the creation time. It is [`None`] from the memory backend, and the host backend reports it when the platform does.

## FileType

[`FileType`] is the kind of a filesystem node, one of seven POSIX kinds, carried in [`Stat::file_type`]. The host receives it and can name variants to compare. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm.

- [`FileType::File`]: a regular file. The memory backend reports only this and [`FileType::Directory`]. The host backend off Unix reports every node that is neither a directory nor a symlink as a file, and on Unix falls back to it for an unrecognized kind.
- [`FileType::Directory`]: a directory. [`Access::list`] applies, and [`Access::read`] fails with [`VfsError::IsADirectory`].
- [`FileType::Symlink`]: a symbolic link. The host backend reports it because its stat and listing do not follow links.
- [`FileType::Fifo`]: a named pipe. Only the host backend on Unix reports it.
- [`FileType::Socket`]: a socket. Only the host backend on Unix reports it.
- [`FileType::CharDevice`]: a character device, such as `/dev/null`. Only the host backend on Unix reports it.
- [`FileType::BlockDevice`]: a block device. Only the host backend on Unix reports it.

## GrepQuery

[`GrepQuery`] is one content search, passed to [`Access::grep`] and [`VfsAccess::grep`]. It is `#[non_exhaustive]` and has no public constructor and no [`Default`], so a host cannot build one. A host holds one only when a backend's [`VfsAccess::grep`] receives it, and can clone that one and assign its public fields.

- [`GrepQuery::pattern`], a [`String`], is the text to search for. With the default body it is a literal substring matched within each line.
- [`GrepQuery::root`], a [`VfsPathBuf`], is the directory the search starts from. [`Access::grep`] canonicalizes it, and a router strips the mount prefix before passing it to the backend.
- [`GrepQuery::is_regex`], a [`bool`], says whether the pattern is a regular expression. The default body fails with [`VfsError::Unsupported`] when it is `true`, so only an overriding backend can serve regex.
- [`GrepQuery::case_insensitive`], a [`bool`], says whether matching ignores case. The default body lowercases both the line and the pattern.
- [`GrepQuery::glob_filter`], an [`Option`] of [`String`], restricts which files are searched. The default body globs `{root}/**/{filter}`, or `{root}/**/*` when it is [`None`].
- [`GrepQuery::max_results`], an [`Option`] of [`usize`], caps the returned matches. [`None`] means no cap. When the cap is reached and another match turns up, the default body stops and sets [`GrepResults::truncated`].

## GrepResults

[`GrepResults`] is the outcome of one search: the hits and whether the cap cut them short. The host receives it from [`Access::grep`]. It is `#[non_exhaustive]`, but [`GrepResults::default`] builds an empty value with no matches and [`GrepResults::truncated`] set to `false`, and its public fields can then be assigned. That makes it the one search type a custom backend can build.

- [`GrepResults::matches`], a [`Vec`] of [`GrepMatch`], holds the hits in backend order. The default body walks files in the glob's sorted order and lines in file order.
- [`GrepResults::truncated`], a [`bool`], is `true` when [`GrepQuery::max_results`] cut the results short, and `false` when every match fit.

## GrepMatch

[`GrepMatch`] is one search hit. The host receives it inside [`GrepResults::matches`]. It is `#[non_exhaustive]` and has no public constructor.

- [`GrepMatch::path`], a [`String`], is the path of the file containing the hit. Through a router, the mount prefix is joined back on, so it is the full virtual path.
- [`GrepMatch::line_number`], a [`usize`], is the 1-based line number of the hit.
- [`GrepMatch::line`], a [`String`], is the full text of the matching line, without its line terminator.

## VfsPath

[`VfsPath`] is a canonical virtual path: rooted, separated by `/`, with no `.` or `..` segments and no duplicate or trailing slashes. Policies receive it in [`Policy::check`], backends receive it in every [`VfsAccess`] method relative to their mount, and the op sink receives it from [`OpEvent::path`]. Hosts never build one, because canonicalization is its only constructor and it is private. Its [`Display`](std::fmt::Display) form writes the canonical string, so `format!("{path}")` works.

- [`VfsPath::as_str`] returns the canonical path as a [`&str`](str), for example `"/a/b"` or `"/"`.
- [`VfsPath::to_buf`] returns an owned [`VfsPathBuf`] copy, for a value that must outlive the borrow, such as a [`VfsAccess::read_link`] result.

## VfsPathBuf

[`VfsPathBuf`] is an owned canonical virtual path, used where a path must be owned, such as [`GrepQuery::root`] or a symlink target. Build one with [`VfsPath::to_buf`] or with its [`From`] conversion from a [`VfsPath`]. Its field is private, so it cannot be built from an arbitrary string. Its [`Display`](std::fmt::Display) form writes the canonical string.

- [`VfsPathBuf::as_str`] returns the canonical path as a [`&str`](str).

## Policy

[`Policy`] is the per-handle hook that decides whether each operation may proceed. A host implements it, and installs it with [`VfsRef::with_policy`] or [`VfsRefBuilder::policy`]. The trait requires [`Send`], and both installers also require [`Sync`] and `'static`. The crate implements it for [`AllowAll`] and [`ModePolicy`].

[`Policy::check`] is the one required method. It takes `&self`, `op`, the [`Op`] being attempted, and `path`, a reference to the canonical [`VfsPath`] in the handle's namespace. For a glob the path is the canonicalized pattern, and for a grep it is the canonical root. A rename or copy calls it once per path. It returns a [`Verdict`], and the verdict is the whole outcome, so it has no failure of its own. A refused operation registers no claim and fires no op event.

## Verdict

[`Verdict`] is a policy's answer for one operation. [`Policy::check`] returns it, and the host builds a variant directly, for example `Verdict::Deny(format!("{op:?} on {path} is refused: write under /drafts instead"))`. The text in a refusal matters in both directions: a [`Verdict::Deny`] text goes back to the model, and a [`Verdict::Ask`] text goes to the user.

- [`Verdict::Allow`]: the operation proceeds to the claims check and the backend.
- [`Verdict::Deny`] holds a [`String`]: the operation is refused, and the text is the model's recovery hint, so say how to recover. The caller receives [`VfsError::PermissionDenied`] with that text.
- [`Verdict::Ask`] holds a [`String`]: the operation needs user approval, and the text is the dialog text, naming what is being asked and which rule fired. At this layer it fails exactly as [`Verdict::Deny`] does, and the approval flow belongs to the host above.

## Op

[`Op`] is the kind of operation being attempted. A policy receives it in [`Policy::check`], and the op sink reads it from [`OpEvent::op`]. Name variants directly when matching. It is not `#[non_exhaustive]`, so a `match` can list every variant. Its [`Debug`](std::fmt::Debug) form, such as `Write`, appears in [`ModePolicy`]'s refusal texts.

- [`Op::Read`]: reading a file's bytes. [`Access::read`], [`Access::read_string`], [`Access::read_range`], and [`Access::read_range_numbered`] issue it. It only looks.
- [`Op::Write`]: creating or overwriting a file. [`Access::write`] and [`Access::str_replace`] issue it. It is a mutation.
- [`Op::Append`]: appending to a file, from [`Access::append`]. It is a mutation.
- [`Op::Delete`]: removing a file, link, or directory, from [`Access::remove`]. It is a mutation.
- [`Op::Rename`]: renaming or moving a path, from [`Access::rename`], checked once for each of its two paths. It is a mutation.
- [`Op::Mkdir`]: creating a directory, from [`Access::mkdir`]. It is a mutation.
- [`Op::Copy`]: copying a file, from [`Access::copy`], checked once for the source and once for the destination. A policy cannot tell the two checks apart. It is a mutation.
- [`Op::Grep`]: searching file contents, from [`Access::grep`], with the canonical root as the path. It only looks.
- [`Op::Exists`]: testing for existence, from [`Access::exists`]. It only looks.
- [`Op::Glob`]: matching paths against a pattern, from [`Access::glob`], with the canonicalized pattern as the path. It only looks.
- [`Op::List`]: listing a directory, from [`Access::list`]. It only looks.
- [`Op::Stat`]: reading metadata, from [`Access::stat`]. It only looks.
- [`Op::Symlink`]: creating a symbolic link. No [`Access`] method issues it in this version. [`ModePolicy`] treats it as a mutation.
- [`Op::ReadLink`]: reading a symbolic link's target. No [`Access`] method issues it in this version. It only looks.
- [`Op::Chmod`]: changing mode bits. No [`Access`] method issues it in this version. [`ModePolicy`] treats it as a mutation.

## AllowAll

[`AllowAll`] is the policy that allows every operation: its [`Policy::check`] always returns [`Verdict::Allow`]. It is the default for [`VfsRef::new`] and for a builder with no [`VfsRefBuilder::policy`] call. It is a unit struct, so the value is just [`AllowAll`], and [`AllowAll::default`] returns the same value.

## ModePolicy

[`ModePolicy`] is the editor mode gate. It never gates reads. For a mutation it asks for approval, allows only markdown paths, or allows everything, according to its current [`Mode`]. [Policies](#policies) lists the operations it treats as mutations.

[`ModePolicy::new`] takes `mode`, the starting [`Mode`], and returns a policy that holds it in a fresh shared cell. It cannot fail. Install the policy with [`VfsRef::with_policy`] or [`VfsRefBuilder::policy`].

[`ModePolicy::handle`] takes `&self` and returns a [`ModeHandle`] that shares the policy's mode cell. It cannot fail. Call it before installing the policy if the UI needs to flip modes. Whether a mode change is one-way or reversible depends only on who still holds a handle.

For a mutation, [`Mode::Agent`] returns [`Verdict::Allow`]. [`Mode::Ask`] returns [`Verdict::Ask`] with "{op:?} on {path} needs user approval: the Ask mode refuses all mutations". [`Mode::Plan`] returns [`Verdict::Allow`] when the canonical path ends in `.md`, and otherwise [`Verdict::Deny`] with "{op:?} on {path} is refused: the Plan mode allows mutations only to markdown paths".

## Mode

[`Mode`] is the editor mode a [`ModePolicy`] enforces: what the model may change right now. Name a variant directly, pass it to [`ModePolicy::new`] or [`ModeHandle::set`], and read it back from [`ModeHandle::mode`]. It has no [`Default`].

- [`Mode::Ask`]: every mutation needs user approval, and reads work. A mutation fails with [`VfsError::PermissionDenied`] carrying the approval text. The host's approval flow sits above this layer.
- [`Mode::Plan`]: mutations are allowed only to paths ending in `.md`, checked case-sensitively, and reads work. A mutation anywhere else fails with [`VfsError::PermissionDenied`] carrying the refusal text.
- [`Mode::Agent`]: every operation is allowed.

## ModeHandle

[`ModeHandle`] is the UI's side of the editor mode gate: a shared cell holding one [`ModePolicy`]'s current [`Mode`]. Get one from [`ModePolicy::handle`]. Clones share the one cell.

- [`ModeHandle::set`] takes `&self` and `mode`, the new [`Mode`], and returns nothing. It cannot fail. The next operation through any handle using the policy sees the new mode.
- [`ModeHandle::mode`] takes `&self` and returns the current [`Mode`]. It cannot fail.

## OpEvent

[`OpEvent`] describes one admitted operation, handed to the op sink installed with [`VfsRefBuilder::on_op`]. It borrows the access's own values, so firing it allocates nothing. Hosts never build one, and its fields are private, so read it through its accessors. None of them can fail.

- [`OpEvent::op`] returns the [`Op`], by value.
- [`OpEvent::path`] returns a reference to the canonical [`VfsPath`] the operation acts on. For a glob it is the canonicalized pattern, and for a grep the canonicalized root. A rename or copy fires one event per path. A sink that keeps events copies the path out, for example with `event.path().to_string()`.
- [`OpEvent::origin`] returns a reference to the [`Origin`] that was passed to [`VfsRef::acquire`] for the access that admitted the operation.

## OpSink

[`OpSink`] is the type of an installed op sink: an [`Arc`](std::sync::Arc) of a [`Fn`] that takes one [`OpEvent`] per admitted operation and is [`Send`] and [`Sync`]. Hosts normally pass a closure to [`VfsRefBuilder::on_op`], which wraps it in this type. [`VfsRefBuilder::on_op`] takes the closure itself, not an [`OpSink`] value, so a host names the alias only to store such a callback of its own. The sink must be cheap, because it runs inline with each operation.

## perform_store_op

[`perform_store_op`] performs one store operation through an [`Access`], which is the work behind an [`Effect::Store`](crate::effect::Effect::Store). A host that calls it answers a store effect exactly as the engine's own drivers do. It takes two arguments, both from the same effect.

- `access`, a reference to an [`Access`], is the capability from the effect's [`access`](crate::effect::Effect#variant.Store.field.access) field. Pass it as received.
- `op`, a [`StoreOp`], is the validated operation from the effect's [`op`](crate::effect::Effect#variant.Store.field.op) field.

It returns a [`Result`] of a [`StoreOutcome`] or a [`StoreError`]. The outcome is [`StoreOutcome::Unit`] for [`StoreOp::Write`], [`StoreOp::Append`], [`StoreOp::StrReplace`], and [`StoreOp::Delete`], [`StoreOutcome::Text`] for [`StoreOp::Read`] and [`StoreOp::ReadNumbered`], [`StoreOutcome::Paths`] for [`StoreOp::Glob`], and [`StoreOutcome::Bool`] for [`StoreOp::Exists`]. Wrap the whole [`Result`] in [`EffectAnswer::Store`](crate::effect::EffectAnswer::Store) and pass it to [`Run::resume`](crate::Run::resume).

It is synchronous, so an async host runs it off its executor and drops the access after it returns, before resuming. Logical paths are joined onto the store mount `/_promptforge/store`. [`StoreOp::Delete`] of a missing file succeeds. [`StoreOp::Glob`] lists only files, as logical paths, and stats each match, which takes a read claim on each matched file.

## StoreOp

[`StoreOp`] is one validated store operation: the name of a prompt's `store.*` call plus the author's arguments. The host receives it inside an [`Effect::Store`](crate::effect::Effect::Store) and runs it with [`perform_store_op`]. A host can also build a variant directly, such as `StoreOp::Read { path: "missing.txt".to_owned(), start: None, end: None }`, or deserialize one. The enum is `#[non_exhaustive]`, so a `match` needs a wildcard arm.

Every path field is a logical path relative to the run's store mount, validated against the [`PathReason`] rules before dispatch.

- [`StoreOp::Write`] is `store.write(path, contents)`. It creates or overwrites the file.
  - [`StoreOp::Write::path`](StoreOp#variant.Write.field.path), a [`String`], is the logical path.
  - [`StoreOp::Write::contents`](StoreOp#variant.Write.field.contents), a [`String`], is the complete new file text, written as its UTF-8 bytes.
- [`StoreOp::Append`] is `store.append(path, contents)`. It appends, creating the file when it is absent.
  - [`StoreOp::Append::path`](StoreOp#variant.Append.field.path), a [`String`], is the logical path.
  - [`StoreOp::Append::contents`](StoreOp#variant.Append.field.contents), a [`String`], is the text to append.
- [`StoreOp::Read`] is `store.read(path, start?, end?)`. With no `start` and no `end` it reads the whole file verbatim. With a `start` it returns a 1-based, inclusive line range joined by `"\n"` with no trailing newline.
  - [`StoreOp::Read::path`](StoreOp#variant.Read.field.path), a [`String`], is the logical path of a UTF-8 file.
  - [`StoreOp::Read::start`](StoreOp#variant.Read.field.start), an [`Option`] of [`i64`], is the first line. A value below 1 fails with [`StoreError::InvalidRange`] and "start must be at least 1", and a negative value counts as 0. A `start` past the last line gives `""`.
  - [`StoreOp::Read::end`](StoreOp#variant.Read.field.end), an [`Option`] of [`i64`], is the last line, inclusive. [`None`] means the last line, and a value past the last line clamps to it. An `end` without a `start` fails with "start is required when end is given", and an `end` before `start` fails with "end must not be before start".
- [`StoreOp::ReadNumbered`] is `store.read_numbered(path, start?, end?)`: the same read with absolute line numbers, right-aligned and followed by `"| "`. With no bounds it numbers the whole file from 1.
  - [`StoreOp::ReadNumbered::path`](StoreOp#variant.ReadNumbered.field.path), a [`String`], is the logical path.
  - [`StoreOp::ReadNumbered::start`](StoreOp#variant.ReadNumbered.field.start), an [`Option`] of [`i64`], follows the rules of [`StoreOp::Read::start`](StoreOp#variant.Read.field.start).
  - [`StoreOp::ReadNumbered::end`](StoreOp#variant.ReadNumbered.field.end), an [`Option`] of [`i64`], follows the rules of [`StoreOp::Read::end`](StoreOp#variant.Read.field.end).
- [`StoreOp::StrReplace`] is `store.str_replace(path, old, new)`. It replaces the one occurrence of the anchor.
  - [`StoreOp::StrReplace::path`](StoreOp#variant.StrReplace.field.path), a [`String`], is the logical path.
  - [`StoreOp::StrReplace::old`](StoreOp#variant.StrReplace.field.old), a [`String`], is the anchor text, which must occur exactly once. An empty anchor fails with [`StoreError::InvalidAnchor`], no match with [`StoreError::AnchorNotFound`], and more than one match with [`StoreError::AnchorAmbiguous`].
  - [`StoreOp::StrReplace::new`](StoreOp#variant.StrReplace.field.new), a [`String`], is the replacement text, and may be empty.
- [`StoreOp::Delete`] is `store.delete(path)`. It removes the file, and a missing file succeeds.
  - [`StoreOp::Delete::path`](StoreOp#variant.Delete.field.path), a [`String`], is the logical path. The removal is not recursive, so a path that is a directory with children fails as a backend error.
- [`StoreOp::Glob`] is `store.glob(pattern)`. It lists the stored files that match.
  - [`StoreOp::Glob::pattern`](StoreOp#variant.Glob.field.pattern), a [`String`], is a glob relative to the store mount, with `*` within one segment and `**` across segments as a whole segment. It must be non-empty, at most 1024 bytes, and free of control characters and backslashes. Only files are listed, as logical paths.
- [`StoreOp::Exists`] is `store.exists(path)`. It tests whether the path exists.
  - [`StoreOp::Exists::path`](StoreOp#variant.Exists.field.path), a [`String`], is the logical path.

The read bounds are [`i64`] for compatibility with the prompt-facing call. [`StoreOp`] implements serde's [`Serialize`](https://docs.rs/serde/latest/serde/trait.Serialize.html) and [`Deserialize`](https://docs.rs/serde/latest/serde/trait.Deserialize.html) with no serde attributes, so it uses serde's default externally tagged form: the variant name is the key and the fields are an object, as in `{"Delete":{"path":"notes.md"}}`.

## StoreOutcome

[`StoreOutcome`] is the successful result of one store operation, which the prompt's `store.*` call returns. [`perform_store_op`] returns it. The enum is not `#[non_exhaustive]`, so a host with its own store performer can build a variant directly, such as `StoreOutcome::Bool(true)`.

- [`StoreOutcome::Unit`]: the operation succeeded with no value, which the prompt sees as nil. It answers [`StoreOp::Write`], [`StoreOp::Append`], [`StoreOp::StrReplace`], and [`StoreOp::Delete`].
- [`StoreOutcome::Text`] holds a [`String`]: the file text, possibly limited to a line range and possibly numbered. It answers [`StoreOp::Read`] and [`StoreOp::ReadNumbered`].
- [`StoreOutcome::Paths`] holds a [`Vec`] of [`String`]: the matching logical file paths, sorted. It answers [`StoreOp::Glob`].
- [`StoreOutcome::Bool`] holds a [`bool`]: whether the path exists. It answers [`StoreOp::Exists`].

## StoreError

[`StoreError`] is the failure of one store operation. [`perform_store_op`] returns it, and the answer carries it back to the run, which raises it at the prompt's call site. [`StoreError::backend`] is the only public constructor. The enum and every variant with fields are `#[non_exhaustive]`, so a pattern on a variant needs `..`. Its documentation directs hosts to match on [`StoreError::kind`] rather than on variants. Every `path` field below is the logical path exactly as the caller supplied it, relative to the store mount.

- [`StoreError::NotFound`]: no file exists at the path. [`StoreOp::Read`], [`StoreOp::ReadNumbered`], and [`StoreOp::StrReplace`] return it for a missing file. Pass it back as the answer, and the author sees it at the call site.
  - [`StoreError::NotFound::path`](StoreError#variant.NotFound.field.path), a [`String`], is the path that did not resolve.
- [`StoreError::InvalidAnchor`]: a [`StoreOp::StrReplace`] anchor was refused before any search. The only cause is an empty anchor.
  - [`StoreError::InvalidAnchor::path`](StoreError#variant.InvalidAnchor.field.path), a [`String`], is the path the edit targeted.
  - [`StoreError::InvalidAnchor::reason`](StoreError#variant.InvalidAnchor.field.reason), a [`&'static str`](str), is the reason, currently always "anchor must not be empty".
- [`StoreError::AnchorNotFound`]: the anchor did not occur in the file, and nothing was written.
  - [`StoreError::AnchorNotFound::path`](StoreError#variant.AnchorNotFound.field.path), a [`String`], is the path that was searched.
  - [`StoreError::AnchorNotFound::anchor`](StoreError#variant.AnchorNotFound.field.anchor), a [`String`], is the anchor text.
- [`StoreError::AnchorAmbiguous`]: the anchor occurred more than once, so the edit was refused rather than applied to an arbitrary match.
  - [`StoreError::AnchorAmbiguous::path`](StoreError#variant.AnchorAmbiguous.field.path), a [`String`], is the path that was searched.
  - [`StoreError::AnchorAmbiguous::anchor`](StoreError#variant.AnchorAmbiguous.field.anchor), a [`String`], is the anchor text.
  - [`StoreError::AnchorAmbiguous::count`](StoreError#variant.AnchorAmbiguous.field.count), a [`usize`], is the number of non-overlapping matches, always 2 or more.
- [`StoreError::InvalidPath`]: a path failed validation before any backend saw it.
  - [`StoreError::InvalidPath::path`](StoreError#variant.InvalidPath.field.path), a [`String`], is the rejected path.
  - [`StoreError::InvalidPath::reason`](StoreError#variant.InvalidPath.field.reason), a [`PathReason`], is the rule it broke.
- [`StoreError::InvalidPattern`]: a [`StoreOp::Glob`] pattern was rejected before matching.
  - [`StoreError::InvalidPattern::pattern`](StoreError#variant.InvalidPattern.field.pattern), a [`String`], is the rejected pattern as supplied, without the store mount prefix.
  - [`StoreError::InvalidPattern::reason`](StoreError#variant.InvalidPattern.field.reason), a [`String`], is "pattern is empty", "pattern exceeds 1024 bytes", "pattern contains a control character", "pattern does not support backslash escapes", or the [`VfsError::InvalidPath`] message for a grammar or canonicalization failure.
- [`StoreError::InvalidRange`]: a [`StoreOp::Read`] or [`StoreOp::ReadNumbered`] line range was rejected.
  - [`StoreError::InvalidRange::path`](StoreError#variant.InvalidRange.field.path), a [`String`], is the path the read targeted.
  - [`StoreError::InvalidRange::reason`](StoreError#variant.InvalidRange.field.reason), a [`&'static str`](str), is "start must be at least 1", "end must not be before start", or "start is required when end is given".
- [`StoreError::WriteRace`]: two live identities touched the same path, and the losing operation never reached the backend. It ends the run with [`RunErrorKind::Determinism`](crate::RunErrorKind::Determinism), which Lua cannot catch.
  - [`StoreError::WriteRace::path`](StoreError#variant.WriteRace.field.path), a [`String`], is the path the conflicting operation targeted.
  - [`StoreError::WriteRace::detail`](StoreError#variant.WriteRace.field.detail), a [`String`], is the [`VfsError::Conflict`] message, naming the canonical virtual path, both identities, and both claim kinds.
- [`StoreError::Backend`]: the backend failed for a reason of its own. Every filesystem error other than not-found and conflict lands here, including a policy denial, a read-only mount, and text that is not UTF-8.
  - [`StoreError::Backend::source`](StoreError#variant.Backend.field.source), a [`Box`] of a [`std::error::Error`] that is [`Send`] and [`Sync`], is the backend's own error. For a filesystem failure it is the [`VfsError`], so a host can downcast it to inspect it.

The methods classify and inspect an error. None of them can fail.

- [`StoreError::kind`] returns the stable [`StoreErrorKind`]. [`StoreError::AnchorNotFound`] and [`StoreError::AnchorAmbiguous`] both map to [`StoreErrorKind::Anchor`], and every other variant maps to the kind of the same name.
- [`StoreError::is_not_found`] returns `true` only for [`StoreError::NotFound`].
- [`StoreError::path`] returns the logical path as an [`Option`] of [`&str`](str), for example `Some("missing.txt")`. It is [`None`] for [`StoreError::InvalidPattern`] and [`StoreError::Backend`].
- [`StoreError::conflict_detail`] returns the full conflict diagnosis from a [`StoreError::WriteRace`] as an [`Option`] of [`&str`](str), and [`None`] for every other variant. The engine passes it verbatim into the run's determinism failure.
- [`StoreError::backend`] takes `source`, any [`std::error::Error`] that is [`Send`], [`Sync`], and `'static`, and returns a [`StoreError::Backend`] holding it boxed. Use it when a host store performer fails for its own reasons, or to convert a [`VfsError`] into a [`StoreError`].

[`StoreError`] implements [`std::error::Error`], and [`StoreError::Backend`] reports its source through [`source`](std::error::Error::source). It is not [`Clone`]. Its [`Display`](std::fmt::Display) texts are lowercase with no trailing period:

- [`StoreError::NotFound`]: "file not found: {path}"
- [`StoreError::InvalidAnchor`]: "invalid anchor for {path}: {reason}"
- [`StoreError::AnchorNotFound`]: "anchor not found in {path}"
- [`StoreError::AnchorAmbiguous`]: "anchor occurs {count} times in {path}, expected exactly one"
- [`StoreError::InvalidPath`]: "invalid path {path:?}: {reason}"
- [`StoreError::InvalidPattern`]: "invalid glob pattern {pattern:?}: {reason}"
- [`StoreError::InvalidRange`]: "invalid line range for {path}: {reason}"
- [`StoreError::WriteRace`]: "write-write race on {path}: another live identity holds a claim on it"
- [`StoreError::Backend`]: "store backend failure"

## StoreErrorKind

[`StoreErrorKind`] is the stable, matchable classification of a [`StoreError`], returned by [`StoreError::kind`]. It is `#[non_exhaustive]`, so a `match` needs a wildcard arm, and new causes can be added without breaking it. It has no [`Display`](std::fmt::Display).

- [`StoreErrorKind::NotFound`]: no file exists at the path.
- [`StoreErrorKind::Anchor`]: a `store.str_replace` anchor did not occur, or occurred more than once.
- [`StoreErrorKind::InvalidAnchor`]: a `store.str_replace` anchor was empty and refused before any search.
- [`StoreErrorKind::InvalidPath`]: a path failed validation.
- [`StoreErrorKind::InvalidPattern`]: a glob pattern failed validation.
- [`StoreErrorKind::InvalidRange`]: a line range failed validation.
- [`StoreErrorKind::WriteRace`]: two live identities touched the same path.
- [`StoreErrorKind::Backend`]: the backend itself failed, including policy denials and read-only mounts surfaced through the store.

## PathReason

[`PathReason`] says why a logical store path was rejected before any backend saw it. It arrives in [`StoreError::InvalidPath::reason`](StoreError#variant.InvalidPath.field.reason), and hosts never build one, though they can name variants to compare. It is `#[non_exhaustive]`. [The run's store](#the-runs-store) gives the order the rules are checked in. The fix in every case is to supply a path that follows the rule.

- [`PathReason::Empty`]: the path was the empty string. Its documentation also claims a path made only of separators, but the leading `/` check runs first, so such a path reports [`PathReason::Absolute`].
- [`PathReason::Absolute`]: the path began with `/`. Store paths are relative to the store mount, so drop the leading slash.
- [`PathReason::Traversal`]: a segment was `.` or `..`.
- [`PathReason::Control`]: the path contained a byte below `0x20` or equal to `0x7f`.
- [`PathReason::EmptySegment`]: the path contained an empty segment, from a `//` run or a trailing `/`.
- [`PathReason::Backslash`]: the path contained `\`, which is a separator on some backends and a literal on others.
- [`PathReason::ReservedName`]: a segment was a platform-reserved device name. The base name before the first `.` is compared case-insensitively, so `con.txt` is rejected.
- [`PathReason::UnsafeSuffix`]: a segment ended in `.` or a space, which some backends silently strip, so the name would not round-trip.
- [`PathReason::TooLong`]: the path exceeded 1024 bytes.

Its [`Display`](std::fmt::Display) texts are "path is empty", "path is absolute", "path contains a traversal segment", "path contains a control character", "path contains an empty segment", "path contains a backslash", "path contains a reserved device name", "path segment ends in an unsafe character", and "path is too long".

## Vfs

[`Vfs`] is one backend behind the virtual namespace. The only way to reach its storage is to acquire a per-identity session, a [`VfsAccess`]. A host implements it for a custom backend. The trait requires [`Send`] but not [`Sync`], because the handle serializes access. The crate implements it for [`MemoryBackend`], [`HostBackend`], and [`VfsRef`]. Pass an implementation to [`VfsRef::new`], [`VfsRef::with_policy`], [`VfsRefBuilder::mount`], or [`VfsRef::overlay`], all of which require `'static`.

- [`Vfs::acquire`] is required. It takes `&mut self` and `id`, the [`ExecId`] that every operation on the new session is attributed to, and returns a [`Box`] of a [`VfsAccess`]. A backend that tracks who touches what keys on the id, and others ignore it. Return any [`VfsError`] when the backend cannot open a session, and the caller of [`VfsRef::acquire`] receives it, or the first operation on the mount through a router. It is called once per [`VfsRef::acquire`] for a directly wrapped backend, and lazily on first touch for a mounted one.
- [`Vfs::release`] is required. It takes `&mut self` and the `id` being released, and returns `()` on success. Return a [`VfsError`] when the identity cannot be released, though the caller ignores it, because the release runs when the [`Access`] is dropped, after its claims are gone. So cancellation, panics, and early returns cannot skip it. Through a router, it is called at each mount the identity touched, after that mount's session is dropped.
- [`Vfs::read_only`] has a default body that returns `false`. It takes `&self` and returns whether the backend rejects all mutations. On a read-only mount, a router refuses [`Access::write`], [`Access::append`], [`Access::remove`], [`Access::mkdir`], [`Access::str_replace`], the destination of [`Access::copy`], and either end of [`Access::rename`] before the backend is touched. A backend used alone through [`VfsRef::new`] must reject mutations itself.

## VfsAccess

[`VfsAccess`] is one identity's session with a backend, and it declares every filesystem operation. A host implements it for a custom backend and returns it boxed from [`Vfs::acquire`]. The trait requires [`Send`]. Hosts do not call it directly: [`Access`] calls it after the policy and claims pass. Every path argument is a reference to a [`VfsPath`] that arrives validated, canonical, and relative to the backend's mount, so a backend never checks it again.

**Required methods.**

- [`VfsAccess::read`] takes `path` and returns the file's bytes as a [`Vec`] of [`u8`]. Return [`VfsError::NotFound`] when the file is absent. The built-ins return [`VfsError::IsADirectory`] for a directory, and the default [`VfsAccess::grep`] skips files whose read fails that way.
- [`VfsAccess::write`] takes `&mut self`, `path`, and `contents`, a [`&[u8]`](slice) holding the complete new file, and returns `()` once the file is created or overwritten. The built-ins create missing ancestors, and the host backend writes failure-atomically.
- [`VfsAccess::append`] takes `&mut self`, `path`, and `contents`, the bytes to append, and returns `()`. It must create the file when it is absent.
- [`VfsAccess::remove`] takes `&mut self`, `path`, and `recursive`, a [`bool`] that says whether a directory's subtree is removed, and returns `()`. Return [`VfsError::NotFound`] when the path is absent, and an error for a directory without `recursive`. The built-ins use [`VfsError::DirectoryNotEmpty`] for a non-empty one. On a symlink, remove the link, never the target.
- [`VfsAccess::exists`] takes `path` and returns a [`bool`]. Return `false` only for a confirmed absence, and an error when existence cannot be determined.
- [`VfsAccess::glob`] takes `pattern`, a [`&str`](str), and returns the matching stored paths, sorted, as mount-relative virtual paths in a [`Vec`] of [`String`]. Through a router the pattern arrives canonicalized and mount-relative, and the router joins the mount prefix back onto each result. Through a directly wrapped backend it arrives as the caller passed it. Return an error for an invalid pattern. The built-ins use [`VfsError::InvalidPath`].
- [`VfsAccess::list`] takes `path` and returns the directory's entries as a [`Vec`] of [`Entry`]. Return an error when the path is not a directory. A custom backend cannot construct [`Entry`] values, so it can only return entries from another backend.
- [`VfsAccess::stat`] takes `path` and returns its [`Stat`]. Return [`VfsError::NotFound`] when the path is absent. A custom backend cannot construct a [`Stat`], so it can only return one from another backend.
- [`VfsAccess::mkdir`] takes `&mut self`, `path`, and `recursive`, a [`bool`] that says whether missing ancestors are created too, and returns `()`. The built-ins use [`VfsError::AlreadyExists`] for an existing path.
- [`VfsAccess::rename`] takes `&mut self`, `from`, and `to`, both on this mount, because the router guarantees one mount. It returns `()`, and on failure leaves both paths unchanged. Make it atomic where the backend allows.
- [`VfsAccess::copy`] takes `&mut self`, `from`, a source file, and `to`, on the same mount. It returns `()`, and on failure leaves both paths unchanged.

**Methods with default bodies.**

- [`VfsAccess::read_range`] takes `path`, `offset`, a [`u64`] starting byte, and `len`, a [`u64`] maximum byte count. It returns up to `len` bytes from `offset`, empty when `offset` is at or past the end, and clipped at the end of the file. The default body reads the whole file and slices it. It fails as [`VfsAccess::read`] does, and with [`VfsError::Backend`] and "read_range offset {offset} exceeds the addressable size" or "read_range length {len} exceeds the addressable size" when a value does not fit a [`usize`]. Override it to seek, as the host backend does. A router passes it to the mount. The trait's documentation says the handle's line-based ranges are built on this method, but [`Access::read_range`] reads the whole file through [`Access::read`] instead, and [`Access`] exposes no byte-range read.
- [`VfsAccess::str_replace`] takes `&mut self`, `path`, `old`, and `new`, and returns `()` after the rewritten file is written. The default body reads the file, counts matches of `old`, replaces the one occurrence, and writes the result. It fails with [`VfsError::Backend`] and "str_replace requires UTF-8 text: {path}: {source}", "str_replace found no occurrence of {old:?} in {path}", or "str_replace found {count} occurrences of {old:?} in {path}; exactly one is required", or with a read or write failure. A router checks the mount's read-only flag before passing it on.
- [`VfsAccess::grep`] takes `query`, a reference to a [`GrepQuery`] whose root is mount-relative through a router, and returns a [`GrepResults`]. The default body globs `{root}/**/{filter}`, where the filter is [`GrepQuery::glob_filter`] or `*`, and a root of `/` becomes the empty base. It skips directories and files that are not UTF-8, splits lines with [`str::lines`], and matches the pattern as a literal substring, lowercasing both sides when [`GrepQuery::case_insensitive`] is `true`. It stops when a match turns up after [`GrepQuery::max_results`] hits are already collected, and sets [`GrepResults::truncated`]. It fails with [`VfsError::Unsupported`] and "the default grep matches literal text only; regex requires a backend override" when [`GrepQuery::is_regex`] is `true`, and with errors from glob, from canonicalizing a globbed path, or from reads other than [`VfsError::IsADirectory`]. Override it for indexed or regex search.
- [`VfsAccess::symlink`] takes `&mut self`, `target`, the link's target, passed verbatim because a router neither strips nor resolves it, and `link`, the path of the link to create. The default body fails with [`VfsError::Unsupported`] and "symlink is not supported by this backend: {link}".
- [`VfsAccess::read_link`] takes `path`, a link, and returns its target as a [`VfsPathBuf`], which an override builds with [`VfsPath::to_buf`]. The default body fails with [`VfsError::Unsupported`] and "read_link is not supported by this backend: {path}".
- [`VfsAccess::chmod`] takes `&mut self`, `path`, and `mode`, a [`u32`] of POSIX mode bits such as `0o644`. The default body fails with [`VfsError::Unsupported`] and "chmod is not supported by this backend: {path}".

No [`Access`] method issues [`VfsAccess::symlink`], [`VfsAccess::read_link`], or [`VfsAccess::chmod`] in this version.
