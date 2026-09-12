# Test-Namespace Cleanup and VFS Debt Removal

<product-contract>

## Product Requirements

Two workstreams land together in the promptforge repository, with two supporting file patches beside it. First, test-only imports and re-exports are removed from non-test modules across the workspace, and the vibe coding tool is patched so runs stop committing their audit ledger into the repository. Second, four debts introduced by the VFS foundation run (the twelve commits `upstream/master..16803524`) are removed: a workspace member whose out-of-repo path dependency breaks every fresh clone and CI run, a process-global path interner that leaks every distinct path for the process lifetime and is reachable from model-controlled Lua, a handle boundary that panics where its trait documents a recoverable error, and a self-policing manifest test that a standard TOML idiom bypasses.

- Problem and users: test plumbing (`#[cfg(test)] use` and `pub use` lines) sits in non-test modules in ten files across five crates, where it exists only to feed descendant test globs; the vibe tool's Mark step stages its ledger into every step commit, so drained ledgers and review queues accumulate as committed repo files; the VFS foundation run shipped four debts found by a debt-collection pass over `upstream/master..HEAD`. Users are the promptforge maintainers and every CI runner.
- Goals: every non-test module carries production items only; test modules import what they need directly; the vibe ledger is scratch, never committed; the four debts are removed with the operator's chosen remedies; `vibe/archdoc.md` describes the real component graph.
- Non-goals: no content-level string dedup machinery; no change to the legitimate pattern of `#[cfg(test)]` methods, functions, and imports whose consumers live in the same module; no change to the deliberate facade re-exports from implementation crates (`promptforge-lua`, `promptforge-model-client`, `promptforge-core-support`, `promptforge-parser`); no remediation of the twelve rejected debt candidates; no commit sequencing (left to the executing tool).
- Success criteria: a workspace grep finds no `#[cfg(test)]` import or re-export in a non-test module whose only consumers are test modules; a vibe run's step commits contain no ledger file; `cargo metadata --locked` and `cargo build` succeed with no sibling checkout beside the repository; a loop canonicalizing distinct paths does not grow the process heap monotonically; a backend whose `acquire` fails produces a run error, not a panic; the shared-vfs manifest test fails when a `[dependencies.foo]` sub-table is injected.
- Constraints: the workspace deletion rule moves removed files to `cabinet/_trash/` and never hard-deletes; the promptforge-core crate policy (`promptforge/crates/promptforge-core/AGENTS.md`) permits verbatim compatibility re-exports and forbids new compatibility vocabulary, which the removals respect because none of the removed lines is a compatibility path; `vibe/archdoc-next.md` does not exist at the disposition ref and is not created.
- Open questions: None

## Functional Specification

The work is code motion and contract correction, not new behavior. The one externally observable behavior change is failure behavior at the VFS handle boundary: a backend session-open failure stops being a process panic and becomes a recoverable error that fails the run. The one security-relevant change is that model-controlled path volume can no longer grow host memory without bound.

- Actors and workflows: the executing agent applies unordered work items; maintainers review per-item diffs; CI validates the whole.
- Inputs and outputs: inputs are the ten source files, two rulebook/tool files, three committed ledger files, and the VFS crates; outputs are the same trees with the corrections applied, plus one promptforge commit removing the three ledger files.
- States and validation: `VfsPath` changes shape (Clone, not Copy); every other change is import motion, file removal, or documentation.
- Errors and recovery: `VfsRef::acquire` and `Access::spawn` return `Result<_, VfsError>`; the executor maps acquisition failure to `RunErrorKind::Store`; removed files remain recoverable from `cabinet/_trash/` and git history.
- Security and privacy behavior: the interner leak was reachable from model-controlled Lua (`store.write` with fresh names); after the fix, path strings are freed when their last owner drops.
- Acceptance criteria: the success criteria above, plus a green focused test scope per work item and a green workspace clippy.

</product-contract>
<implementation-contract>

## Technical Design

The only cross-module design is in `shared-vfs`: `VfsPath` becomes an `Arc<str>`-backed value with no global table, and the handle's acquisition boundary becomes fallible to match the trait's documented contract. Everything else is local: import motion into test modules, one crate removal, one visibility correction, and documentation patches.

- Architecture: `VfsPath` holds `Arc<str>`; `canonicalize` allocates one `Arc<str>` per call; claims tables hold clones; the string frees when its last owner drops. Sharing is per value lineage (clones share one allocation), not per string content (no dedup table). No process-global state, no lock, no eviction machinery; the u32-exhaustion panic disappears with the interner.
- Modules and interfaces: `promptforge/crates/shared-vfs/src/path.rs` loses the `Interner`, the `OnceLock<Mutex<..>>` global, and gains the `Arc<str>` field; `promptforge/crates/shared-vfs/src/handle.rs` makes `VfsRef::acquire`, `acquire_with`, and `Access::spawn` return `Result<_, VfsError>`; `promptforge/crates/shared-vfs/src/router.rs` already propagates backend acquisition failure and needs no contract change; `promptforge/crates/shared-vfs/src/lib.rs` fixes the manifest test's section matching.
- File and public API changes: `VfsPath` loses `Copy`; `VfsPath::as_str` returns `&str` borrowed from self instead of `&'static str`; trait signatures already take `&VfsPath`, so backends are untouched; `VfsRef::acquire` and `Access::spawn` gain `Result`; the `promptforge-bashkit` crate leaves the workspace; `promptforge/crates/gateway-local/src/artifacts.rs` changes `mod confine;` to `pub(crate) mod confine;` so `cache.rs` tests reach `source_marker_path` by its real path.
- Data, persistence, failure, security, and privacy constraints: no persisted or wire format changes; the panic-to-error change alters failure behavior only in a path no current backend can reach (both shipped backends have infallible acquisition); the interner removal alters memory behavior only upward (bounded by live owners instead of by history).

</implementation-contract>
<verification-contract>

## Testing Plan

Each work item carries a focused check; the shared-vfs changes add new regression tests; exit is the union of the per-crate suites and workspace lints. No test behavior changes except the two new regression tests and the updated interner property test.

- Unit: new regression test in shared-vfs - a stub backend whose `acquire` returns `Err` fails acquisition with a `VfsError` instead of panicking; the manifest test gains a `[dependencies.foo]` sub-table row; the `identical_paths_intern_to_one_entry` test's pointer-equality assertion becomes content equality; a loop canonicalizing distinct paths is asserted not to grow the heap monotonically.
- Integration and end-to-end: the claims conflict tests (`two_writes_by_two_identities_conflict`, the copy/rename claim matrix) and the fanout suite in promptforge-core prove the de-interned path still keys the claims model; the executor suite proves acquisition failure surfaces as `RunErrorKind::Store`.
- Regression, security, and performance: the heap-growth check covers the model-reachable leak; no performance criterion - claim lookups move from integer compare to short-string hash, unmeasured because the rate is one per VFS operation.
- Exit criteria: `cargo check`, `cargo clippy --all-targets`, and `cargo test` green for `promptforge-core`, `promptforge-lua`, `gateway-config`, `gateway-local`, `workshop-server`, `shared-vfs`, `promptforge-vfs`, `promptforge-store`, and `promptforge-agent`; `cargo metadata --locked` and `cargo build` green at workspace root; a grep of `tools-public/coding/vibe-coder.md` confirms no instruction stages or commits a ledger file.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Coarse steps over fine steps: the re-export removals are one behavior slice verified by one test scope, not one commit per file; the whole run targets about five commits. The user's words: "15 steps is way too many individual commits ... more steps than the previous plan which added a whole feature".
  - Remove `crates/promptforge-bashkit/` rather than relocating or excluding it: the spike's deliverable was evidence already recorded in the run ledger. The user's words: "remove the crate (c)".
  - De-intern `VfsPath` to an `Arc<str>`-backed value with no global table (option A): matches the intended ownership semantics with the least machinery; the integer-compare optimization it discards was never measured. The user's words: "why do we intern strings? that's not what I wanted. I suggested shared strings, but an owner would have to exist. once the last claim which shares a string goes away then we dont need the shared string anymore" and "I lean A".
  - No content-level dedup: unmeasured need, and dedup can be added later as a purely internal change behind the same `Arc<str>` shape. The user asked "should we implement the dedup?" and did not object to the negative recommendation.
  - Keep `Arc<str>` rather than `Box<str>`: equal size, cheaper clones at per-operation claim sites, `Send + Sync` (claims ride with `Access` into `spawn_blocking`), and exactly the requested shared-with-owner semantics. The user asked "do we even need the Arc then" and did not object to the recommendation to keep it.
  - Make `VfsRef::acquire` and `Access::spawn` fallible (option a): about six production call sites after the bashkit removal; the trait already documents the error and the router already propagates it. The user's words: "what do you think we should do? there's almost no callers", then "3. A".
  - Fix the stale `store` component line in `promptforge/vibe/archdoc.md` and add the missing VFS layer line. The user's words: "fix archdoc.md".
  - Patch `tools-public/coding/vibe-coder.md` so the ledger is scratch and never staged: the 2026-09-08 rewrite regressed the original scratch-ledger convention. The user's words: "can we fix vibe-coder.md to not generate these things in the repo?"
  - Remove the three committed ledger files from promptforge. The user selected "Patch the tool and remove the three committed files".
  - Patch `tools-public/rulebooks/rust-rulebook.md` section 11 with the test-import rule, detection entry, and correction pair: the rulebook sanctioned the test glob without constraining who may feed it. The user's words: "does rust-rulebook.md need a patch to prevent this?"
- Rejected alternatives:
  - Refcounted global interner with eviction (DEBT-VFS-02 option B): keeps process-global state, a lock on every `canonicalize`, and eviction bookkeeping. Revisit only if profiling shows `canonicalize` allocations hot; option A does not foreclose it.
  - Per-owner interner (DEBT-VFS-02 option C): overlays mount one handle as another's backend, so paths from two tables mix in one claims flow; cross-table identity is a real correctness risk. No revisit condition identified.
  - Narrow the `Vfs::acquire` contract to must-not-fail and keep the panic (DEBT-VFS-03 option b): cements the contradiction and forces the anticipated SQLite backend into infallible-acquire contortions. Revisit only if fallible acquisition proves unaffordable at a future boundary.
  - Relocate the bashkit spike to `spikes/` or exclude it in the root manifest (DEBT-VFS-01 options a and b): both keep dead weight whose evidence is already recorded. Revisit if the adapter regains a consumer.
- Assumptions, risks, and notes:
  - The CI failure from the bashkit path dependency is inferred from cargo's probed resolution behavior, not an observed run; the mechanism is certain on any sibling-less machine.
  - Debt evidence came from a two-pass collection (analysis, then independent challenge) over the twelve target commits; the challenger upheld all four accepted findings and all twelve rejections.
  - Rejected debt candidates, recorded so they are not re-litigated: claim granularity for glob/list/grep matches the designed per-path contract; overlays lose `read_range` push-down (performance only); `Ask` collapses to `PermissionDenied` (recorded v1 decision); `write_owned` deferred with no caller; host-backend stage-1 containment limits explicitly scoped; nested-handle double claim registration never self-conflicts; plan-mode copy refusal is conservative and visible; a cancelled run's in-flight store op completes (bounded, documented); anchor-replace duplication shows no drift; agent VMs keep inline store closures (single identity); process-global tables beyond the interner show no independent contradiction.
  - `promptforge/vibe/archdoc-next.md` does not exist at the disposition ref; no queue records needed resolution.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (builds only the gateway, the default workspace member; `cargo build -p workshop` for the desktop app, or `cargo workshop` for the one-command staged Workshop build)
- Focused test command pattern: `cargo test -p <crate> <test-name-substring>` or `cargo nextest run -p <crate> <filter>` (CI uses e.g. `cargo test -p gateway-stt --test it architecture`)
- Component test command pattern: `cargo nextest run --locked -p <crate>` (add `--all-features` where the crate gates features; gateway race tests use `cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <name>`)
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server` plus `cargo test --doc -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop crates: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`); supply-chain: `cargo deny check` and `cargo audit`
- Formatter check command: `cargo fmt --all --check`
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server`; user guide: `mdbook build guide`
- Test placement and naming conventions: unit tests live inline in source files under `#[cfg(test)] mod tests`; integration tests are a single `it` target rooted at `crates/<crate>/tests/it/main.rs` with one module per area (e.g. `tests/it/boot.rs`, nested `tests/it/realtime_stt/*.rs`), fixtures under `tests/fixtures/` and shared helpers under `tests/common/`; a few crates use standalone `tests/<name>.rs` targets instead. Test functions are long snake_case sentences (e.g. `a_direct_launch_recovers_the_lease_from_a_terminated_owner`). Ordinary `cargo test` must stay fully offline (no downloads, network, external processes, models, or credentials). UI packages under `crates/*/ui` test with `npm test`; Node helper scripts in `tools/` have sibling `*.test.mjs` files. Benchmarks use criterion (dev-only).
- Directory map: `crates/` holds all 37 Rust workspace members plus `shared-ui` (a TypeScript+CSS package excluded from the Cargo glob); `guide/` is the mdbook user guide (four doc sets: Workshop, gateway, prompt language, agent programs); `design/` holds design notes; `tools/` holds Node.js helper scripts (gateway sidecar staging, TTS live checks); `prompts/` holds prompt files; `vibe/` holds session plans, the ledger, and `archdoc.md`; `images/` holds docs art; `local/` holds local config; `.github/workflows/` holds CI (ci.yml, release, nightly, guide); `.config/nextest.toml` configures nextest; `clippy.toml`, `rustfmt.toml`, `deny.toml`, `dist-workspace.toml`, `rust-toolchain.toml` pin tooling at the root.
- Component boundaries (per `vibe/archdoc.md` and `AGENTS.md`): the executor product (`promptforge`, `promptforge-core`, `promptforge-core-support`, `promptforge-parser`, `promptforge-lua`, `promptforge-agent`, `promptforge-store`, `promptforge-vfs`, `promptforge-tools`, `promptforge-bashkit`, `promptforge-webfetch`, `promptforge-web-search`, `promptforge-model-client`, `promptforge-tool-picker`) parses and runs prompt pipelines and Lua agent programs; the gateway product (`gateway`, `gateway-config`, `gateway-config-ui`, `gateway-local`, `gateway-logging`, `gateway-routing`, `gateway-stt`, `gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-whisper-ffi`, `gateway-web-search`) owns model routing, provider credentials, and local inference; the workshop product (`workshop`, `workshop-server`) is the Tauri desktop shell and in-process server; the shared substrate (`shared-vfs`, `shared-loopback`, `shared-progress`, `shared-protocol`, `shared-sidecar`, `shared-ui`) depends on no product crate. Four dependency rules bind: gateway crates cannot depend on workshop crates; promptforge crates cannot depend on gateway or workshop crates; gateway crates cannot depend on promptforge crates; workshop crates cannot depend on gateway crates. `product-integration-tests` exercises cross-product behavior; `build-*` crates are build tooling.
- Conventions summary: Rust edition 2024 with workspace lint tables in the root `Cargo.toml` (`unsafe_code` forbidden, clippy `all` denied and `pedantic` warned, `unwrap_used`/`expect_used` denied, rustdoc link lints denied); lints live in manifest tables, not attributes. Behavior changes ship with tests in the same change; structural enforcement tests require explicit user approval. No clap - binaries hand-roll argv parsing. `Result` for expected failures; library and serve paths never exit the process or install process-global state. Comments explain non-obvious constraints and cite upstream issue URLs for workarounds. Long-running work reports through `shared-progress`. Unsafe stays in its owned boundary (`gateway-whisper-ffi`) with documented invariants. Cargo features gate real constraints (toolchain, native builds), not product shape. Builds must not dirty the git tree (CI enforces a clean-tree check). UI bundles build into `OUT_DIR` via crate build scripts driven by `npm ci`.

</project-survey>
<execution-plan>

## Execution Instructions

Components in dependency order, re-decomposed after the operator rejected the fine-grained layout ("way too many individual commits ... more steps than the previous plan which added a whole feature"):

- `tooling` (steps 1-2): the rust-rulebook and vibe-coder patches. Both landed out of band in the tools-public repository as `3705831` before the run started, so both steps are complete at seed time and the patched rules govern this run.
- `cleanup` (steps 3-4): the workspace-wide test-namespace cleanup as one behavior slice verified by one test scope, then the removal of the three committed ledger files. Placed before the VFS work so later commits stay free of ledger staging.
- `vfs-debt-removal` (steps 5-7): the bashkit removal, then the shared-vfs contract corrections as one slice (de-interning, the manifest test bypass, and the archdoc correction whose wording depends on de-interning), then fallible acquisition, whose caller set assumes the bashkit adapter is gone.

Each step is one commit containing its code and tests.

<step-1>

### Step 1: add the test-import rule to rust-rulebook.md [completed]

- Component: tooling
- In `tools-public/rulebooks/rust-rulebook.md` section 11: add the rule that test imports live inside the test module (with the same-module `#[cfg(test)]` code carve-out), the detection entry for `#[cfg(test)]` imports or re-exports whose only consumers are test modules, and the correction pair showing the move into `mod tests`.
- Landed out of band: applied directly and committed in the tools-public repository as `3705831` before the run started.

</step-1>

<step-2>

### Step 2: make the vibe ledger scratch in vibe-coder.md [completed]

- Component: tooling
- In `tools-public/coding/vibe-coder.md`: allocate one scratch `vibe-ledger.md` per run in the Per-Step Cycle preamble (keyed by the active plan's vibe name, never staged); make Resume Recovery read the scratch ledger only when it survives, with plan marks and git log authoritative otherwise; make the Mark step append to the scratch ledger and stage only the plan marker; align the human-facing preamble ("the audit ledger is scratch, never committed").
- Landed out of band: applied directly and committed in the tools-public repository as `3705831` before the run started.

</step-2>

<step-3>

### Step 3: remove test-only re-exports workspace-wide and relocate test imports [completed]

- Component: cleanup
- One behavior slice across five crates: delete every `#[cfg(test)]` import or re-export whose only consumers are test modules, and let each consumer import the name directly from its real home. The `execute.rs` and `execute/tests/mod.rs` portion is already applied in the worktree (coded before the re-decomposition); this step completes the remaining files and lands the whole slice as one commit.
- In `promptforge/crates/promptforge-core/src/execute.rs`: delete the `#[cfg(test)]` re-export block and the stray `ModelSet` re-export, change `pub(crate) use context::RunContext;` to a private `use`, and change `pub(crate) mod scheduler;` to `mod scheduler;` (already applied). In `promptforge/crates/promptforge-core/src/execute/tests/mod.rs`: the relocated imports, merged into existing use lines (already applied). The thirteen child test files keep their existing globs.
- In `promptforge/crates/promptforge-core/src/lua.rs`: delete the `#[cfg(test)]` re-exports of `Compactor`, `Conflict`, and `ToolRuntime`; point the consumers (`execute/tool_loop.rs`'s test-only wrapper signature, `execute/tests/tool_loop.rs`, `execute/tests/tool_scoping.rs`) at `promptforge_lua` directly.
- In `promptforge/crates/promptforge-core/src/client.rs`: delete the `#[cfg(test)]` `ToolSchemaError` re-export; the test module of `promptforge/crates/promptforge-core/src/error.rs` imports `promptforge_model_client::client::ToolSchemaError` directly.
- In `promptforge/crates/promptforge-core/src/model.rs`: delete the `#[cfg(test)]` `ModelInvocation` re-export; the five consumer test files (`execute/tests/scheduler.rs`, `execute/tests/input.rs`, `execute/tests/models_loop.rs`, `model/tests/mod.rs`, `lua/coro_tests.rs`) import `promptforge_model_client::model::ModelInvocation` directly.
- In `promptforge/crates/promptforge-core/src/cancel.rs`: delete the `#[cfg(test)]` `scope` re-export; the three consumer test files import `promptforge_core_support::cancel::scope` directly and call `scope(` instead of `cancel::scope(` at six sites.
- In `promptforge/crates/promptforge-lua/src/lib.rs`: delete the `#[cfg(test)] pub(crate) use vm::{LuaOutcome, run_chunk};`; `promptforge/crates/promptforge-lua/src/tests.rs` extends its existing `use crate::vm::LocalTools;` to include both names.
- In `promptforge/crates/gateway-config/src/config.rs`: delete the `#[cfg(test)]` `interpolate` re-export (the non-test `interpolate_value` re-export stays); `config/tests.rs` adds `use super::interpolate::interpolate;`.
- In `promptforge/crates/gateway-local/src/artifacts.rs`: delete the `#[cfg(test)] use archive::extract_archive;` and the `#[cfg(test)] pub(crate) use confine::source_marker_path;`; change `mod confine;` to `pub(crate) mod confine;`; `artifacts/tests.rs` extends its existing `super::archive` import with `extract_archive` and adds `use super::confine::source_marker_path;`; `cache.rs` changes its test import to `crate::artifacts::confine::source_marker_path`.
- In `promptforge/crates/workshop-server/src/heartbeat.rs`: move the `#[cfg(test)] use crate::gateway::GatewayClient;` inside the inline `mod tests`.
- Verification: `cargo check`, `cargo clippy --all-targets`, and `cargo test` green for `promptforge-core`, `promptforge-lua`, `gateway-config`, `gateway-local`, and `workshop-server`; a grep finds no `#[cfg(test)]` import or re-export in a non-test module whose only consumers are test modules.

</step-3>

<step-4>

### Step 4: remove the three committed ledger files [completed]

- Component: cleanup
- In the promptforge repository: move root `vibe-ledger.md`, root `vibe-review.md`, and `vibe/vibe-ledger.md` to `cabinet/_trash/`, stating the recovery sentence for each, and commit the removal.
- Verification: `git status` clean; the commit touches only the tracked deletions (root `vibe-ledger.md` and `vibe/vibe-ledger.md`; `vibe-review.md` was never tracked and is moved to `cabinet/_trash/` alongside them).
- Component verification for `cleanup` (the survey's per-crate pattern cannot derive a cross-crate component target): `cargo check`, `cargo clippy --all-targets`, and `cargo test` for `promptforge-core`, `promptforge-lua`, `gateway-config`, `gateway-local`, and `workshop-server`.

</step-4>

<step-5>

### Step 5: remove the promptforge-bashkit crate [completed]

- Component: vfs-debt-removal
- Remove `promptforge/crates/promptforge-bashkit/` by moving it to `cabinet/_trash/` (stating the recovery sentence), regenerate `Cargo.lock`, and sweep remaining bashkit references in CI, guide, and READMEs.
- Verification: `cargo metadata --locked` and `cargo build` succeed with no sibling checkout beside the repository; no bashkit references remain.

</step-5>

<step-6>

### Step 6: de-intern VfsPath, close the manifest test bypass, and correct archdoc [completed]

- Component: vfs-debt-removal
- In `promptforge/crates/shared-vfs/src/path.rs`: remove the `Interner` and the `OnceLock<Mutex<..>>` global; give `VfsPath` an `Arc<str>` field so `canonicalize` allocates one `Arc<str>` per call and the string frees when its last owner drops. `VfsPath` loses `Copy`; `VfsPath::as_str` returns `&str` borrowed from self instead of `&'static str`. Update claim sites to hold clones, change the `identical_paths_intern_to_one_entry` property test's pointer-equality assertion to content equality, and add the regression check that a loop canonicalizing distinct paths does not grow the heap monotonically.
- In `promptforge/crates/shared-vfs/src/lib.rs`: extend the section matching in `the_manifest_declares_no_dependencies` so any header equal to or starting with `dependencies.`, `dev-dependencies.`, or `build-dependencies.` is a dependency table, with a regression row.
- In `promptforge/vibe/archdoc.md`: correct the `store` component line to name the facade and its dependency, and add a VFS layer line covering `shared-vfs` (canonical paths, claims, routing, memory and host backends) and `promptforge-vfs` (policy gate), both depending on none.
- Verification: the heap-growth check, the claims conflict tests (`two_writes_by_two_identities_conflict`, the copy/rename claim matrix), and the fanout suite pass; the manifest test fails with an injected `[dependencies.foo]` sub-table and passes without it; the archdoc component list matches the workspace's actual dependency directions.

</step-6>

<step-7>

### Step 7: make handle acquisition fallible [completed]

- Component: vfs-debt-removal
- Depends on step 5: the described caller set assumes the bashkit adapter (three acquisition call sites) is gone.
- In `promptforge/crates/shared-vfs/src/handle.rs`: make `VfsRef::acquire`, `acquire_with`, and `Access::spawn` return `Result<_, VfsError>`.
- Map acquisition failure to `RunErrorKind::Store` at the executor boundary and to the agent's error in `promptforge-agent`; adjust tests and doc examples mechanically.
- Add the regression test: a stub backend whose `acquire` returns `Err` fails acquisition with a `VfsError` instead of panicking.
- Verification: the failing-backend regression test and the executor and agent suites pass.

</step-7>

- Deferred and out of scope: `write_owned` (deferred pending profiling, no caller); content-level dedup (rejected, revisit on measurement); the twelve rejected debt candidates; `#[cfg(test)]` helpers whose consumers live in the same module (legitimate, stay); the facade re-exports (deliberate, stay); historical run plans under `promptforge/vibe/` that reference the removed ledger files (dated records, stay).
- Exit criteria: per-step verification above, then the Testing Plan exit criteria - `cargo check`, `cargo clippy --all-targets`, and `cargo test` green for `promptforge-core`, `promptforge-lua`, `gateway-config`, `gateway-local`, `workshop-server`, `shared-vfs`, `promptforge-vfs`, `promptforge-store`, and `promptforge-agent`; `cargo metadata --locked` and `cargo build` green at workspace root; a grep of `tools-public/coding/vibe-coder.md` confirms no instruction stages or commits a ledger file.
</execution-plan>
