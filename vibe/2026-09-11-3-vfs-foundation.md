---
name: Promptforge Vfs Foundation
overview: "Focused foundation layer: a Vfs/VfsAccess trait pair with a claims model (ExecId-attributed access via an RAII Access capability; concurrent write conflicts are fatal), a mount router with universal cross-platform path handling that subsumes Bashkit's filesystem needs, a Policy layer with reason-carrying verdicts (AllowAll v1, modes in promptforge-vfs), a host filesystem backend, the Store rewritten as a public concrete facade (no trait) over Vfs mounted at /_promptforge/store with the Lua store table unchanged, and executor::run() taking VfsRef instead of StoreRef."
todos:
  - id: vfs-trait
    content: "Design the Vfs/VfsAccess trait pair and VfsRef handle: sync, bytes-based, Store-derived semantics plus stat/list/grep, acquire/release on the backend trait"
    status: pending
  - id: router
    content: "Build the mount router: builder-style mount installation, longest-prefix dispatch, lazy per-mount acquire, nestable, universal path canonicalization (Windows/macOS/Linux)"
    status: pending
  - id: mem-backend
    content: Implement the in-memory Vfs backend carrying former MemStore semantics
    status: pending
  - id: hostdir-backend
    content: "IN SCOPE (without it the Vfs is useless): host backend in shared-vfs (std::fs is std - the zero-dependency rule holds) with HostBackend::identity() and HostBackend::rooted(dir). Stage 1 thin: direct std::fs ops, lexical+canonicalize containment, failure-atomic writes (sibling temp + rename). Stage 2 hardening toward the Bashkit RealFs oracle (resolver trio, symlink policies, Windows long paths and device names) as the threat model demands"
    status: pending
  - id: policy
    content: "Implement the Policy layer: Op and Verdict (Deny/Ask carry reason strings), per-op check in Access before the claims check, AllowAll in shared-vfs, ModePolicy (Ask/Plan/Agent) in promptforge-vfs with a UI-flippable shared mode"
    status: pending
  - id: store-rewrite
    content: Rewrite Store as a public concrete facade (no trait) over Vfs mounted at /_promptforge/store; Lua store table unchanged; WriteScope registry deleted in favor of the claims model; hosts seed/extract declared input/output keys through it
    status: pending
  - id: executor-api
    content: Change executor::run() to take VfsRef instead of StoreRef; RunContext builds the Store facade internally
    status: pending
  - id: store-yield
    content: Make Lua store operations leaf yields in the coroutine protocol (new Request/Answer variants, one dispatch arm, spawn_blocking over the sync Vfs); uniform for all backends - no inline fast path
    status: pending
  - id: bashkit-adapter
    content: "SPIKE (deliverable is evidence, not integration): implement the Bashkit FsBackend adapter over VfsRef as a path dependency against the local clone at bashkit/, compile-check, and smoke-test an ls/cat/grep script against mounted backends. Toolchain already verified compatible: workspace runs stable 1.98, Bashkit's 1.95 pin applies only inside its own repo. Success proves the trait subsumes Bashkit; a mapping failure here is the spike working as intended, cheaply"
    status: pending
  - id: claims
    content: "Implement the claims model: ExecId vending, Access RAII capability (acquire/spawn/borrow/move/drop), readers/writers claims tables keyed by interned canonical paths, canonicalize (stub-to-intern acceptable in v1), fatal determinism RunErrorKind, scheduler installation of the current access per chain step"
    status: pending
  - id: tests
    content: Port the promptforge-store test suite onto the rewritten Store; add router and path-canonicalization matrices
    status: pending
isProject: false
---

# Promptforge Vfs Foundation

<product-contract>

## Product Requirements

One filesystem abstraction replaces the run-scoped Store trait and serves every future consumer: Lua, the model's tools, and the Bashkit engine. The Store survives as a public concrete facade with an unchanged Lua surface, mounted inside the namespace it used to stand apart from. The executor's public API pivots from StoreRef to VfsRef. Everything outside this foundation layer is explicitly deferred.

- Problem and users: the harness needs one filesystem abstraction. Today the Store is a run-scoped, text-only trait; Bashkit needs a filesystem backend; run records and terminals need a virtual namespace; Windows path handling is a documented model-failure source. Users are promptforge authors and, downstream, the model inside every run.
- Goals:
  - One `Vfs` trait plus a cloneable `VfsRef` handle, shaped like the proven Store/StoreRef pattern.
  - The trait subsumes Bashkit's filesystem needs so the adapter is mechanical.
  - A router installs multiple overlays into one namespace and behaves identically on Windows, macOS, and Linux.
  - The Store becomes a public concrete facade (no trait) over Vfs, mounted at `/_promptforge/store`; the Lua `store` table is behaviorally unchanged, and hosts seed declared inputs and extract declared outputs through it.
  - `executor::run()` takes `VfsRef` in place of `StoreRef`.
- Non-goals: the do_shell dispatcher, git builtins, approval policy, the SQLite run-record backend, terminal mirrors, and the Bashkit integration itself (adapter readiness only).
- Success criteria: the existing promptforge-store test suite passes against the rewritten Store; the executor's doc example compiles and runs with VfsRef; a Bashkit script can ls/cat/grep across mounted backends through the adapter.
- Constraints: sync trait (the backends are sync-native; the executor provides asynchrony at the yield boundary); bytes at the trait level with text conveniences above; existing Store semantics preserved exactly (error kinds, anchor-edit rules, numbered reads, write-conflict detection - now via the claims model); no loose files; shared-vfs is std-only with zero dependencies.
- Open questions:
  - Where the Store's bytes physically live at rest (memory backend vs SQLite-backed) - memory for this phase.

## Functional Specification

Three actors share one handle with three different views: Lua through the unchanged store table, the executor through the facade it constructs, and later the engine through the adapter. The operation set is the Store's proven semantics widened by stat, annotated list, and first-class grep. Validation and canonicalization happen once at the handle boundary. The Lua-visible error vocabulary is frozen.

- Actors and workflows:
  - The Lua VM drives the `store` table exactly as today; every operation routes through the Store facade into Vfs.
  - The executor receives one VfsRef per run and constructs the Store facade over it internally.
  - The production host (pattern: wg21-paperflow/crates/papergate/src/app.rs) gets a stock VfsRef, seeds the prompt's declared input keys through `vfs.store()`, runs, and extracts the declared output keys - all without any real files; a missing declared output is an explicit contract error naming the prompt's promise.
  - Bashkit (later phase) consumes the same handle through the adapter; the model's file tools (later phase) consume it directly.
- Inputs and outputs:
  - The prompt's frontmatter declares the host contract: `input: { path, description }` and `output: { path, description }` keys name store paths the host seeds before the run and extracts after (pattern: wg21-paperflow/crates/papergate/papergate.md).
  - Vfs core operations: read (bytes), read_range (1-based inclusive line range, verbatim and numbered variants), write, append, str_replace (anchor-unique), remove (strict: absent is NotFound, directories need the recursive flag; the Store facade keeps Lua's idempotent delete by mapping NotFound to Ok), exists, glob, list (entries with size, type, optional description), stat, grep (pattern over a subtree).
  - grep has a default implementation (read and scan) so simple backends get it free; indexed backends override later.
  - mkdir, rename, copy are first-class; symlink, read_link, chmod exist but default to unsupported.
- States and validation:
  - Mount tables are fixed at construction; a per-run router is cheap Arc-clones plus the run's mounts.
  - All Vfs access is attributed: every operation flows through an Access capability carrying an ExecId, and the claims tables (readers and writers maps from interned canonical path to live identities) live in the handle. The WriteScope registry is deleted.
  - Virtual paths are validated and canonicalized at the VfsRef boundary, never inside backends.
- Errors and recovery:
  - A determinism violation (two live claims on one path from different identities, at least one a write) is a fatal RunErrorKind that terminates the run instantly, naming the path, both identities, and both claim kinds. It is not catchable from Lua.
  - The Lua-visible StoreError vocabulary is preserved unchanged (NotFound, InvalidPath, InvalidRange, AnchorNotFound, AnchorAmbiguous, InvalidAnchor, WriteRace, InvalidPattern).
  - Vfs has its own error kind set; the Store facade maps between them at the boundary.
  - Writes to read-only mounts fail with a clear read-only error, never partial application.
- Security and privacy behavior:
  - Read-only mounts are enforced by the backend, not by convention.
  - Backends never see uncanonicalized paths; traversal escape from a mount prefix is rejected at the router.
- Acceptance criteria:
  - Lua prompts using the store table behave identically before and after the rewrite, verified by the ported test suite.
  - A single VfsRef serves the Store mount, a memory scratch mount, and a second overlay simultaneously with correct longest-prefix routing.

</product-contract>
<implementation-contract>

## Technical Design

The design mirrors the proven Store/StoreRef shape one level down: a sync trait behind a poison-safe cloneable handle, with a router that is itself an implementation so overlays nest. The Store inverts from public trait to public concrete facade over a prefix-scoped capability. All host-OS path complexity is confined to the host backend; the internal namespace is POSIX-shaped everywhere. Four crates are touched: shared-vfs and promptforge-vfs are new; promptforge-store and promptforge-core change.

- Architecture:
  - Vfs is a sync trait; VfsRef is the Arc-plus-mutex cloneable handle mirroring StoreRef's proven pattern, including poison-safe locking.
  - The router is itself a Vfs implementation, so overlays nest and the executor sees one uniform handle. Mounts install builder-style at construction (mount consumes and returns self; the built table is immutable and Arc-shared); the router's acquire returns a routing access that resolves the longest-prefix mount per operation and acquires each backend's access lazily on first touch. Claims live above the router: the Access wrapper checks the fully-resolved canonical path before routing, so conflicts are caught regardless of which backend serves the path.
  - The Store inverts: from public trait with pluggable backends to a public concrete facade (no trait) over a prefix-scoped VfsRef, exposed as vfs.store().
  - All access is capability-based: the backend trait Vfs has only acquire/release/read_only; every operation lives on the backend's VfsAccess trait object, and the public RAII capability Access wraps it with the ExecId, the claims check, and per-call locking. Drop releases the identity and its claims - cancellation, panics, and early returns cannot leak claims.
- Modules and interfaces:
  - Two crates, split generic machinery from promptforge policy:
    - `shared-vfs` (new crate at promptforge/crates/shared-vfs, matching the shared-* family conventions: plain shared-* package name, version.workspace, publish = false; the workspace globs crates/* so no members edit is needed). It depends on nothing: std only, no workspace crates, no external crates - yours: "shared-vfs must not depend on anything else." The interner, claims tables, and locking are all hand-rolled on std. It holds only generic machinery: the traits, the types, the Router, the claims, the handle, a generic memory backend. Nothing in it knows the string "/_promptforge" exists. It is the permanent bottom of the dependency stack.
    - `promptforge-vfs` (new crate at promptforge/crates/promptforge-vfs): the promptforge policy layer - the /_promptforge mount layout, the stock constructors (empty() with the store mount preinstalled), and the Store-facing conventions. It depends on shared-vfs; promptforge-store and promptforge-core sit above it.
  - The manifest expresses the zero-dependency rule: an empty `[dependencies]` table with a comment stating the rule, and nothing else. Fast builds are designed in, not hoped for: zero dependencies means the crate compiles alone and never rebuilds for a dependency rev; dyn at every boundary (Box<dyn Vfs>, Box<dyn VfsAccess>) keeps our code out of dependents' codegen units; profile knobs stay in the workspace root manifest. The named cost: editing shared-vfs rebuilds everything above it, so its surface must stay stable - which this plan's inlined declarations serve.
  - The complete public surface of shared-vfs: traits `Vfs` and `VfsAccess` (backend authors) and `Policy`; types `VfsRef`, `VfsRefBuilder`, `Access`, `ExecId` (opaque: no public constructor - it appears in the `Vfs::acquire` signature, so it must be nameable, but only the handle vends them), `VfsPath`, `VfsPathBuf`, `Entry`, `Stat`, `FileType`, `GrepQuery`, `GrepMatch`, `GrepResults`, `Op`, `Verdict`, `AllowAll`, `VfsError` and its kinds; the memory backend; the host backend (HostBackend::identity / HostBackend::rooted). Everything else is crate-private: `Router` (mounts are installed via VfsRefBuilder), `Volume`, `Claims`, the interner, canonicalize (called inside Access methods, which take &str), and the claims-checking wrapper.
### Public declarations

```rust
/// One backend behind the virtual namespace.
///
/// Sync by design: the Lua VM and the executor's single driver thread are
/// synchronous. Bytes at the operation level. `Send` is required, `Sync`
/// is not: the handle serializes access. The only way to touch storage is
/// to acquire an access object bound to an identity.
pub trait Vfs: Send {
    /// Acquires an access object bound to `id`. Every operation on the
    /// returned object is attributed to that identity: backends that
    /// care can know who is touching what; the rest ignore it.
    fn acquire(&mut self, id: ExecId) -> Result<Box<dyn VfsAccess>, VfsError>;

    /// Releases `id`. Also called from the access object's Drop, so
    /// teardown paths (cancel, panic, early return) cannot skip it.
    fn release(&mut self, id: ExecId) -> Result<(), VfsError>;

    /// Whether this backend rejects all mutations.
    fn read_only(&self) -> bool {
        false
    }
}

/// One identity's session with a backend. Holds the ExecId.
/// All filesystem operations live here - no access object, no ops.
/// Paths arrive validated, canonicalized, and interned; backends never
/// re-validate.
pub trait VfsAccess: Send {
    /// Reads the file at `path` exactly as stored.
    fn read(&self, path: &VfsPath) -> Result<Vec<u8>, VfsError>;

    /// Reads `len` bytes starting at byte `offset`.
    /// Default: read whole, slice. Backends that can seek (host
    /// directory, SQLite) override and never materialize the file.
    /// The handle's line-based ranges are built on this.
    fn read_range(&self, path: &VfsPath, offset: u64, len: u64)
        -> Result<Vec<u8>, VfsError>;

    /// Creates or overwrites the file at `path`.
    ///
    /// Noted but not implemented in v1: a defaulted
    /// `write_owned(&mut self, path: &VfsPath, contents: Vec<u8>)`
    /// delegating to `write`, which the memory overlay would override to
    /// move the buffer with zero copies. Add when profiling calls for it.
    fn write(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Appends to the file at `path`, creating it if absent.
    fn append(&mut self, path: &VfsPath, contents: &[u8]) -> Result<(), VfsError>;

    /// Removes the file, link, or directory at `path`.
    /// Absent is NotFound; a directory without `recursive` is an error.
    /// On a symlink, removes the link, never the target.
    /// (The Store facade keeps Lua's idempotent delete by mapping
    /// NotFound to Ok - strictness lives in the trait, kindness in
    /// the facade.)
    fn remove(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// A confirmed absence is `Ok(false)`; a backend failure is `Err`.
    fn exists(&self, path: &VfsPath) -> Result<bool, VfsError>;

    /// Returns stored paths matching `pattern`, sorted.
    fn glob(&self, pattern: &str) -> Result<Vec<String>, VfsError>;

    /// Lists the directory at `path`.
    fn list(&self, path: &VfsPath) -> Result<Vec<Entry>, VfsError>;

    /// Returns metadata for `path`.
    fn stat(&self, path: &VfsPath) -> Result<Stat, VfsError>;

    /// Creates the directory at `path`.
    fn mkdir(&mut self, path: &VfsPath, recursive: bool) -> Result<(), VfsError>;

    /// Renames or moves, atomically where the backend allows.
    fn rename(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Copies the file at `from` to `to`.
    fn copy(&mut self, from: &VfsPath, to: &VfsPath) -> Result<(), VfsError>;

    /// Replaces the unique occurrence of `old` with `new`.
    /// Zero matches and multiple matches are both errors.
    /// Default: read, count, replace, write. Override to push down.
    fn str_replace(&mut self, path: &VfsPath, old: &str, new: &str)
        -> Result<(), VfsError>;

    /// Searches files under the query's root.
    /// Default: glob, read, line scan. Override for indexed backends.
    fn grep(&self, query: &GrepQuery) -> Result<GrepResults, VfsError>;

    /// POSIX extras; default implementations return Unsupported.
    fn symlink(&mut self, target: &VfsPath, link: &VfsPath) -> Result<(), VfsError>;
    fn read_link(&self, path: &VfsPath) -> Result<VfsPathBuf, VfsError>;
    fn chmod(&mut self, path: &VfsPath, mode: u32) -> Result<(), VfsError>;
}

/// The seven POSIX kinds, named rather than lumped - a virtual
/// /dev/null (char device) is a plausible backend, and Other would
/// hide it. The engine adapter maps the first four directly and the
/// three specials to File with a trace (unreachable in practice:
/// neither our v1 backends nor the engine's ever produce them).
pub enum FileType {
    File,
    Directory,
    Symlink,
    Fifo,
    Socket,
    CharDevice,
    BlockDevice,
}

pub struct Entry {
    pub name: String,
    pub stat: Stat,
    pub description: Option<String>,      // annotation column; None outside /_promptforge;
                                          // the engine adapter drops it
}

/// Options preserve honesty: a backend that does not track a field
/// says None rather than fabricating (an invented mtime is
/// nondeterministic; a constant one makes `ls -t` sort garbage).
/// The adapter emits the 0o644/0o755 default when mode is None.
pub struct Stat {
    pub file_type: FileType,
    pub size: u64,
    pub mode: Option<u32>,
    pub modified: Option<SystemTime>,
    pub created: Option<SystemTime>,
}

pub struct GrepQuery {
    pub pattern: String,
    pub root: VfsPathBuf,
    pub is_regex: bool,
    pub case_insensitive: bool,
    pub glob_filter: Option<String>,
    pub max_results: Option<usize>,
}

pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line: String,
}

pub struct GrepResults {
    pub matches: Vec<GrepMatch>,
    pub truncated: bool,
}
```

```rust
/// Identity of one serial thread of execution. Process-unique,
/// vended from a process-global monotonic counter. Opaque: no public
/// constructor - it must be nameable (it appears in Vfs::acquire and
/// Access::id), but only the handle vends them.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ExecId(u64);

/// Canonical, interned virtual path. Produced by canonicalize() at the
/// moment the API receives a path; interning makes claim lookups
/// pointer-cheap and guarantees alias detection.
pub struct VfsPath { /* interned shared string */ }

pub struct VfsRef { /* Arc<Volume>, poison-safe */ }

impl VfsRef {
    pub fn new(backend: impl Vfs + 'static) -> VfsRef;  // single backend
    pub fn builder() -> VfsRefBuilder; // mount installation
    pub fn acquire(&self) -> Access; // the only way in: fresh ExecId
    pub fn store(&self, access: &Access) -> Store; // facade bound to the caller's identity

    /// Returns a handle with `backend` mounted at `prefix` over this
    /// handle's namespace. The claims table is shared: conflicts are
    /// detected across both views of the same storage.
    pub fn overlay(&self, prefix: &str, backend: impl Vfs + 'static) -> VfsRef;
}

/// Mount installation for VfsRef. Mounts are fixed at build(), so the
/// table is immutable and cheap to Arc-share thereafter.
pub struct VfsRefBuilder { /* the Router under construction */ }

impl VfsRefBuilder {
    pub fn mount(self, prefix: &str, backend: impl Vfs + 'static) -> Self;
    pub fn build(self) -> VfsRef;
}

// Construction patterns:
// identity mount:  VfsRef::builder().mount("/", HostBackend::identity()).build()
//                  - virtual C:/Users/x/y IS host C:\Users\x\y
// chroot mount:    VfsRef::builder().mount("/", HostBackend::rooted("C:/work")).build()
//                  - virtual /a/b IS host C:/work/a/b, containment rejects escape
// overlay:         base.overlay("/_promptforge/runs", runs_backend)
//                  - shares the base's claims table, swaps only the backend view
// stock handle:    promptforge_vfs::empty() - VfsRef plus the /_promptforge/store
//                  memory mount; lives in the policy crate because the mount
//                  layout is promptforge policy, not shared-vfs machinery

/// The public capability. Holds an ExecId and the backend's access
/// object; every operation canonicalizes the path, checks the claims
/// tables, then locks the backend per call (never across an await).
pub struct Access { /* ExecId + Arc<Volume> + Box<dyn VfsAccess> */ }

impl Access {
    /// Returns the capability for a new concurrent thread of execution.
    /// Called by the executor when it spawns one: the child gets a
    /// fresh ExecId, and my claims are deleted from the tables (they
    /// predate the child by construction; a retired claim can never
    /// conflict again). The spawn IS the happens-before edge - no
    /// fence call, no epochs.
    pub fn spawn(&self) -> Access;
    pub fn id(&self) -> ExecId;

    pub fn read(&self, path: &str) -> Result<Vec<u8>, VfsError>;
    pub fn read_string(&self, path: &str) -> Result<String, VfsError>;
    pub fn read_range(&self, path: &str, start: usize, end: Option<usize>)
        -> Result<String, VfsError>;
    pub fn read_range_numbered(&self, path: &str, start: usize, end: Option<usize>)
        -> Result<String, VfsError>;
    // write, append, str_replace, remove, exists, glob, list, stat, grep:
    // same shapes, taking &str
}

impl Drop for Access {
    // releases this identity and deletes its claims
}

/// What operation is being attempted - the policy matches on this.
pub enum Op {
    Read, Write, Append, Delete, Rename, Mkdir, Copy, Grep, // ...
}

/// The policy's answer. Reasons are load-bearing in both directions:
/// Deny's string flows back to the model as the tool error (its
/// recovery path); Ask's string is what the user sees in the
/// approval dialog (what is being asked, and which rule fired).
pub enum Verdict {
    Allow,
    Deny(String),
    Ask(String),
}

/// One policy per VfsRef, consulted by Access on every operation,
/// before the claims check. Dynamic through shared state: the host
/// or UI holds the same Arc and changes behavior mid-run.
pub trait Policy: Send {
    fn check(&self, op: Op, path: &VfsPath) -> Verdict;
}

/// v1 ships AllowAll. ModePolicy (Ask / Plan / Agent, markdown-only
/// in Plan) lives in promptforge-vfs - editor policy, not VFS machinery.
pub struct AllowAll;
```

  - Identity lifecycle maps to VM lifecycle: concurrent threads (fanout arms, async tasks, the walk, host phases) call `acquire()`; blocking children (call chains) borrow the parent's access (no new identity, no false conflicts); transfer of control moves the access object; destruction drops it. The ordered-vs-concurrent distinction is expressed by borrow-vs-spawn, so no flag exists.
  - The host backend is in scope and lives in shared-vfs: std::fs is std, so the zero-dependency rule holds. Stage 1 is thin (direct std::fs ops, lexical+canonicalize containment, failure-atomic writes via sibling temp + rename); stage 2 hardens toward the Bashkit RealFs oracle (resolver trio, symlink policies, Windows long paths and device names) as the threat model demands. Two constructors: HostBackend::identity() (virtual path is the host path) and HostBackend::rooted(dir) (chroot-style, containment-enforced).
  - The oracle's public interface (Bashkit RealFs, from bashkit/crates/bashkit/src/fs/realfs.rs and lib.rs - the shape our port replicates, minus async):

```rust
/// Access mode for the real filesystem backend.
pub enum RealFsMode {
    ReadOnly,   // all write operations return permission denied
    ReadWrite,  // breaks the sandbox boundary; trusted scripts only
}

/// Real filesystem backend scoped to a root directory.
/// The root is canonicalized and validated as a directory at construction.
pub struct RealFs { /* root: PathBuf, mode: RealFsMode */ }

impl RealFs {
    pub async fn open(root: impl AsRef<Path>, mode: RealFsMode) -> io::Result<Self>;
    pub fn root(&self) -> &Path;
    pub fn mode(&self) -> RealFsMode;
    // note: the sync new() is deprecated upstream for blocking;
    // ours is sync by design, so HostBackend::rooted is the sync new
}

// Builder-level mounting (BashBuilder, lib.rs):
//   mount_real_readonly(host_path) / mount_real_readonly_at(vfs_path, host_path)
//   mount_real_readwrite(host_path) / mount_real_readwrite_at(vfs_path, host_path)
//   allowed_mount_paths(...)            - the mount allowlist (TM-FS-013)
//   is_sensitive_mount_path(host_path)  - the sensitive-path denylist check
// Our equivalents: Router::mount(prefix, HostBackend::rooted(dir)) plus the
// Policy layer; the mode maps to our read_only() backend flag.
```
  - `promptforge-store`: the Store facade becomes a public concrete struct (no trait) over a prefix-scoped Access, used by the Lua bindings and by hosts for seeding/extraction; StoreError stays; MemStore/FileStore as public backend types disappear into Vfs backends.
  - `promptforge-core`: run() signature change; RunContext::new takes the VfsRef and builds the Store facade for section VMs (see promptforge/crates/promptforge-core/src/execute.rs).
  - Bashkit adapter (lives near the future integration crate): implements bashkit::FsBackend over VfsRef; whole-file reads served from read; unsupported operations (symlink, chmod) return the engine's unsupported error.
- File and public API changes:
  - `execute::run(prompt, args, resolution, vfs: &VfsRef, config)` replaces the `store: &StoreRef` parameter.
  - promptforge-store's public surface stays source-compatible for Lua-facing behavior; the Store trait is removed from the public API.
  - Caller migration is one line: `StoreRef::memory()` becomes `promptforge_vfs::empty()`. The stock handle always carries the store mount: empty() means empty of content, not of mounts - a router with a fresh memory backend at `/_promptforge/store`, so callers can seed before run() and extract after. `vfs.store()` returns the public Store facade scoped to the mount; hosts and Lua bindings share it, and callers never hardcode the mount path. run() uses the existing mount; the child-router overlay remains only as a defensive fallback for hand-built routers lacking one. Per-run freshness is caller discipline (one VfsRef per run, or clear between runs), matching papergate's fresh-temp-dir-per-run pattern today. Caller census (this workspace): every external caller uses `StoreRef::memory()` (workshop-server session_agents.rs, promptforge-lua vm.rs and benches, executor tests); `with_files` has no callers outside the store crate's own tests and doc examples, so no convenience constructor is carried over.
- Data, persistence, failure, security, and privacy constraints:
  - The trait is bytes-based (`Vec<u8>`) so binary content and Bashkit both fit; text helpers (read_string, numbered ranges) live at the VfsRef layer and error on non-UTF-8 where text is required.
  - Internal namespace is POSIX-shaped (rooted, forward slashes, strict); all host-OS translation (drive letters, case-insensitive comparison, long-path prefixing, device names) lives only in the future real-FS backend, never in the router or virtual paths.
  - Sync trait with an async boundary: store operations become leaf yields in the executor's coroutine protocol (new Request/Answer variants, one dispatch arm, the proven tools.call pattern), answered via spawn_blocking against the sync Vfs. spawn_blocking is task-per-io on a bounded cached pool, never thread-per-io.
  - Consistency rule: every store op takes the yield path uniformly, including memory-backed ones. No inline fast path - answering differently by backend makes interleaving behavior backend-dependent, which is exactly the semantic drift the claims model exists to prevent. The channel hop is the price of that invariance, and it is cheap.
  - Concurrency model, three separate mechanisms for three separate properties: the mutex gives exclusion (uncontended on the executor's single driver thread; operations never hold it across an await), the ExecId gives attribution (no access object, no operations), and the claims table gives correctness.
  - The claims model: the handle holds two maps (readers and writers: interned canonical path to live ExecIds) plus a live-identity set. Every entry is a live, conflict-eligible claim - retired or released claims are deleted, never stored. The conflict rule: a write booms if another live identity appears in the path's readers or writers; a read booms if another live identity appears in its writers; read-read never conflicts. A violation is a fatal RunErrorKind terminating the run on the spot, naming the path, both identities, and both claim kinds. spawn() deletes the parent's claims (the spawn is the happens-before edge); Drop releases the child's.
  - Claims are keyed by interned canonical paths produced at API receipt - yours: "it should be calculated / canonicalized at the time the API receives a path. This guarantees we catch aliases." Canonicalization includes the facade's mount-prefix resolution (Lua's `paper.md` and the host's `/_promptforge/store/paper.md` are one key), lexical normalization of the virtual namespace, and the case rule; symlink aliases do not exist in v1. Per your rabbithole caution, v1 may stub canonicalize to interning the passed string; multi-platform host canonicalization lives in the deferred real-FS backend.
  - The model is primitive-agnostic: it sees spawn and drop events only, so a fanout implemented in Lua over call_async is enforced identically to any executor-level primitive - which the WriteScope registry (fanout-specific) could never have survived. Claims cover plain writes and appends, which WriteScope never did: papergate's cross-arm appends to evidence.md are caught.
  - The prevention pattern the boom teaches: arms write arm-scoped paths and the join merges in arm order - deterministic by construction.
  - Failure-atomicity contract (from Bashkit's TM-FS-014): a failed write, copy, or rename leaves source, destination, and accounting unchanged. Append is one lock acquisition, so read-check-write TOCTOU cannot arise (their TM-DOS-034 lesson).
  - The policy layer: one Policy object per VfsRef (global, not per-mount; a host wanting per-mount rules multiplexes inside its implementation - yours: "I suppose if the host wants a more rich system they can multiplex it into a single Policy object"). Access consults the policy on every operation, before the claims check (a denied operation never registers a claim). The policy is dynamic through shared state: the UI holds the same Arc and flips modes mid-run (e.g. during user_input), and the next operation sees it - no executor involvement.
  - Verdicts carry reasons - yours: "Deny and Ask should have an attached string." Deny's string is the model's recovery path (read-only vs locked vs mode-restricted produce different corrections); Ask's string is what the user sees in the approval dialog (what is being asked, which rule fired).
  - Modes are a policy implementation, not VFS machinery: ModePolicy with Ask (deny all mutations), Plan (mutations only to markdown paths), Agent (allow all) lives in promptforge-vfs; modes gate mutations, never reads. The mode policy absorbs the seal: one-way vs reversible is just who still holds the mode handle. The static read_only() backend flag stays - a property of the mount, orthogonal to policy.
  - Rust API mechanics: #[non_exhaustive] on Entry, Stat, GrepQuery, FileType, and the error enum (Entry is designed to grow - annotations live there); #[must_use] on Access (an acquire dropped immediately is a bug the compiler can catch); Default where a zero value is meaningful (memory backend); private fields with accessors on every public struct that carries invariants.
  - Conscious deviation: VfsAccess has sixteen methods against the one-to-three-methods guideline. The methods are one cohesive capability, not unrelated surface; defaulted methods (read_range, str_replace, grep) keep the required set at eleven; the engine's own FsBackend makes the same choice. Recorded so it is a decision, not a discovery.

### Crate-private declarations

```rust
/// Canonicalizes a virtual path at API receipt. v1 may stub to
/// interning the passed string as-is; the real work is lexical
/// normalization of the virtual namespace only (dot segments, duplicate
/// separators, trailing slash, case rule) plus the facade's mount-prefix
/// resolution, so Lua's `paper.md` and the host's
/// `/_promptforge/store/paper.md` are one key. Multi-platform host
/// canonicalization is a rabbithole that lives in the deferred
/// real-FS backend, not here.
///
/// Crate-private: the only way to form a VfsPath is through Access
/// methods taking &str, so canonicalization at receipt is enforced by
/// visibility, not convention.
fn canonicalize(path: &str) -> Result<VfsPath, VfsError>;

/// One mounted filesystem instance: its backend and the ledger of
/// who is touching what. The two are separately Arc-shareable so
/// overlay() can share the claims table while swapping the backend.
struct Volume {
    backend: Arc<Mutex<Box<dyn Vfs>>>,
    claims: Arc<Claims>,
}

/// The bookkeeping of who is touching what. Every entry is a live,
/// conflict-eligible claim; retired or released claims are deleted,
/// never stored.
struct Claims {
    readers: HashMap<VfsPath, Vec<ExecId>>,
    writers: HashMap<VfsPath, Vec<ExecId>>,
    live: HashSet<ExecId>,
}

/// The mount table. Backends install at prefixes; longest prefix wins.
/// A Router is itself a Vfs, so routers nest. Crate-private: the public
/// concept is "a VfsRef with these mounts," expressed through
/// VfsRefBuilder; privacy enforces mounts-fixed-at-construction, since
/// nobody outside the crate can hold one.
struct Router { /* BTreeMap<VfsPathBuf, Box<dyn Vfs>> */ }

impl Router {
    // acquire(id) returns a routing access: each op resolves the
    // longest-prefix mount and delegates to that backend's access,
    // acquired lazily on first touch of that mount.
}
```

</implementation-contract>
<verification-contract>

## Testing Plan

Parity is the gate: the existing store suite must pass against the rewritten facade unchanged in intent. Around that, new matrices cover the router and the cross-platform path layer, and a smoke test proves the Bashkit adapter end to end. Mount-escape attempts are rejected at the router. No public API outside the four named crates (shared-vfs, promptforge-vfs, promptforge-store, promptforge-core) may change.

- Unit:
  - Path canonicalization matrix per OS convention, including dot segments, mixed separators, casing rules, and traversal rejection.
  - Router longest-prefix dispatch, nested mounts, shadowing, and read-only enforcement.
  - grep default implementation correctness and match semantics.
  - A self-policing manifest test: reads the crate's own Cargo.toml via CARGO_MANIFEST_DIR and asserts the dependency tables are empty, so the zero-dependency rule fails the build instead of eroding.
- Integration and end-to-end:
  - Executor run end-to-end with VfsRef, including a fanout whose cross-arm append to one path terminates the run with a determinism violation naming both arms.
  - Bashkit adapter smoke: mount memory plus store backends, run an ls/cat/grep script, verify output and exit codes.
- Regression, security, and performance:
  - The full existing promptforge-store test suite runs against the rewritten Store unmodified in intent: anchor edits, numbered ranges, idempotent delete, glob grammar, poison handling, write-race detection (now expressed through the claims model).
  - Compile-time Send/Sync assertions for VfsRef and Access (both hold dyn and interior mutability; losing either is a major, invisible break).
  - Claims lifecycle: borrow-vs-spawn semantics, transfer moves claims, drop releases them, spawn deletes the parent's claims (sequential fanouts stay legal), and alias attempts (facade-relative vs mount-absolute spellings of one file) collide on one interned key.
  - Mount-escape attempts (dot segments, absolute re-rooting) rejected at the router.
- Exit criteria:
  - All ported tests pass; the executor doc example compiles and runs; the adapter smoke test passes; no public API outside the four named crates changes.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Own Vfs trait rather than adopting an engine's - yours: "design our own Vfs and VfsRef (like Store and StoreRef)"; our tools need ranges, annotations, and grep that engine traits lack.
  - Sync trait, async boundary - corrected rationale: the trait is sync because the backends are sync-native (memory, std::fs, rusqlite), not because Lua is synchronous. Lua was never the constraint: the executor's yield protocol exists precisely so Lua does not have to be. Store ops become leaf yields answered via spawn_blocking against the sync Vfs, exactly like tools.call - so fanout arms interleave during store I/O instead of stalling the driver (FileStore blocks the driver thread today). Completion-based I/O (io_uring/IOCP via compio) is a backend-and-scheduler concern the boundary absorbs; the trait never changes.
  - Bytes at the trait level - Bashkit and future binary content require it; text semantics layer above.
  - Borrowed write, with write_owned noted but not implemented - yours: "for the memory vfs overlay, taking ownership of the contents string is a natural fit," tempered by "note the signature but leave it out of the implementation"; the trait documents the defaulted `write_owned(Vec<u8>)` hatch for the memory overlay's zero-copy move, to be added only when profiling calls for it.
  - Store becomes a concrete facade, not a trait - yours: "it is no longer a trait it is just a regular, private implementation which is exposed to Lua." Refined to public-concrete (not private) because production hosts seed declared inputs and extract declared outputs through it (the papergate pattern).
  - Store mounts at `/_promptforge/store` - yours: 'it installs into the overlay: "/_promptforge/store"'.
  - grep is a first-class Vfs operation with a default scan implementation - yours: "the Store implementation now calls into the Vfs to read, grep, etc"; the default keeps simple backends free of index work while SQLite/FTS backends can override later.
  - Router uses a POSIX-shaped internal namespace on every OS - one canonical form, host translation confined to the real-FS backend.
  - Mounts fixed at construction - matches both engines' models and keeps routing immutable and cheap to clone per run.
  - Crate placement in the shared-* family - yours: "it should be in a shared-* crate"; shared-vfs sits beside shared-protocol and shared-progress as cross-cutting infrastructure with plain naming and publish = false, at the bottom of the dependency stack.
  - Handle-level synchronization over backend-level - yours: "Vfs will be accessed concurrently for sure, because of fanout"; fanout interleaves on one driver thread and sync operations cannot tear, so one mutex at the handle beats Bashkit-style per-backend RwLocks, and the WriteScope registry covers the semantic (not data) race.
  - The claims model for determinism - yours: "It's safe, but it's not correct because that's non-deterministic behavior. That means replay is not going to work right." Attribution via ExecId presented at lock acquisition (your token-at-the-mutex), identity tied to the VM lifecycle (creation, transfer, destruction), claims released on destroy, and conflicts fatal: "It should fail loud and terminate instantly."
  - RAII access object - yours: "we just put acquire and release on the Vfs, and acquire returns a `Box<VfsAccess>`... So it is literally impossible to mess up." Drop releases claims, so cancellation and error paths cannot leak; the ordered-vs-concurrent distinction is expressed by borrowing the parent's access versus spawning a fresh one, so no flag exists.
  - spawn() over fork() - fork lies twice (children do not inherit claims; the word is Unix-only in a Windows-first product); spawn matches tokio and the Lua call_async vocabulary.
  - Volume and Claims as the internal names - Shared named a role, not a concept; the volume is one mounted filesystem instance (backend plus claims ledger), and the claims table is its own named type. Inner was the tolerated-but-weaker idiom.
  - Builder-style mount installation with lazy per-mount acquire - mounts are declared in one place and immutable after construction; backends learn about identities only when a path under their prefix is first touched.
  - Router crate-private, VfsRefBuilder public - yours: "I dont quite understand why Router is public." The public concept is "a VfsRef with these mounts"; privacy enforces mounts-fixed-at-construction because nobody outside the crate can hold a Router.
  - ExecId public but opaque - it must be nameable (it appears in the Vfs::acquire signature and Access::id), but no public constructor; only the handle vends identities.
  - Two-crate split, generic machinery vs promptforge policy - shared-vfs holds only generic VFS machinery and never names /_promptforge; promptforge-vfs holds the mount layout and stock constructors. Approved: "Yes. That sounds good."
  - overlay() shares the claims table - two handles over the same storage with two claims tables would blind the determinism check exactly where two views coexist; Volume therefore holds backend and claims as separately shareable Arcs. VfsRef itself implements Vfs (forwarding acquire with the given ExecId) so a base handle mounts under a child router.
  - WriteScope registry deleted - yours: "I never liked the WriteScope registry anyway." The claims table is the single mechanism and covers plain writes and appends, which WriteScope never did.
  - Uniform yield path for all store ops - yours: "I agree with all of that, and make a note about consistency." No inline fast path for memory backends, because backend-dependent answer paths make interleaving behavior backend-dependent; the channel hop is the price of the invariance.
  - Wide access trait as a conscious deviation - the filesystem capability is cohesive, defaulted methods keep the required surface at eleven, and the engine's own backend trait makes the same choice; recorded against the small-trait guideline so it is a decision, not a discovery.
  - The stock VfsRef always carries the store mount - yours: "The empty Vfs should still support a store"; empty() means empty of content, not of mounts, because the seeding contract requires the store to exist before run() is called.
  - vfs.store() as the seeding/extraction handle - the production workflow (stock Vfs, seed declared inputs at known filenames, run, extract declared outputs, no real files) needs a public facade available before run(); hosts and Lua bindings share it. Confirmed: "vfs.store() looks like the correct model."
  - vfs.store() ships as an extension trait in promptforge-store, not an inherent method on VfsRef - the Store facade type lives in promptforge-store, which sits above promptforge-vfs and shared-vfs in the dependency stack, so shared-vfs cannot name the type. A prelude-exported extension trait preserves the declared vfs.store(&access) call shape without inverting the stack.
  - API symmetry as a deliberate property - yours: "on the Rust side the API is essentially the same which means no new learning surface for integrators." The Store is the scratchpad that survives the context reset when control transfers between sections; Lua's store table and the Rust facade are the same interface in two languages, documented once, and the model's later file tools become a third consumer of the same verbs.
  - Host backend in scope in shared-vfs - yours: "without it, this is fucking useless." std::fs is std, so the zero-dependency rule holds; stage 1 thin with atomic writes, stage 2 hardening toward the RealFs oracle. (Supersedes the earlier deferral, which treated papergate's FileStore use as diagnostics-only.)
  - Host backend will be ported, not adapted - source exploration of Bashkit's RealFs shows it is tokio::fs throughout (adapting means block_on per call plus their Tokio dependency) and leaves two Windows gaps we require (long-path prefixing, device names); its value is the hardened algorithms and the realfs test suite as a behavioral oracle, not the type.
- Rejected alternatives:
  - Co-locating Vfs with run()'s other traits in promptforge-core-support - rejected: that crate holds small host-support primitives (cancel, observe, untrusted guards) while Vfs is a subsystem (router, paths, glob, grep, backends) with consumers well beyond the executor (Lua, hosts, workshop-server, the future engine adapter); the workspace has no run()-traits crate to join, since Tool, ModelResolver, and ToolResolver each live in their domain crates. Revisit: only as an additive umbrella re-export crate for integrator ergonomics, never as a move.
  - Async trait - rejected: block_on inside a current-thread driver is a deadlock hazard; revisit only if a backend genuinely needs async I/O.
  - Adopting bashkit::FsBackend as the core trait - rejected: whole-file-only reads, five-field metadata, pre-1.0 dependency direction; the adapter isolates it instead.
  - Keeping Store as a public trait with pluggable backends - rejected: two filesystem abstractions is the incoherence this plan exists to remove.
- Assumptions, risks, and notes:
  - Sync real-FS reads will block the driver thread; acceptable for this phase, offload later if traces show stalls.
  - Two glob implementations (Store's matcher and the VFS's) must not drift; port one, delete the other.
  - Error mapping between Vfs kinds and StoreError must be total; a missed kind is a Lua-visible behavior change.
  - Strictness will bite spawn-and-forget patterns: a parent that keeps writing paths a live task has claimed will boom. Error message quality decides whether authors experience this as guidance or noise.
  - The policy seam was pressure-tested against learned allow-rules ("Always allow this directory" via a rule-list policy) and absorbed the feature with zero changes to the trait, Access, claims, or executor - evidence the interface is wide enough before it was needed.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (builds only the gateway, the default workspace member; run `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui` once after cloning). Full desktop build: `cargo workshop` (a `.cargo/config.toml` alias for `run -p build-workshop`; add `--release` or `--target <triple>` as needed).
- Focused test command pattern: `cargo nextest run -p <crate> <test-name-filter>` (nextest config in `.config/nextest.toml`; heavy STT/tool-picker suites are concurrency-limited via test groups).
- Component test command pattern: `cargo nextest run -p <crate>`; integration targets also runnable as `cargo test -p <crate> --test it <filter>` (CI example: `cargo test -p gateway-stt --test it architecture`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, plus doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc` (workshop crates are covered separately on Windows: `cargo nextest run --locked -p workshop -p workshop-server` and `cargo test --doc -p workshop -p workshop-server`).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop crates: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`). Supply chain: `cargo deny check` and `cargo audit`.
- Formatter check command: `cargo fmt --all --check`.
- Docs command: `mdbook build guide` for the user guide; `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` for API docs (CI runs it with `RUSTDOCFLAGS: -D warnings`).
- Test placement and naming conventions: unit tests live in `src/` modules; integration tests live in `crates/<crate>/tests/`. Multi-file suites use one target with a `main.rs` entry plus sibling module files (e.g. `crates/promptforge-core/tests/suite/{main,execution,fanout,parsing,shipped,support}.rs`); gateway and workshop-server use a `tests/it/` target. Fixtures sit beside tests (e.g. `tests/prompts/{valid,invalid,execution}`, `workshop-server/tests/fixtures`). Test names are long descriptive snake_case sentences (e.g. `a_process_lifetime_lease_recovers_after_its_owner_is_terminated`). Benches use criterion (`crates/promptforge-core/benches`). Node helper scripts in `tools/` have sibling `.test.mjs` files.
- Directory map: `crates/` holds all 36 workspace crates grouped by product prefix (`promptforge-*` executor/language, `gateway*` inference server, `workshop*` desktop app, `shared-*` cross-product substrate, `build-*` build tooling, `product-integration-tests`); `crates/shared-ui` is a TypeScript+CSS package excluded from the Cargo workspace. `guide/` is the mdbook user guide (four doc sets: Workshop, gateway, prompt language, agent programs). `prompts/` holds example prompt programs. `design/` holds design notes. `tools/` holds Node.js helper scripts (gateway sidecar staging, TTS live checks). `vibe/` holds project governance records: `archdoc.md`, dated decision logs, `ACTIVE`. `.config/nextest.toml` configures nextest; `.cargo/config.toml` sets the static-CRT Windows target and the `workshop` alias; `.github/workflows/` holds CI and release packaging; `images/` holds README assets.
- Component boundaries (per `vibe/archdoc.md`): executor (`promptforge-core` and supporting `promptforge-*` crates) parses and runs prompt pipelines and Lua agent programs; gateway (`gateway*` crates) is an independent server owning model routing, provider credentials, and local inference; CLI (`promptforge` crate) is a thin shell adapter over the executor; Workshop UI (`workshop`, `workshop-server`) is the Tauri desktop shell hosting the executor in-process; store (`promptforge-store`) is the run-scoped virtual filesystem; the Lua VM boundary (`promptforge-lua`) sandboxes prompt code; shared substrate (`shared-*`) carries progress, loopback discovery, protocol, and sidecar facilities. Dependency directions: executor depends on gateway protocol, store, Lua boundary, shared substrate; CLI and Workshop depend on executor, gateway, store, substrate; store and substrate depend on nothing. AGENTS.md enforces four cross-product rules: Gateway crates cannot depend on Workshop or PromptForge product crates; PromptForge crates cannot depend on Gateway or Workshop crates; Workshop crates cannot depend on Gateway crates.
- Conventions summary: Rust edition 2024 on the stable toolchain (`rust-toolchain.toml`), resolver 3, workspace version 0.3.0. Workspace lints forbid `unsafe_code`, deny `clippy::all` plus `unwrap_used`/`expect_used`, and warn on missing docs. Comments explain non-obvious constraints and cite upstream issue URLs for platform workarounds. Behavior changes ship with tests in the same change; structural enforcement (parsers, snapshots, allowlists, topology checks) requires explicit user approval. Cargo features gate real constraints (toolchain or native build), never product shape. Runtime and serve paths never compile native dependencies, exit the process, or install process-global state. Long-running work reports through `shared-progress`. The two web UIs are TypeScript bundled by esbuild through Cargo build scripts, with Node.js 22 required.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: shared-vfs skeleton, value types, and canonical paths [completed]

- Component: shared-vfs core
- Create `crates/shared-vfs` (package `shared-vfs`, version.workspace, publish = false, empty `[dependencies]` table carrying the zero-dependency comment; the workspace `crates/*` glob needs no members edit) with the `src/lib.rs` module layout.
- Write `crates/shared-vfs/AGENTS.md` following the per-crate convention (every workspace crate has one), with exactly these three rules: "std only. No dependencies, workspace or external. The manifest test enforces this; never weaken it." / "No promptforge policy: no /_promptforge paths, no Store, no run concepts." / "The public surface is load-bearing: add defaulted methods, never change existing signatures. Every edit rebuilds the whole stack."
- Define `VfsError` and its kinds (#[non_exhaustive]), `FileType` (seven POSIX kinds), `Entry` (name, stat, description annotation column), `Stat` (honest Option fields), `GrepQuery`, `GrepMatch`, `GrepResults` (#[non_exhaustive], private fields with accessors where invariants exist).
- Implement the hand-rolled interner, `VfsPath`/`VfsPathBuf`, and crate-private `canonicalize()` (lexical normalization of the POSIX-shaped namespace: dot segments, duplicate separators, trailing slash, case rule; stub-to-intern acceptable in v1).
- Tests: the self-policing manifest test (reads the crate's own Cargo.toml via CARGO_MANIFEST_DIR and asserts the dependency tables are empty) and the path canonicalization matrix (dot segments, mixed separators, casing rules, traversal rejection).

</step-1>

<step-2>

### Step 2: Vfs and VfsAccess traits and policy types

- Component: shared-vfs core
- Define `trait Vfs` (acquire/release/read_only, Send, sync by design), `trait VfsAccess` (sixteen methods; defaults: read_range slices a whole read, str_replace is read-count-replace-write, grep is glob-read-line-scan, symlink/read_link/chmod return Unsupported), `trait Policy`, `enum Op`, `enum Verdict` (Deny and Ask carry reason strings), and `AllowAll`.
- Tests: default read_range, str_replace (zero and multiple matches are both errors), and grep match semantics against an in-test stub backend; unsupported defaults return the right error kind.

</step-2>

<step-3>

### Step 3: handle, Access capability, and claims tables

- Component: shared-vfs core
- Implement `ExecId` (opaque, process-global monotonic counter, no public constructor), `Volume` (backend and claims as separately Arc-shareable), `Claims` (readers/writers maps from interned VfsPath to live ExecIds plus the live set; retired or released claims are deleted, never stored), `VfsRef` (`Arc<Volume>`, poison-safe locking, `VfsRef::new` and `acquire()`), and `Access` (#[must_use]; canonicalizes at receipt, consults the Policy before the claims check so a denied operation never registers a claim, registers claims, locks the backend per call; `spawn()` vends a fresh ExecId and deletes the parent's claims; Drop releases the identity and its claims).
- Conflict rule: a write fails when another live identity appears in the path's readers or writers; a read fails on another live writer; read-read never conflicts. The violation is a dedicated VfsError kind naming the path, both identities, and both claim kinds (the executor maps it to the fatal RunErrorKind in step 10).
- Tests: claims lifecycle (borrow vs spawn, transfer moves claims, drop releases, spawn deletes the parent's claims so sequential fanouts stay legal), alias collision (facade-relative and mount-absolute spellings of one file land on one interned key), the conflict matrix, and compile-time Send/Sync assertions for VfsRef and Access.

</step-3>

<step-4>

### Step 4: router, builder, and overlays

- Component: shared-vfs router
- Implement crate-private `Router` (BTreeMap mount table, longest-prefix dispatch, lazy per-mount acquire on first touch), `impl Vfs for Router` (routers nest), `impl Vfs for VfsRef` (forwards acquire with the given ExecId, so a base handle mounts under a child router), `VfsRefBuilder` (mount consumes and returns self; build() freezes the table), and `VfsRef::overlay()` (shares the claims table, swaps only the backend view).
- Enforce at the router: writes to read-only mounts fail with a clear read-only error and never partially apply, and traversal escape from a mount prefix is rejected.
- Tests: longest-prefix dispatch, nested mounts, shadowing, read-only enforcement, mount-escape rejection (dot segments, absolute re-rooting), and one VfsRef serving the store mount, a memory scratch mount, and a second overlay simultaneously.

</step-4>

<step-5>

### Step 5: memory backend

- Component: shared-vfs backends
- Implement the generic in-memory backend in shared-vfs carrying former MemStore semantics (Default where a zero value is meaningful; acquire/release accept ExecId attribution as a no-op).
- Tests: the full operation surface (read, read_range, write, append, remove strictness, exists, glob, list, stat, mkdir, rename, copy) against the memory backend.

</step-5>

<step-6>

### Step 6: host backend, stage 1 thin

- Component: shared-vfs backends
- Implement `HostBackend::identity()` (virtual path is the host path) and `HostBackend::rooted(dir)` (chroot-style) in shared-vfs over direct std::fs: lexical plus canonicalize containment for rooted, failure-atomic writes (sibling temp file plus rename; a failed write, copy, or rename leaves source, destination, and accounting unchanged), and the read_only flag rejecting all mutations.
- Tests: containment escape rejection, atomic-write failure cases, and round trips in temp directories.

</step-6>

<step-7>

### Step 7: promptforge-vfs policy crate

- Component: promptforge-vfs
- Create `crates/promptforge-vfs` (depends on shared-vfs only): the `/_promptforge/store` mount layout, the `empty()` stock constructor (a router with a fresh memory backend at the store mount; empty of content, not of mounts), and `ModePolicy` (Ask denies all mutations, Plan allows mutations only to markdown paths, Agent allows all; modes gate mutations, never reads) behind a UI-flippable shared Arc so a mode change mid-run takes effect on the next operation.
- Tests: mode flip visibility through the shared handle, Plan markdown-only enforcement, and empty() carrying the store mount.

</step-7>

<step-8>

### Step 8: Store facade rewrite and parity suite

- Component: promptforge-store
- Rewrite `promptforge-store`: the Store trait, MemStore, and FileStore disappear from the public API; a public concrete `Store` facade wraps a prefix-scoped Access and is exposed as `vfs.store(&access)` via a prelude-exported extension trait (see decision record). Preserve the StoreError vocabulary exactly with a total VfsError-to-StoreError mapping, anchor-edit rules, numbered reads, idempotent delete (NotFound maps to Ok), and the glob grammar (port one glob implementation, delete the other). Delete the WriteScope registry.
- Tests: the full existing promptforge-store suite ported onto the facade unmodified in intent (anchor edits, numbered ranges, idempotent delete, glob grammar, poison handling, write-race detection now expressed through the claims model). This suite is the parity gate.

</step-8>

<step-9>

### Step 9: executor API pivot to VfsRef

- Component: promptforge-core
- Change `execute::run(prompt, args, resolution, vfs: &VfsRef, config)`; `RunContext::new` takes the VfsRef and builds the Store facade internally; run() overlays a fresh memory store only as a defensive fallback for hand-built routers lacking the mount; the scheduler installs the current Access per chain step.
- Migrate callers: `StoreRef::memory()` becomes `promptforge_vfs::empty()` in workshop-server session_agents.rs, promptforge-lua vm.rs and benches, and executor tests; `with_files` is not carried over. The executor doc example compiles and runs with VfsRef.
- Tests: executor run end-to-end with VfsRef, and the papergate-shaped seed-run-extract round trip on a stock VfsRef with no real files (a missing declared output is an explicit contract error naming the prompt's promise).

</step-9>

<step-10>

### Step 10: store operations as leaf yields

- Component: promptforge-core
- Add the new Request/Answer variants and one dispatch arm (the proven tools.call pattern) so every Lua store operation becomes a leaf yield answered via spawn_blocking against the sync Vfs: uniform for all backends, no inline fast path. Map the claims-violation VfsError to the fatal determinism RunErrorKind that terminates the run instantly and is not catchable from Lua.
- Tests: a fanout whose cross-arm append to one path terminates the run with a determinism violation naming both arms, the arm-scoped-writes plus ordered-merge fixture passing, and interleaving invariance across memory and host backends.

</step-10>

<step-11>

### Step 11: Bashkit adapter spike

- Component: bashkit adapter spike
- Implement `bashkit::FsBackend` over VfsRef as a path dependency against the local `bashkit/` clone: whole-file reads served from read, symlink/chmod return the engine's unsupported error, FileType maps the first four kinds directly and the three specials to File with a trace, Stat mode None emits the 0o644/0o755 defaults, and the adapter captures the current ExecId at exec start.
- Compile-check and smoke-test an ls/cat/grep script across mounted memory and store backends, verifying output and exit codes. The deliverable is evidence that the trait subsumes Bashkit; a mapping failure is the spike working as intended.

</step-11>

- Deferred and out of scope: do_shell dispatcher and argv pattern, git builtins and commit path, SQLite run-record backend and /_promptforge renderers, terminal mirrors, annotated listings, Bashkit integration proper (hooks, TraceMode, analyze), kaish fallback, stage-2 host-backend hardening.

</execution-plan>
