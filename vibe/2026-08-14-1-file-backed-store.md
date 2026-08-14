---
name: File-backed store
overview: Add a FileStore backend to promptforge-core that persists the virtual store as flat files in a caller-provided directory, enabling post-run inspection and cross-run resume via store.exists.
todos:
  - id: file-store-impl
    content: Add FileStore struct and Store impl with unit tests
    status: completed
  - id: dev-runner-wire
    content: Wire dev runner to FileStore; remove post-run dump reconcile
    status: completed
  - id: cli-flag
    content: Add --store flag to CLI
    status: completed
  - id: verify
    content: Final verification pass
    status: completed
isProject: false
---

# File-backed store for pipeline debugging and resume

## Target

A `FileStore` that implements the existing `Store` trait, backed by a real directory. The caller provides the path explicitly - no defaults, no derivation. The prompt engine remains a pure executor.

## What exists

- `Store` trait with 8 methods: `write`, `append`, `read_lines`, `read`, `str_replace`, `delete`, `glob`, `exists`
- `StoreRef` wraps any `Box<dyn Store + Send>` behind `Arc<Mutex<...>>`
- `StoreRef::new(backend)` already accepts custom backends
- `StorePath::parse` validates logical paths (rejects traversal, control chars, device names)
- Dev runner's `dump/paths.rs` has `safe_relative_path` mapping logical paths to FS-safe relative paths
- All three callers (CLI, dev, MCP) currently use `StoreRef::memory()`

## Design

### FileStore

A new struct in `promptforge-core/src/store/`:

```rust
pub struct FileStore {
    root: PathBuf,
}
```

Implements `Store`. Each method maps the already-validated logical path to a filesystem path under `root` using the same confinement rules the dev dump already enforces (reuse or inline the `safe_relative_path` logic). Operations are synchronous filesystem I/O - acceptable because `StoreRef` is already behind a mutex and store ops are not on the hot path.

Methods map directly:
- `write` -> create parent dirs + `fs::write`
- `append` -> create parent dirs + `OpenOptions::append`
- `read` -> `fs::read_to_string`; missing file -> `StoreError`
- `read_lines` -> read + number lines (same format as MemStore)
- `str_replace` -> read + single replace + write back
- `delete` -> `fs::remove_file`; missing -> `StoreError`
- `glob` -> walk `root` with pattern matching (reuse existing glob logic from MemStore, adapted to fs)
- `exists` -> `Path::exists` (returns `Ok(false)` for missing, not error)

### Constructor

```rust
impl FileStore {
    pub fn new(root: impl Into<PathBuf>) -> std::io::Result<Self>
}
```

Creates the root directory if it does not exist (`create_dir_all`). Returns `io::Error` if creation fails. No other implicit behavior.

### StoreRef integration

Callers construct with: `StoreRef::new(Box::new(FileStore::new(path)?))`

No new constructor on `StoreRef`. No convenience method. The caller does the plumbing explicitly.

### Caller changes

Each caller decides its own policy:

- **Dev runner**: derive the store path from the prompt file: same directory, same stem, no extension, as a subdirectory (e.g. `prompts/research-person.md` -> `prompts/research-person/`). This is the dev runner's policy, not the engine's. Pass to `FileStore::new`. Remove the post-run dump reconcile (store is already on disk). The `dump/` module simplifies or dies.
- **CLI**: add a `--store <dir>` flag. Required for persistence; without it, use `StoreRef::memory()` (ephemeral, same as today). No default path.
- **MCP server**: use `StoreRef::memory()` by default (runs are independent). Add optional `[run].store_dir` config if needed later. Not in this plan.

### Path confinement

`FileStore` reuses the existing `StorePath::parse` validation (already rejects traversal, `..`, absolute paths, device names). Additionally, the FS mapping function (from dev dump's `safe_relative_path` or equivalent) ensures no escape from `root`. A logical path that fails the FS safety check returns `StoreError` rather than silently skipping.

### No new behavior in the engine

`execute::run` receives a `&StoreRef`. It does not know or care whether the backend is memory or files. The `Store` trait contract is unchanged. Resume logic (`store.exists` then skip) is prompt-author code in Lua, not engine behavior.

## Steps

1. Add `FileStore` struct and `Store` impl in `crates/promptforge-core/src/store/file.rs`. Unit tests covering all 8 methods plus confinement rejection.
2. Wire dev runner to use `FileStore` instead of memory + post-run dump. Remove or simplify the dump reconcile.
3. Add `--store <dir>` to CLI. Memory when absent.
4. Verify: fmt, clippy, all tests.

## Binding rules

### Rust (from rust-rulebook.md)

- `Result` for expected failure; panic only for bugs.
- No `unsafe`.
- No new dependencies unless named in this plan (none named - use only std fs).
- Document every new public item; `# Errors` on public fallible functions.
- Test in the same commit as the behavior it guards.
- `cargo fmt --all --check` and `cargo clippy -p promptforge-core --all-targets --all-features -- -D warnings` before every commit.
- Take `&Path` / `impl Into<PathBuf>` for path params; return `PathBuf` when owned.
- Propagate errors with `?`; map `io::Error` through `StoreError` at the boundary.

### Vibe-coding Rust (from vibe-coding-rust-guide.md)

- No `.clone()` to silence borrows; restructure first (ladder: reorder > split fields > entry API > indices > own > deliberate clone > interior mutability).
- No `unwrap`/`expect` in library code without an invariant comment.
- No `Arc`/`Rc`/`RefCell` spray - `FileStore` owns its `PathBuf`; no shared state beyond what `StoreRef`'s existing mutex provides.
- Prefer std first: `std::fs`, `std::path`, `std::io`. No external filesystem crate.
- Run `cargo check` after every edit; fix with the redesign ladder, not clone/Arc/unsafe.
- Every external crate name verified against crates.io (none needed here).
- Async: `FileStore` is sync (called under `StoreRef`'s mutex, which is already not held across awaits). No async fs.
- Borrow checker: `FileStore` owns `root: PathBuf`; methods take `&self` / `&mut self` per trait; no lifetime gymnastics expected.

### Agent catch-list (grep before commit)

```
rg -n '\.unwrap\(|\.expect\(' --glob '*/store/file.rs'
rg -n '\.clone\(' --glob '*/store/file.rs'
rg -n '\bunsafe\b' --glob '*/store/file.rs'
```

Zero hits required for the first two (library code); unsafe forbidden outright.
