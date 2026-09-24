The virtual filesystem a run's store lives in, and the host extension point behind it.

Every file a prompt reads or writes through its `store` table goes through one virtual namespace: absolute POSIX-style paths, served by backends mounted at prefixes. A host builds the namespace, hands it to the run, performs the run's store effects against it, and can plug in backends and policies of its own.

# Handles and access

A [`VfsRef`] is the cloneable handle over one namespace; clones share the backends, the policy, and the claims ledger. [`VfsRef::acquire`] is the only way in: it vends an [`Access`], the capability every operation goes through, bound to a fresh [`ExecId`] and labeled with an [`Origin`] for observability. Each operation canonicalizes its path (an escape past the root fails with [`VfsError::InvalidPath`]), consults the policy, checks the claims, and then calls the backend. Two live identities touching one path conflict with [`VfsError::Conflict`]; dropping an [`Access`] releases its identity and its claims, so cancellation, panics, and early returns cannot leak them. Every failure is a [`VfsError`].

A [`MemoryBackend`] keeps its files in memory, and its clones share the same storage:

```
use promptforge::vfs::{MemoryBackend, Origin, VfsRef};

let vfs = VfsRef::new(MemoryBackend::new());
let access = vfs.acquire(Origin::new("memory backend example"))?;
access.write("/notes.md", b"todo")?;
assert_eq!(access.read("/notes.md")?, b"todo");
# Ok::<(), promptforge::vfs::VfsError>(())
```

A [`HostBackend`] serves host directories: [`HostBackend::identity`] maps virtual paths straight to host paths, and [`HostBackend::rooted`] confines them under one directory, chroot-style. Its writes, copies, and renames are failure-atomic.

# Mounting

[`VfsRef::builder`] returns a [`VfsRefBuilder`]. [`mount`](VfsRefBuilder::mount) installs a backend at a prefix, and the longest matching prefix serves each path, so a longer mount shadows the same prefix of a shorter one; each backend sees paths relative to its own mount. Mounts are fixed at [`build`](VfsRefBuilder::build), so the table is immutable and cheap to share. The builder also installs the handle's [`policy`](VfsRefBuilder::policy) and its operation observer ([`on_op`](VfsRefBuilder::on_op)).

[`VfsRef::overlay`] returns a new handle with one more backend mounted at a prefix over an existing handle's namespace. The overlay shares the base's claims table, so conflicts are caught across both views; that is right only for two views of the same storage. The base does not see the overlay's mount:

```
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
# Ok::<(), VfsError>(())
```

Operations that name two paths, a rename or a copy, must stay within one mount: backend atomicity stops at the mount boundary, so crossing mounts fails with [`VfsError::Unsupported`].

# The run's store

One run-scoped handle, set on the [`RunContext`](crate::RunContext), is shared by every section, so store state persists across sections even though a section's Lua state never does. The run's store is a mount inside that handle's namespace, which the prompt's `store` table scopes to.

- [`RunContext::new`](crate::RunContext::new) starts with the stock handle: a fresh memory backend at the store mount and nothing else.
- [`Environment::base_vfs`](crate::Environment::base_vfs) sets the host roots every run shares, and [`Environment::prepare`](crate::Environment::prepare) gives each run [`Environment::run_vfs`](crate::Environment::run_vfs): a fresh router with the base mounted at `/` and a fresh memory store for the run. It is a router rather than an overlay because concurrent runs' stores are different storage; the base's own claims table still catches two runs conflicting on one host file.
- A host that activates capabilities before prepare builds the run's handle first with [`Environment::run_vfs`](crate::Environment::run_vfs), hands it to the capabilities, and sets it with [`RunContext::vfs`](crate::RunContext::vfs); prepare then keeps it, and the capabilities and the run share one store.
- After prepare, [`RunContext::vfs_handle`](crate::RunContext::vfs_handle) is the handle a host seeds before the run and extracts output from after it.

# Performing a store effect

A store operation reaches the host as an [`Effect::Store`](crate::effect::Effect::Store) holding the chain's [`Access`] and the validated [`StoreOp`]. [`perform_store_op`] runs it exactly as the engine's own drivers do, and its [`StoreOutcome`] or [`StoreError`] is the [`EffectAnswer::Store`](crate::effect::EffectAnswer::Store). The call is synchronous, because the filesystem is synchronous by design, so an async host runs it off its executor. The host uses the capability as given and never derives, widens, or retains store scope from it. A [`StoreError`] is classified by [`StoreErrorKind`], and a store path rejected before any backend saw it names its [`PathReason`].

# What a policy gates

A [`Policy`] is consulted on every operation, before the claims check, with the [`Op`] and the canonical path, and answers with a [`Verdict`]. [`Verdict::Allow`] lets the operation proceed. [`Verdict::Deny`] refuses it, and its reason flows back to the model as the tool error, so it should say how to recover. [`Verdict::Ask`] means the operation needs user approval; its reason is the dialog text, and at this layer it fails with [`VfsError::PermissionDenied`], leaving the approval flow to the host above. A refused operation registers no claim and fires no observer. A policy is dynamic through shared state, so a host can change its behavior mid-run and the next operation sees the change.

The default policy is [`AllowAll`]. [`ModePolicy`] is the editor mode gate: its [`Mode`] refuses every mutation pending approval ([`Mode::Ask`]), allows mutations only to markdown paths ([`Mode::Plan`]), or allows everything ([`Mode::Agent`]), never gating reads, and the UI flips it mid-run through its [`ModeHandle`].

```
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
# Ok::<(), VfsError>(())
```

# Read-only mounts

A backend whose [`Vfs::read_only`] returns true is a read-only mount. The router rejects every mutation on it with [`VfsError::PermissionDenied`] before the backend is touched, so a refused operation never partly applies, while reads flow as usual; a copy is refused when its destination is read-only, and a rename when either end is. [`HostBackend::with_read_only`] makes a host directory read-only. The router enforces the flag for mounts installed through [`VfsRefBuilder::mount`] or [`VfsRef::overlay`]; a backend used on its own through [`VfsRef::new`] rejects mutations itself if it must, as [`HostBackend`] does.

```
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
# Ok::<(), VfsError>(())
```

# Implementing a backend

A backend implements two traits. [`Vfs`] is the backend itself: [`acquire`](Vfs::acquire) opens a session for one [`ExecId`] and [`release`](Vfs::release) ends it, and [`read_only`](Vfs::read_only) defaults to false. It must be `Send` but need not be `Sync`, because the handle serializes access. [`VfsAccess`] is one identity's session and declares every filesystem operation. Paths arrive validated, canonical, and relative to the backend's mount, so a backend never re-validates them.

- Required methods: [`read`](VfsAccess::read), [`write`](VfsAccess::write), [`append`](VfsAccess::append), [`remove`](VfsAccess::remove), [`exists`](VfsAccess::exists), [`glob`](VfsAccess::glob), [`list`](VfsAccess::list), [`stat`](VfsAccess::stat), [`mkdir`](VfsAccess::mkdir), [`rename`](VfsAccess::rename), and [`copy`](VfsAccess::copy).
- Methods with default bodies: [`read_range`](VfsAccess::read_range) reads the whole file and slices it, for backends that cannot seek; [`str_replace`](VfsAccess::str_replace) reads, replaces the unique occurrence, and writes; [`grep`](VfsAccess::grep) globs, reads, and scans lines for literal text, returning [`VfsError::Unsupported`] for a regex query ([`GrepQuery`], [`GrepResults`], [`GrepMatch`]); and [`symlink`](VfsAccess::symlink), [`read_link`](VfsAccess::read_link), and [`chmod`](VfsAccess::chmod) return [`VfsError::Unsupported`]. A backend overrides any of them to push the work down.

Directory listings are [`Entry`] values with a [`FileType`], and metadata is a [`Stat`]. Paths in signatures are [`VfsPath`] and [`VfsPathBuf`].

Hosts implement [`Vfs`], [`VfsAccess`], and [`Policy`], so the three evolve compatibly: any method added to one of them later must have a default body, and adding one without a default is a breaking change to every host backend.

# Observing operations

[`VfsRefBuilder::on_op`] installs an [`OpSink`] that fires on every admitted operation - after the policy and the claims pass, before the backend executes - with an [`OpEvent`] naming the operation, the canonical path, and the caller's [`Origin`]. It is fire-and-forget: no outcome flows back, and a refused operation never fires. The sink must be cheap, because store operations fire it from the host's blocking pool.
