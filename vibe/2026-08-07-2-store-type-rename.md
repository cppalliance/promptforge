---
name: Store type rename
overview: Rename virtual-file types to Store / MemStore / StoreRef, and make Lua sys.x fail loud on missing keys. Two commits.
todos:
  - id: rename-store-rs
    content: Rename FileStore->Store, MemVfs->MemStore, Store->StoreRef in store.rs (docs, types, unit tests)
    status: completed
  - id: update-rust-callers
    content: Update all Rust imports/call sites to StoreRef including test modules; Fix FailingStore impl
    status: completed
  - id: update-docs
    content: Update README, design-*, STATUS, research notes that name the types
    status: completed
  - id: verify-commit-rename
    content: fmt, clippy -D warnings, full test pass, commit 1 (rename only)
    status: completed
  - id: sys-fail-loud
    content: Seal Lua sys; add lua + subst tests for missing/write; fix README now wording
    status: completed
  - id: verify-commit-sys
    content: fmt, clippy -D warnings, full test pass, commit 2 (sys fail-loud)
    status: completed
isProject: false
---

# Store Rename + Lua `sys` Fail-Loud

## Commit 1: Store / MemStore / StoreRef rename

| Current | New | Role |
|---|---|---|
| `FileStore` | `Store` | Backend trait |
| `MemVfs` | `MemStore` | In-memory backend |
| `Store` | `StoreRef` | Shared handle |

Do not introduce a disk-backed `FileStore` type. Leave the Lua global `store`, local names like `store`, the `store` module, `StoreError`, and observer detail strings unchanged.

Guidance: [tools-public/rulebooks/rust-rulebook.md](tools-public/rulebooks/rust-rulebook.md).

### Rename order in [store.rs](promptforge/crates/promptforge-core/src/store.rs)

1. Handle `Store` -> `StoreRef`
2. Trait `FileStore` -> `Store`
3. `MemVfs` -> `MemStore`

### Rust callers and tests

Replace handle type uses with `StoreRef` in production and test code:

- [store.rs](promptforge/crates/promptforge-core/src/store.rs) unit tests and doctests (`Store::memory` -> `StoreRef::memory`, `MemVfs` -> `MemStore`, trait name in examples)
- [lua.rs](promptforge/crates/promptforge-core/src/lua.rs) including `impl Store for FailingStore` and every `Store::memory()` in tests/doctests
- [execute.rs](promptforge/crates/promptforge-core/src/execute.rs) + [execute/tests.rs](promptforge/crates/promptforge-core/src/execute/tests.rs)
- core-tests: dump.rs, dev.rs, suite.rs, scenarios.rs
- cli main.rs, mcp runner.rs + progress.rs tests

No new behavioral tests for the rename; updating every existing test/doctest that names the types is required so the suite stays green.

### Docs

README, design-core.md, design-core-orig.md, design-mcp-server.md, research rationale, STATUS.md if it names the Rust handle.

Skip prompt titles like "Store Fall-through".

### Verify commit 1

`cargo fmt --all`, `clippy -D warnings`, `cargo test -p promptforge-core` and compile/test any other packages that import the renamed types. One rename-only commit.

---

## Commit 2: Lua `sys.x` fails loud

### Current behavior

- Prose `{{ sys.bogus }}` already errors in [subst.rs](promptforge/crates/promptforge-core/src/subst.rs) (`missing {{ path }}`).
- Lua `sys` is injected as a plain table in [`inject_host`](promptforge/crates/promptforge-core/src/lua.rs); missing keys return `nil`.

### Change

After converting the sys JSON to a Lua table in `inject_host`, attach a metatable:

- `__index` - if the key is absent from the real table, raise a Lua error naming the missing key (e.g. `unknown sys field 'bogus'`). Present keys still resolve normally.
- `__newindex` - reject every write (`sys` is runtime metadata, not author-writable).

Implementation detail: keep the real key/value pairs on the table (or on a private store table the metatable closes over). Prefer a private data table + empty proxy if that keeps `pairs(sys)` from seeing internal machinery; otherwise a metatable on the populated table with `__index` only for misses is enough. Do not allow authors to replace the metatable after injection if `setmetatable` is already stripped by hardening - confirm and keep host `raw_set` of the sealed table.

### Tests (required in the same change)

Per the rust rulebook: new behavior ships with tests in the same commit.

**Lua (`lua.rs` tests):**

- happy path: `return tostring(sys.id)` / `return sys.when` succeed with injected values (extend or keep `reads_sys`)
- missing read: `local _ = sys.bogus` / `return sys.bogus` -> `Error::Lua`, message includes the unknown field name
- write rejected: `sys.when = "x"` -> `Error::Lua`
- write of new key rejected: `sys.extra = 1` -> `Error::Lua`

**Prose substitution (`subst.rs` tests):** keep/extend coverage that `{{ sys.bogus }}` errors (already `missing_key_is_error`); add an explicit `sys`-namespaced case if the existing test only exercises `var` / unknown namespace.

**Do not** leave fail-loud as docs-only.

### Docs

- README Substitution / Lua host globals: note that Lua `sys` only exposes the keys the runtime injected, and unknown reads or any writes raise.
- Fix the README gloss for `sys.now`: it is refreshed at each section start, not a build-time stamp.
- design-core.md one-line note if it describes `sys` access.

### Verify commit 2

Same fmt / clippy / test gate with the new tests green. Second commit focused on fail-loud `sys` access.
