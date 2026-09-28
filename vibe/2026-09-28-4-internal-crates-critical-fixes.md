---
name: Internal crates critical fixes
overview: "Fix the three critical defects found in the promptforge-internal review (cancellation swallowed by pcall, an out-of-bounds panic when a walk ends with a live model task, and host-backend symlink operations acting on link targets), and sweep up a set of small, mechanical fixes: dead scheduler state, defensive arithmetic, the _G guard lock, dead re-exports, stale docs and comments, and two flat-directory folds."
todos:
  - id: lua-cancel
    content: "Lua cancellation: cancel_requested capture; re-raise in pcall, xpcall, local handler call, compactor call; tests"
    status: pending
  - id: chain-section-label
    content: "Engine: record entered section name on Chain; section_name reads it; walk-end and between-sections notice tests"
    status: pending
  - id: host-symlinks
    content: "VFS host: no-follow resolution for remove, exists, stat, mkdir, rename; Windows directory links; symlink tests"
    status: pending
  - id: scheduler-cleanup
    content: "Engine: delete dead call stack; saturating slot release; const nz_*; concurrent-call test"
    status: pending
  - id: lua-guard-lock
    content: "Lua: __metatable on _G guard metatables in argv.rs and prose.rs; guard test (resolved by a separate change, not a step of this plan; see the Decision Record)"
    status: completed
  - id: dead-items
    content: Remove NormalizedTurn non_exhaustive, unused StreamDelta and metrics re-exports; rename promptforge-api- temp prefixes
    status: pending
  - id: docs-sweep
    content: Engine AGENTS/README/lib.md/Cargo description; types, lua, model-client, container README; new vfs README; root AGENTS.md line 55
    status: pending
  - id: comments-sweep
    content: Fix stale comments and remove history/audit-tag comments across engine, vfs, model-client, types, parser
    status: pending
  - id: dir-folds
    content: Fold engine run-* into execute/run/ and recording-* into test_support/recording/
    status: pending
  - id: verify
    content: AGENTS.md gates, cargo deny, xtask api --check unchanged, facade docs build
    status: pending
isProject: false
---

# Internal crates: critical fixes and safe sweep

<product-contract>

## Product Requirements

A review of `crates/promptforge-internal/` at `8622b227` found three defects that can hang a host, crash it, or delete a file on disk. This plan fixes all three, each with a regression test that drives the exact failing path, and sweeps up small fixes that are mechanical or clearly correct and need no design decision. Everything the review found that needs a decision or real design work is listed under Deferred and Out of Scope.

- Problem and users:
  - Hosts and operators. A cancelled run whose Lua code loops inside `pcall` without yielding never returns from `Run::step`: the cancellation hook raises an ordinary `mlua::Error::RuntimeError` (`crates/promptforge-internal/lua/src/hardening.rs`, lines 145 to 148), and the shim's `pcall` and `xpcall` replacements return every caught failure without re-raising (`crates/promptforge-internal/lua/src/__impl_coro.lua`, lines 74 to 87). The instruction budget is effectively unlimited (`crates/promptforge-internal/lua/src/lib.rs`, line 69), so cancellation is the only way to stop a runaway loop.
  - Hosts. `Run::step`, documented as infallible, panics when a walk runs off its last section while the chain owns a live model-origin task. `end_section` advances `chain.index` past the section (`crates/promptforge-internal/engine/src/execute/scheduler/walk.rs`, line 230), the walk calls `finish` (lines 150 and 192), `finish` settles owned tasks (`scheduler/chain.rs`, line 175), abandonment queues a model-task notice (`scheduler/tasks.rs`, line 471), and `queue_task_notice` calls `section_name()` (`scheduler/notices.rs`, line 72), which indexes `slice[self.index]` out of bounds (`scheduler.rs`, line 383). The same lookup reports a notice under a section the owner has not entered when a model task ends between sections (`scheduler/tasks.rs`, line 346).
  - Operators using the host backend. In rooted mode every operation resolves through `contain`, which canonicalizes the full path when it exists (`crates/promptforge-internal/vfs/src/host.rs`, lines 131 to 133 and 392 to 399). Canonicalization follows a symlink at the final component, so `remove` (lines 494 to 516) deletes the link's target, and `rename`, `stat`, `exists`, and `mkdir` act on the target too. A link pointing outside the root cannot be removed at all. The trait contract says remove acts on the link, never the target (`crates/promptforge-internal/vfs/src/traits.rs`, lines 167 to 169), and the host code already uses `symlink_metadata` expecting link semantics (`host.rs`, lines 503 to 505, 520 to 528, 575 to 576).
- Goals:
  - Cancellation always unwinds a Lua block, through any number of `pcall` or `xpcall` layers.
  - No chain-end path indexes past its slice; task notices and other reports carry the section the chain most recently entered.
  - Host-backend operations that act on a path itself act on a symlink as a link; operations that act on contents follow links and keep the containment check.
  - The small fixes in Functional Specification land with no change to the facade's public API listing.
- Non-goals:
  - Any item under Deferred and Out of Scope.
  - Changes to `crates/promptforge/public-api.txt`.
  - Splitting oversized files or enabling the 500-line check.
- Success criteria:
  - A run cancelled while its block runs `while true do pcall(function() while true do end end) end` returns from `Run::step` with a cancelled outcome; the same holds for `xpcall`, a local tool handler that loops, and a compactor that loops.
  - A prompt whose last section falls through while a model-origin task is live finishes without a panic, reports `TaskAbandoned`, and reports the notice under that last section.
  - On the host backend, `remove("/link")` removes the link and leaves the target intact; a link to a path outside the root can be removed, stat'ed, checked for existence, and renamed; reading or writing through a link that escapes the root is still refused.
  - Every check in Testing Plan passes and the facade listing is unchanged.
- Constraints:
  - The engine performs no I/O and adds no effect kind or suspension point.
  - Repository rules in `AGENTS.md` apply: comments state only non-obvious constraints, behavior changes ship with tests, errors state required versus actual.
  - `promptforge-vfs` stays dependency-free (`crates/promptforge-internal/vfs/src/lib.rs`, line 191).
- Open questions: None

## Functional Specification

Cancellation becomes uncatchable by author code while the cancel flag is set. Chains remember the name of the section they last entered, so reports never need the current walk position to be valid. The host backend gains a no-follow resolution for operations on the path itself. The sweep is small, independent edits.

- Actors and workflows:
  - Author Lua under cancellation: any `pcall` or `xpcall` whose protected call fails while the run's cancel flag is set re-raises the failure instead of returning it. The shim's internal protected calls do the same: the local tool handler call (`__impl_coro.lua`, line 129) and the compactor call (line 208). The block guard (line 339) stays the boundary that returns the failure to Rust.
  - Scheduler reporting: `section_name()` returns the prompt title during the live H1 pass, and otherwise the name of the section the chain most recently entered. Before a walking chain enters any section, it returns the name of the slice's first section, or the prompt title when the slice is empty. `section()` and `blocks()` keep indexing the live section and are only called with a live frame.
  - Host backend: two resolutions. Follow resolution, which is today's `contain`, is used by `read`, `read_range`, `write`, `append`, `list`, `glob`, and `copy`. No-follow resolution canonicalizes only the parent and appends the final component unchanged, still requiring the parent to sit under the root. It is used by `remove`, `exists`, `stat`, `mkdir`, and both sides of `rename`. The mounted root `/` resolves to the root in both.
  - On Windows, removing a directory symlink uses `fs::remove_dir` rather than `fs::remove_file`, because `remove_file` fails on a directory link.
- Inputs and outputs:
  - Cancelled outcome: an uncaught cancellation still maps to the run's cancelled result at the VM failure boundary, as today.
  - Task notices and `TaskAbandoned` events for a chain between sections, or past its last section, carry the section it last entered.
- States and validation:
  - The shim obtains the cancel state through a new prelude capture backed by `InstructionBudget::is_cancelled` (`crates/promptforge-internal/lua/src/hardening.rs`, line 123).
  - The recorded section name is set when `enter_section` enters a section and survives `end_section` and `finish`.
- Errors and recovery:
  - The containment check for follow resolution is unchanged: a read or write whose final target escapes the root is refused with the existing "escapes the mounted root" denial.
  - No-follow resolution refuses a path whose parent escapes the root with the same denial.
- Security and privacy behavior:
  - A cancelled run can no longer keep a host thread busy indefinitely.
  - Removing or renaming a link never touches the target, inside or outside the root.
  - `_G`'s guard metatable carries `__metatable`, so `setmetatable(_G, ...)` can no longer strip the frozen `argv` and read-only `prose` guards, matching every other guard in the crate (`crates/promptforge-internal/lua/src/sys.rs`, line 93; `crates/promptforge-internal/lua/src/proxy.rs`, line 26).
- Acceptance criteria:
  - Every Success criterion holds and every Testing Plan check passes.

</product-contract>
<implementation-contract>

## Technical Design

All three critical fixes are local to one crate each: the Lua shim and its prelude captures, the scheduler's chain state, and the host backend's path resolution. The sweep touches documentation and comments across the six crates, a few dead items, and two directory layouts. No public facade item changes.

- Architecture:
  - Lua: cancellation is re-raised in Lua, at the only places author and shim code can catch errors, so no Rust-side error type needs to change.
  - Engine: the section label becomes chain state instead of a derived lookup, which removes the dependency on a valid walk position from every report path.
  - VFS: path resolution splits by intent, so each operation picks link or target semantics explicitly.
- Modules and interfaces:
  - `crates/promptforge-internal/lua/src/coro.rs`: `install_shim_prelude` (line 156) passes a `cancel_requested` capture reading the VM's `InstructionBudget`. `crates/promptforge-internal/lua/src/__impl_coro.lua`: `protected_call` and `protected_xcall` (lines 74 to 87), the local handler call (line 129), and the compactor call (line 208) re-raise when it returns true.
  - `crates/promptforge-internal/engine/src/execute/scheduler.rs`: `Chain` gains the recorded section name; `section_name()` (lines 397 to 405) reads it. `crates/promptforge-internal/engine/src/execute/scheduler/walk.rs`: `enter_section` sets it.
  - `crates/promptforge-internal/vfs/src/host.rs`: a no-follow sibling of `contain` (lines 123 to 150) and of `HostAccess::resolve` (lines 392 to 399); `remove`, `exists`, `stat`, `mkdir`, and `rename` switch to it.
- File and public API changes:
  - No facade item changes; `crates/promptforge/public-api.txt` stays identical.
  - Deleted: the scheduler's `stack` field and its uses (`scheduler.rs`, lines 429 to 431 and 499; `scheduler/dispatch.rs`, line 314; `scheduler/chain.rs`, lines 208 to 212 and 254 to 258; `scheduler/drive.rs`, line 199). Its only readers are a `clear`, a `debug_assert` pop, and a conditional pop, so nothing decides from it.
  - Moved: the engine's `run-effect.rs`, `run-effect-tests.rs`, and `run-tests.rs` into `crates/promptforge-internal/engine/src/execute/run/`, and `recording-forward.rs`, `recording-forward-tests.rs`, and `recording-observation.rs` into `crates/promptforge-internal/engine/src/test_support/recording/`, per the three-file directory rule in `AGENTS.md`.
  - New: `crates/promptforge-internal/vfs/README.md`.
- Data, persistence, failure, security, and privacy constraints:
  - No log, replay, or wire format changes.
  - Release builds can no longer wrap the task slot counter: `slots_used -= 1` (`scheduler/tasks.rs`, line 591) becomes a saturating subtraction, and the `debug_assert` above it stays.

</implementation-contract>
<verification-contract>

## Testing Plan

Each critical fix gets a regression test that reproduces the exact failing path and fails before the fix. The sweep relies on the existing suites, the unchanged facade listing, and the docs build. Symlink tests run on Unix and on Windows when symlink creation is permitted.

- Unit:
  - Lua, under a set cancel flag: a block running `while true do pcall(function() while true do end end) end` ends with the cancelled error; the same with `xpcall` and a message handler; a local tool handler that loops ends the block; a compactor that loops ends the block. A `pcall` that catches an ordinary error while the flag is clear still returns `false, err`.
  - Lua: `setmetatable(_G, nil)` and `getmetatable(_G)` no longer expose or remove the `argv` and `prose` guards; assigning `argv` outside H1 is still refused afterwards.
  - VFS host backend: remove a link to a file inside the root and assert the target survives; remove, stat, exists, and rename a link whose target is outside the root; reading and writing through that link are still refused; remove a dangling link; on Windows, remove a directory link. Windows tests return early only when symlink creation fails with the privilege error, and say so in the test name or message.
- Integration and end-to-end:
  - Engine: a prompt whose last section runs `tools.allow_tasks()`, lets the model start a task through the canned model, and falls through while the task is live. It asserts no panic, a `TaskAbandoned` event, and a task notice under that section's name.
  - Engine: a model task that ends while its owner sits between two sections reports its notice under the section just ended.
  - Engine: two concurrently admitted tasks that each `call` a section issuing a chat, answered in reverse issue order, complete in a debug build.
- Regression, security, and performance:
  - `cargo xtask api --check` reports no difference from `crates/promptforge/public-api.txt`.
  - The existing symlink escape tests in `crates/promptforge-internal/vfs/src/host.rs` (around line 787) still pass.
- Exit criteria:
  - The `AGENTS.md` verification commands (`AGENTS.md`, lines 49 to 55).
  - `cargo deny check`, which CI runs (`.github/workflows/ci.yml`, lines 340 to 341).
  - The facade docs build that CI gates, which catches broken intra-doc links from the file moves.

</verification-contract>
<decision-record>

## Decision Record

The critical fixes are the smallest changes that close each failure path at its root. The sweep is limited to items that are mechanical or clearly correct, change no public API, and need no design call. Anything that alters behavior authors or hosts could depend on, or that needs a decision, is deferred.

- Decisions:
  - Fix the three critical defects. User's words: "Lets plan to fix the critical items and also sweep up the little fixes which you think are safe".
  - Re-raise cancellation in the Lua shim when the cancel flag is set, instead of making the hook error uncatchable in Rust. Rationale: the shim already owns every `pcall` and `xpcall` author code can reach, and the flag check is one capture.
  - Record the entered section name on the chain instead of guarding each `section_name()` caller. Rationale: it fixes both the out-of-bounds panic and the notice mislabeled between sections, and no future caller can reintroduce the bug.
  - Split host-backend resolution into follow and no-follow instead of special-casing `remove`. Rationale: `rename`, `stat`, `exists`, and `mkdir` share the bug, and the existing `symlink_metadata` calls already expect link semantics.
  - Include the `_G` `__metatable` lock in the sweep. Rationale: every other guard in the crate already sets it, and without it both `_G` guards can be removed in one call. It does stop author code from replacing `_G`'s metatable, which nothing in the tree does.
  - Delete the scheduler's call stack rather than make it concurrency-aware. Rationale: nothing decides from it; it exists only to feed a `debug_assert` that fails when concurrent tasks each `call`.
- Rejected alternatives:
  - Raising cancellation as a Rust `mlua::Error::external` value that `pcall` cannot catch. Reason: Lua's `pcall` catches every error value, so this would not help. Revisit: never.
  - Returning a placeholder section name from `section_name()` when the index is past the slice. Reason: it would still mislabel notices between sections. Revisit: never.
  - Fixing only `remove`. Reason: `rename`, `stat`, `exists`, and `mkdir` would still act on targets. Revisit: never.
- Assumptions, risks, and notes:
  - Cancellation takes effect at the next hook firing, every 10,000 instructions (`crates/promptforge-internal/lua/src/lib.rs`, line 62); a loop that yields was already cancellable through the scheduler's teardown.
  - `fs::remove_dir_all` does not follow symlinks on current stable Rust, so a recursive remove of a directory containing links stays inside it.
  - Moving the `run-*` and `recording-*` files changes `#[path]` attributes and possibly intra-doc links; the docs build catches breakage.
  - The Lua review's findings on `var.x = nil`, empty-looking proxies, the unused `models.infer` path, and the duplicate store implementation were reported by one reviewer and not re-checked; they are deferred for that reason as well.
- Resolved on 2026-09-28 by a separate change outside this plan: the `_G` `__metatable` lock (todo `lua-guard-lock`), first raised here as an open decision by the project survey.
  - Resolution: option (c) below. `_G` now carries one guard metatable with `__metatable` set (`crates/promptforge-internal/lua/src/globals.rs` and `__impl_globals.lua`), serving `argv` and `prose` for every section. The sandbox's `setmetatable` and `getmetatable` are replacements that record an author's `_G` metatable behind the guard, so the documented pattern keeps working, both pinning tests pass unchanged, and no route in the sandbox removes or bypasses the guard. The guide passages are updated to match. This plan builds nothing for it.
  - The decision above to include the lock rests on nothing in the tree replacing `_G`'s metatable. The user guide documents doing exactly that: "Your own metatable on _G" in `guide/src/language/05-lua-environment.md` (line 210) shows `setmetatable(_G, { __index = defaults })` in the shared library, and `guide/src/language/03-blocks-and-prose.md` (line 722) shows the same pattern. Two tests pin it: `section_vm_host_injection_bypasses_shared_global_metatables` (`crates/promptforge-internal/lua/src/tests.rs`, line 1177) and `lazy_prose_composes_with_a_shared_library_metatable` (`crates/promptforge-internal/engine/src/execute/tests/suite/lazy_prose.rs`, line 227).
  - `inject_host_with_var` installs the frozen `argv` guard on `_G`'s metatable (`crates/promptforge-internal/engine/src/execute/section_vm.rs`, line 139) before `replay_shared` runs the shared library (line 163). With `__metatable` set on that guard, the documented `setmetatable(_G, ...)` raises "cannot change a protected metatable", the shared library fails to load, and both tests fail.
  - By reading the same code (not by running it), the documented pattern today replaces the `argv` guard in every non-H1 section, and the prose guard then delegates the `argv` lookup to the author's `__index`, so `argv` reads nil there in the guide's example. This is the pre-existing gap the lock was meant to close.
  - Options: (a) defer the lock together with the guard-composition question; (b) lock as specified and retire the documented pattern, rewriting both guide passages and both tests, an author-visible change; (c) keep author metatables and make them compose under the guards, for example a sandbox `setmetatable` that installs an author's `_G` metatable as the guards' delegate, which is new design. This Decision Record's opening rule defers anything authors could depend on, which favors (a).
  - No step of this plan builds the Functional Specification's `_G` bullet (under Security and privacy behavior) or the Testing Plan's `_G` unit test; the separate change built and tested both, so Decomposition need not be rerun for them.

### Deferred and Out of Scope

- Deferred: an optional capability that backs a tool slot refuses the run (`crates/promptforge-internal/engine/src/execute/fill.rs`, lines 42 to 54). Needs a decision on whether optional capabilities may back tool slots. Revisit first.
- Deferred: the engine's `test-support` feature and public `test_support` module (`crates/promptforge-internal/engine/Cargo.toml`, lines 25 to 38; `engine/src/lib.rs`, lines 11 to 12). Needs a decision on whether the feature removal was meant to include the engine.
- Deferred: `Environment::max_depth`, which does nothing but is on the facade (`crates/promptforge/public-api.txt`, line 420). Removing it changes the public API.
- Deferred: narrowing the engine's unused public items (`pub mod model`, `pub mod parser`, root re-exports, `StoreOp`, `StoreOutcome`), which touches doctests and a bench.
- Deferred: VFS claim gaps (nested subtrees, created parent directories, list against recursive remove), Windows case and stream aliasing, and glob and grep ignoring nested mounts. Revisit as one claims-model plan.
- Deferred: `Completion::from_result` and `ToolCall::from_parts` skipping live-path validation, and restructuring `ClientError`.
- Deferred: the cancellation waker leak in `crates/promptforge-internal/types/src/cancel.rs` (lines 100 to 141).
- Deferred: parser span offsets, the indented `---` frontmatter close, headings inside blockquotes and lists, and missing error locations.
- Deferred: the Lua findings on `var.x = nil`, proxy enumeration, the unused `models.infer` path, and the duplicate store implementation, after re-verification.
- Deferred: a dropped live timer that never wakes its waiter, answers ignored after the run is decided, `unwrap_or_else(|| panic!(..))` sites, SSE edge cases, the vfs manifest test's parsing holes, and plan mode's `.md`-suffix check.
- Deferred: splitting the oversized files and adding the `## Invariants` marker so the 500-line check applies. Revisit after the splits, so the check does not fail the build on arrival.
- Out of scope: the `product-*` and `listing-*` directory folds in `crates/build-xtask/src/`, which are outside the internal crates.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <package>`. Plain `cargo build` builds only the default member, `crates/gateway/app` (package `gateway`). The six internal crates (`promptforge-engine`, `promptforge-lua`, `promptforge-vfs`, `promptforge-model-client`, `promptforge-types`, `promptforge-parser`) and the `promptforge` facade build with no UI or native prerequisites. Crates whose build scripts bundle a UI into `OUT_DIR` (`workshop-server`, `gateway-config-ui`, and their dependents, which the workspace-wide runs include) need `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first. Before any `-p workshop` build, CI builds `cargo build --locked -p gateway --no-default-features` and stages it with `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe` (undo with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`). The clippy and full-suite runs build every member they check, so a separate workspace build adds nothing beside them, and a standalone `cargo check --workspace` never runs beside clippy. `.cargo/config.toml` aliases `cargo xtask` to `run -p build-xtask --` and `cargo workshop` to `run -p build-workshop --`, and links Windows builds with `rust-lld` and the static CRT.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <filter>`, where `<filter>` is one or more test-name or module-path substrings (nextest runs a test matching any of them), such as `host::tests`, `model_task_notices`, or `scheduler::concurrency`. The internal crates keep every test inside `src/` as unit-test modules and have no integration target; the `promptforge` facade's integration target is `--test suite`. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Nextest skips doctests, so a doc example needs `cargo test --locked --doc -p <package> --all-features`. `.config/nextest.toml` marks a test slow at 60 seconds and terminates it after three periods, 180 seconds.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test --locked --doc -p <package> --all-features` (the workshop trio without `--all-features`). The structural harness alone: `cargo test -p build-xtask`; its nightly-only fixtures: `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features`, `cargo nextest run --locked -p workshop-server --features headless`, the gateway process-ownership race tests, and the UI `npm test` runs. The workspace run includes `build-xtask`, the structural harness.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`, `cargo deny check`, and `cargo audit` (CI's `supply-chain` job; `cargo-deny` 0.20.2 and `cargo-audit` 0.22.2 are installed locally). Per-package pattern for scoped runs: `cargo clippy --locked -p <package> --all-targets --all-features -- -D warnings`. The UI typechecks (`npm run typecheck --workspaces --if-present` from `crates/workshop`, `npm run typecheck` from `crates/gateway/config-ui/ui`) apply only to UI changes. `.githooks/` holds a pre-commit hook (the formatter check) and a pre-push hook (the headless check, the workspace clippy, `cargo deny check`); neither is installed in this clone, since `core.hooksPath` is unset.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`). No UI formatter or JS linter is configured.
- Docs command: with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the facades alone with default features, `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`, then the engine's private items, `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`. These are CI's `docs` job; CI's `check-workshop` job also builds `cargo doc --locked --no-deps -p workshop-server --document-private-items`. Per-package pattern: `cargo doc --locked --no-deps --all-features -p <package>` under the same flag. The facade surface check is `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed locally), compared against the committed `crates/promptforge/public-api.txt`. User guide: `cargo xtask site --books-only`; the combined guide regenerates with `cargo run --locked -q -p build-user-guide`.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file `<module>-tests.rs`, wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (for example `engine/src/execute/requirements-tests.rs`, `lua/src/prelude-tests.rs`, `types/src/event-tests.rs`, `model-client/src/client/stream-tests.rs`). Small modules keep an inline `#[cfg(test)] mod tests {}` at the bottom (`engine/src/execute/config-limits.rs`, `parser/src/list.rs`), and `promptforge-vfs` uses inline blocks throughout (`vfs/src/host.rs` from line 645, whose helper `make_dir_link` makes a junction on Windows through `mklink /J` and a symlink on Unix). VFS tests return `Result<(), VfsError>`.
  - `promptforge-engine`'s behavior suite is `src/execute/tests/`, declared by `execute/tests.rs`: one file per area (`model_task_notices.rs`, `model_tasks.rs`, `local_tools.rs`, `models_loop_compactors.rs`, `tool_loop.rs`, `run_termination.rs`, and others) plus `scheduler/` (`concurrency.rs`, `walk.rs`, `failures.rs`, `fanout.rs`, `live_h1.rs`, `store_gate.rs`) and `suite/`. These tests drive whole runs through the `test_support` drivers against canned or scripted models. `cancel_during_in_flight_tool_call_returns_promptly` (`tool_loop.rs`, line 470) is the mid-run cancel pattern: a multi-thread tokio test, `TokioDriver::cancel_handle`, a cancel after 100 ms, and an `Err(crate::Error::Interrupted)` assertion within 5 seconds. `engine/tests/prompts/` holds fixture prompts, not a test target.
  - `promptforge-lua` keeps one large `src/tests.rs` beside its `<module>-tests.rs` siblings, `protocol/tests/`, and `tools/tests.rs`; shared helpers sit in `tests-recording.rs`. Its cancellation tests, `long_running_lua_block_cancels_cooperatively` and `a_pre_cancelled_run_aborts_a_tight_loop_promptly`, are in `tests.rs`. `prelude-tests.rs`'s `section_vm_with_var` builds a section VM with the scheduler control globals and coroutine shims installed, the setup under which the shim's `pcall` and `xpcall` replacements are live; no Lua-crate test drives the shim's local-tool or compactor paths, which the engine suites reach through the scheduler.
  - Integration targets: `tests/suite/` in the `promptforge` and `harness` facades, `tests/it/` in harness and workshop crates. Each `main.rs` opens with `#![expect(clippy::expect_used, clippy::unwrap_used, reason = ...)]`.
  - Shared fixtures live in `test_support` modules behind a `test-support` feature (engine, lua, parser, runner, sessions) or a `test-fixtures` feature (gateway and workshop crates).
  - Test functions are snake_case sentences stating the behavior, such as `a_rooted_backend_rejects_links_that_escape_the_mount_root`. `clippy.toml` allows `unwrap` and `expect` in tests; the workspace lints deny both elsewhere.
  - Benches: `engine/benches/models_loop.rs` and `lua/benches/surface.rs`, both requiring `test-support`.
- Directory map:
  - `crates/`: every Rust crate and UI package. Its root is the public layer: the `promptforge` facade (`src/`, `tests/suite/`, `public-api.txt`), the `harness` facade, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS, not a crate), the `build-*` tooling crates, and `workspace-hack`.
  - `crates/promptforge-internal/`: a manifestless container holding `engine` (`src/execute/` with `scheduler/` and `tests/`, `src/test_support/`, `src/lua/`, `src/model/`, `src/lib.md` as the crate doc, `benches/`, `tests/prompts/`), `lua` (the section VM, the Lua shims `__impl_coro.lua`, `__impl_fanout.lua`, `__impl_messages.lua`, `__impl_store.lua`, `__impl_tasks.lua`, plus `protocol/` and `tools/`), `vfs` (a flat `src/`, std only), `model-client` (`client/`, `model/`), `types` (`tools/`), and `parser` (`contract/`). Each crate has `AGENTS.md` and `Cargo.toml`; all but `vfs` have `README.md`; the container's `README.md` describes the six.
  - `crates/harness-internal/`, `crates/workshop/`, and `crates/gateway/`: the other family containers (the harness runner, models, capabilities, log, sessions, and web crates; the Workshop desktop app, server, and UI npm workspaces; the gateway app and its subsystems, including the nested `stt/`).
  - `guide/`: user guide chapter sources (`src/language/` among them), mdBook books, and the combined `guide/promptforge-language-guide.md`. `prompts/`: sample prompts. `tools/`: Node and Python maintenance scripts.
  - `vibe/`: `archdoc.md`, dated plan and run records (`YYYY-MM-DD-N-slug.md`), reference notes, and a gitignored `scratch/`.
  - `.github/workflows/ci.yml` (jobs `fmt`, `clippy`, `test`, `docs`, `check-workshop`, `check-workshop-linux`, `ui`, `supply-chain`, `api-surface`, and the aggregate `ci-green`), `.githooks/`, `.config/nextest.toml`, `.cargo/config.toml`, and at the root `Cargo.toml` (an explicit container member list; `default-members` is the gateway app), `rust-toolchain.toml` (stable), `clippy.toml`, `rustfmt.toml`, and `deny.toml`.
- Component boundaries:
  - `promptforge` is a facade of single-item re-exports over `crates/promptforge-internal/*` and the only promptforge crate that code outside the family may name. `promptforge-*` crates depend on no gateway, workshop, or harness crate; an internal crate may list `promptforge` as a dev-dependency only so its doc examples compile.
  - Inside the container dependencies run one way: `promptforge-vfs` depends on nothing, which its manifest test enforces; `promptforge-types` depends on no sibling; `promptforge-model-client` on types; `promptforge-lua` on model-client, types, and vfs; `promptforge-parser` on lua and types; `promptforge-engine` on all five. The engine performs no I/O and names tokio only behind `test-support`.
  - `harness` fronts `crates/harness-internal/*`, whose crates depend only on `promptforge`. Workshop crates may name `harness`, `promptforge`, the gateway public pair, and `shared-*`. Gateway private crates depend on no promptforge, harness, or workshop crate. `shared-*` crates depend on no product crate.
  - `cargo test -p build-xtask` enforces the topology, the `## Invariants` markers in workshop-* and harness-* crates, lint inheritance, and the 500-line ceiling in marker crates. None of the six internal crates carries the marker, so the ceiling does not bind them. `cargo xtask api --check` enforces the facade surface.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain. Workspace lints every member inherits: `unsafe_code = "forbid"`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn; clippy `all` and `pedantic` denied; `unwrap_used` and `expect_used` denied; broken and private intra-doc links denied, so removing an item a doc comment links to fails the docs build.
  - Source directories are flat: one or two files beside a parent module are `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`; at three they become a `foo/` subdirectory in standard layout with no path attributes, and they flatten back below three. At survey time `engine/src/execute/` holds three `run-*` siblings and `engine/src/test_support/` holds three `recording-*` siblings, both on the wrong side of the line.
  - Behavior changes ship with tests in the same change. No new structural enforcement without explicit user approval.
  - Error and status messages are written for a model reader: concise, self-contained, naming required versus actual.
  - Comments explain only a non-obvious constraint, ordering requirement, or workaround; every workaround cites its upstream issue URL.
  - JSON that reaches the run log round-trips exactly: sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Cargo features gate real constraints, not product shape. Library and serve paths return failures instead of exiting or installing process-global state.
  - CI passes `--locked` on builds and tests and fails when a build dirties the tree.

</project-survey>
<execution-plan>

## Execution Instructions

- Status: decomposed on 2026-09-28 against local `master` at `8622b227` ("Document the missing-service drop in Requirements::merge"), the commit the plan's review names, with a clean worktree. Every file, function, test, and line named below was found at its stated place at that commit; where the plan's earlier text was off, the step gives the corrected place.
- Path: Full. Four components, one step each, in dependency order. Each step's Todo line names the frontmatter todos it builds.
- Component order:
  1. **VFS host link semantics** (`host-symlinks`) - first. `promptforge-vfs` is the bottom of the dependency stack, and the later steps' Lua and engine suites run over it. It shares no file with any other step.
  2. **Lua cancellation** (`lua-cancel`) - second. `promptforge-lua` sits below the engine, so Step 3's engine suite runs over the fixed shim. Its two engine test additions touch no file Step 3 edits.
  3. **Engine scheduler** (`chain-section-label`, `scheduler-cleanup`) - third. Both todos edit the same scheduler files, and one test set covers both.
  4. **Internal crates sweep** (`dead-items`, `docs-sweep`, `comments-sweep`, `dir-folds`, `verify`) - last. Its docs describe the behavior Steps 1 to 3 leave, its comment edits sit in files Step 3 edits, and its directory folds move `execute/run-effect.rs`, `execute/run-effect-tests.rs`, `execute/run-tests.rs`, and the three `test_support/recording-*.rs` files, so the folds land after every step that could edit them.
- Standing rules for every step:
  - Each step is one commit holding its code, docs, and tests.
  - Line numbers refer to `8622b227`. Earlier steps move lines in files later steps edit (Step 3 in `scheduler/tasks.rs`, which Step 4 also edits); re-locate any reference by content before editing.
  - The run starts at `cd38ce8c`, two commits past `8622b227`. Those two commits, `c18672e0` ("Guard argv and prose behind one locked _G metatable") and `cd38ce8c` ("Refuse reserved names as tool aliases and role labels"), are the separate `_G` change. They edited `promptforge-lua` files (`argv.rs`, `prose.rs`, `hardening.rs`, `lib.rs`, `vm.rs`, `prelude.rs`, `globals.rs`, `__impl_globals.lua`, the Lua README and AGENTS.md) and some engine and parser tests, so Lua-crate line numbers can be off by a few lines; re-locate by content.
  - No type, signature, or doc change reaches `crates/promptforge/public-api.txt`.
  - Verification cadence: Steps 1 to 3 each end a component, so the run verifies each at COMPONENT scope (the formatter check, per-package clippy, and the component tests of the touched packages). Step 4 is the final step and is verified at FULL scope, which covers the Testing Plan's exit criteria: the `AGENTS.md` Verification commands (`AGENTS.md`, lines 70 to 78, not the lines 49 to 55 the Testing Plan cites), `cargo deny check` (CI's `supply-chain` job, `.github/workflows/ci.yml` lines 363 to 364, not lines 340 to 341), the facade docs build, and `cargo +nightly-2026-09-05 xtask api --check`.
  - Not done here: everything under Deferred and Out of Scope, and the `_G` lock, which a separate change resolved.
  - Writing: plain English, single dashes only, never em dashes or double dashes.

<step-1>

### Step 1: Act on host-backend symlinks as links [completed]

- Component: VFS host link semantics
- Piece: the no-follow resolution and its five callers, built jointly in one commit. The resolver cannot land alone, because a private function with no caller fails `dead_code` under the denied-warnings clippy run, and one test set covers all five operations.
- Todo: `host-symlinks`
- Depends on: nothing
- Read: Product Requirements, the third Problem bullet, the third Goal, and the third Success criterion; Functional Specification, the Host backend and Windows bullets, Errors and recovery, and the second Security bullet; Technical Design, the VFS bullets; Testing Plan, the VFS host backend bullet and the escape-test regression bullet; Decision Record, the split-resolution decision, its rejected alternative, and the `remove_dir_all` note; Project Survey.
- Build, in this order:
  - Tests first (see Tests). Confirm at `8622b227` that removing a link to an in-root file deletes the target, and that `remove`, `stat`, `exists`, and `rename` on a link whose target is outside the root fail with the "escapes the mounted root" denial.
  - `crates/promptforge-internal/vfs/src/host.rs`: add a no-follow sibling of `contain` (line 123). It resolves the candidate's parent through `contain`, so the parent's nearest existing ancestor must still sit under the root, then appends the final component unchanged. The mounted root itself, for which `join_virtual` yields the root, resolves to the root.
  - Add a no-follow sibling of `HostAccess::resolve` (line 392): identity mode returns the host path exactly as `resolve` does, and rooted mode calls the new function.
  - Switch `remove` (line 494), `exists` (line 518), `stat` (line 574), `mkdir` (line 580), and both `host_from` and `host_to` in `rename` (lines 616 and 617) to the no-follow resolver. `read`, `read_range`, `write`, `append`, `list`, `glob`, and `copy` keep `resolve`, and the escape denial text is unchanged.
  - `remove` on Windows: when `symlink_metadata` reports a directory link (`std::os::windows::fs::FileTypeExt::is_symlink_dir`, which a junction also satisfies), remove it with `fs::remove_dir`, because `fs::remove_file` fails on a directory link. Every other non-directory keeps `fs::remove_file`, and real directories keep today's `recursive` handling. The Windows branch sits behind `#[cfg(windows)]`, as the file's other platform code does.
  - The module doc (lines 1 to 16) states the two resolutions: operations on a path itself (`remove`, `exists`, `stat`, `mkdir`, `rename`) act on a final-component link as a link, and operations on contents follow links under the containment check. Keep the Stage 2 deferral for what stays deferred.
- Tests: in `host.rs`'s `mod tests`, beside `a_rooted_backend_rejects_links_that_escape_the_mount_root` (line 783):
  - A new helper `make_file_link` beside `make_dir_link` (lines 713 to 732): `std::os::unix::fs::symlink` on Unix, `std::os::windows::fs::symlink_file` on Windows. A test that needs a file link returns early only when Windows refuses with the privilege error (raw OS error 1314, `ERROR_PRIVILEGE_NOT_HELD`) and says so in an `eprintln!` message; any other creation failure fails the test.
  - Removing a link to a file inside the root removes the link, and the target keeps its bytes.
  - For a link whose target is a file outside the root: `exists` is true, `stat` reports `FileType::Symlink`, `rename` moves the link and leaves the target where it was, and `remove` removes the link while the outside target keeps its bytes. `read` and `write` through the link still fail with `PermissionDenied`.
  - Removing a dangling link succeeds.
  - Removing a directory link made with `make_dir_link` removes the link, and the target directory and its contents survive. The test asserts that the helper succeeded: on Windows it makes a junction, which needs no privilege, and exercises the new `fs::remove_dir` branch.
  - Focused command: `cargo nextest run --locked -p promptforge-vfs --all-features host::tests`.
  - Existing pins that must pass unchanged: `a_rooted_backend_rejects_links_that_escape_the_mount_root`, `a_rooted_backend_round_trips_files_and_directories`, and the rest of `host::tests`.
- Verify: COMPONENT scope for `promptforge-vfs`: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-vfs --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-vfs --all-features`, then `cargo test --locked --doc -p promptforge-vfs --all-features`.
- Commit: one commit with the `host.rs` changes and tests.
- Done when: the new tests fail before the change and pass after it, and apart from the follow resolver no path-level operation in `host.rs` canonicalizes a final-component link.

</step-1>

<step-2>

### Step 2: Let cancellation unwind through every Lua pcall

- Component: Lua cancellation
- Piece: the `cancel_requested` capture and its four re-raise sites, built jointly in one commit. The capture has no use without a site that reads it, and one test set covers all four sites.
- Todo: `lua-cancel`. The Lua crate's other todo, `lua-guard-lock`, was resolved by a separate change and is not built here.
- Depends on: nothing. Placed after Step 1 because `promptforge-lua` depends on `promptforge-vfs`.
- Read: Product Requirements, the first Problem bullet, the first Goal, and the first Success criterion; Functional Specification, the Author Lua under cancellation bullet, the Cancelled outcome bullet, the prelude capture bullet under States and validation, and the first Security bullet; Technical Design, the Lua architecture bullet and the `coro.rs` and `__impl_coro.lua` interface bullet; Testing Plan, the Lua cancellation unit bullet; Decision Record, the re-raise decision, the rejected `mlua::Error::external` alternative, and the hook-interval note; Project Survey.
- Build, in this order:
  - Tests first (see Tests). Before the fix the Lua-crate loops never end, so nextest terminates those tests after 180 seconds; that timeout is the expected failing-first result. Run the check once, filtered to the new tests. A case that already ends before the fix stays as a pin, and the return says so instead of contorting the test.
  - `crates/promptforge-internal/lua/src/coro.rs`: `install_shim_prelude` (line 156) takes the VM's `InstructionBudget`, passed from `SectionVm::install_coro_shims` (`vm.rs`, line 694) out of the `instruction_budget` field (`vm.rs`, line 115), and hands the shim a new last chunk argument, `cancel_requested`: a Lua function returning `InstructionBudget::is_cancelled()` (`hardening.rs`, line 123).
  - `crates/promptforge-internal/lua/src/__impl_coro.lua`: add `cancel_requested` to the chunk's argument list (lines 22 to 24) and to the header comment (lines 1 to 21), then:
    - `protected_call` and `pcall_outcome` (lines 69 to 76): on failure, when `cancel_requested()` is true, raise the raw failure with `error(failure, 0)` instead of returning `false` and the normalized error.
    - `protected_xcall` (lines 80 to 87), in both the function-handler and the non-function-handler branch: when the protected call fails and `cancel_requested()` is true, raise instead of returning.
    - `run_local_tool` (lines 127 to 137): when `raw_pcall(handler, args)` (line 129) fails and `cancel_requested()` is true, call `leave_local_handler()` and raise the failure at once, before the `local_tool_done` yield.
    - `compact` (lines 207 to 216): when `raw_pcall(compactor, reason)` (line 208) fails and `cancel_requested()` is true, raise the raw failure rather than the normalized one.
    - `guard` (lines 338 to 340) is unchanged. It stays the boundary that returns the failure to Rust, where `SectionVm::map_chunk_failure` (`vm.rs`, line 1278) maps any failure under the cancel flag to `Error::Interrupted`.
  - At `cd38ce8c` the Rust references sit at: `SectionVm::install_coro_shims`, `vm.rs` line 697; the `instruction_budget` field, `vm.rs` line 115; `SectionVm::map_chunk_failure`, `vm.rs` line 1282; `InstructionBudget::is_cancelled`, `hardening.rs` line 130.
  - The `_G` guard needs no change and gets no re-raise. `__impl_globals.lua` captures the base `pcall` at load and uses it only around the base `setmetatable` and `getmetatable`, C functions that run no Lua instructions, so the instruction hook cannot raise inside those protected calls. The shim's `pcall` and `xpcall` replacements reach `_G` through `raw_set` in `install_shim_prelude`, which the guard metatable does not intercept. The Lua test VM installs the guard exactly as a section VM does, so the new tests run with it live.
  - A `pcall` or `xpcall` that fails while the flag is clear behaves as today.
- Tests:
  - `crates/promptforge-internal/lua/src/tests.rs`, beside `a_pre_cancelled_run_aborts_a_tight_loop_promptly` (line 2210). Build a section VM with the coroutine shims installed the way `section_vm_with_var` in `prelude-tests.rs` does (host injection, `install_host_apis`, `install_scheduler_control_globals`, `install_coro_shims`), install an already-cancelled `CancelHandle` with `SectionVm::set_cancel`, and start each block with `SectionVm::start_block_coro`. `while true do pcall(function() while true do end end) end` fails with `Error::Interrupted`; the same loop through `xpcall` with a function message handler fails with `Error::Interrupted`; and on a VM with no cancel flag installed, `pcall(error, 'boom')` still returns `false` and the error.
  - `crates/promptforge-internal/engine/src/execute/tests/local_tools.rs`: a run whose local tool handler loops without yielding, cancelled mid-handler through `TokioDriver::cancel_handle` after a short delay in the manner of `cancel_during_in_flight_tool_call_returns_promptly` (`tool_loop.rs`, line 470), returns `Err(crate::Error::Interrupted)` within 5 seconds.
  - `crates/promptforge-internal/engine/src/execute/tests/models_loop_compactors.rs`: the same for a `models.loop` whose compactor loops without yielding on an overflow round.
  - Each new test's name contains `cancel`.
  - Focused commands: `cargo nextest run --locked -p promptforge-lua --all-features cancel` and `cargo nextest run --locked -p promptforge-engine --all-features local_tools models_loop_compactors`.
  - Existing pins that must pass unchanged: `long_running_lua_block_cancels_cooperatively`, `a_pre_cancelled_run_aborts_a_tight_loop_promptly`, and `cancel_during_in_flight_tool_call_returns_promptly`.
- Verify: COMPONENT scope for `promptforge-lua` and `promptforge-engine`: `cargo fmt --all --check`; `cargo clippy --locked -p <package> --all-targets --all-features -- -D warnings` for each; the component test pattern for each.
- Commit: one commit with the `coro.rs`, `vm.rs`, and `__impl_coro.lua` changes and the new Lua and engine tests.
- Done when: every new cancelled-run test ends with `Error::Interrupted`, the flag-clear `pcall` test passes, and the Lua and engine suites pass.

</step-2>

<step-3>

### Step 3: Keep chain reports off the walk position and drop the call stack

- Component: Engine scheduler
- Piece: the recorded section label and the scheduler cleanup, built jointly in one commit. Both edit `scheduler.rs`, `scheduler/chain.rs`, and `scheduler/tasks.rs`, and three engine tests form one set covering both: two notice tests pin the label, and the concurrent-call test pins the stack deletion. The `const` wrap in `config-limits.rs` rides along as the component's one remaining mechanical fix.
- Todo: `chain-section-label`, `scheduler-cleanup`
- Depends on: nothing. Placed after Step 2 so the engine suite runs over the final Lua crate.
- Read: Product Requirements, the second Problem bullet, the second Goal, and the second Success criterion; Functional Specification, the Scheduler reporting bullet, the second Inputs and outputs bullet, and the recorded section name bullet under States and validation; Technical Design, the Engine bullets, the Deleted bullet, and the saturating subtraction bullet; Testing Plan, the three Engine integration bullets; Decision Record, the record-on-chain and delete-the-stack decisions and the rejected placeholder name; Project Survey.
- Build, in this order:
  - Tests first (see Tests). Confirm at `8622b227` that the walk-end test panics with an index out of bounds in `Chain::section` (`scheduler.rs`, line 383), the between-sections test reports the wrong section name, and the concurrent-call test fails the `debug_assert_eq!` in `finish` (`scheduler/chain.rs`, lines 208 to 212).
  - `crates/promptforge-internal/engine/src/execute/scheduler.rs`: `Chain` gains an owned field holding the name of the section the chain most recently entered. `section_name()` (lines 395 to 405) returns the prompt title when `h1` is set and the recorded name otherwise, and its doc says so. `section()` and `blocks()` are unchanged and keep indexing the live section.
  - Initialize the field where a chain is built, by the Functional Specification's rule (the name of the slice's first section, or the prompt title when the slice is empty): the `Chain` literal in `start_chain` (`scheduler/chain.rs`, line 77) and the one in `start_live_h1` (`scheduler/h1.rs`, line 48), the only two `Chain` literals.
  - `scheduler/walk.rs` `enter_section` (line 131): after building the frame for `slice[index]` (lines 158 to 168), record that section's name. The H1 branch (lines 135 to 145) leaves the field alone, and `end_section`, `pop_position`, and `finish` never change it.
  - Delete the call stack: the `stack` field and its doc (`scheduler.rs`, lines 429 to 431) and its initializer (line 499); the push in `dispatch_call` (`scheduler/dispatch.rs`, line 314) and the doc's "pushes it on the chain stack" (lines 299 to 301); the `debug_assert_eq!` pop in `finish` (`scheduler/chain.rs`, lines 208 to 212), keeping the `answer_inline` call after it; the conditional pop and its comment (`scheduler/chain.rs`, lines 254 to 258); and the `clear` in `teardown` (`scheduler/drive.rs`, line 199). Reword the docs that describe the stack so none names it: the module doc's first line and its core-contents sentence (`scheduler.rs`, lines 1 and 32 to 36) and the `call_depth` field doc (lines 312 to 316).
  - `scheduler/tasks.rs` `release_slots`: `chain.slots_used -= 1` (line 591) becomes a saturating subtraction; the `debug_assert!` above it (lines 587 to 590) stays.
  - `crates/promptforge-internal/engine/src/execute/config-limits.rs`: wrap each `nz_*` call in `const { }`, in `RunLimits::new` (lines 70 to 74) and in the module's test (lines 186 and 187), so a zero literal fails to compile instead of reaching the macro's `unreachable!()` (line 15) at run time. The macro and its `const fn`s stay. No other file calls `nz_*`.
- Tests:
  - `crates/promptforge-internal/engine/src/execute/tests/model_task_notices.rs`: a prompt whose last section runs `tools.allow_tasks()`, lets the canned model start a task, and falls through while the task is live. The run finishes without a panic, reports a `TaskAbandoned` event, and reports the task notice under that last section's name.
  - Same file: a model task that ends while its owner sits between two sections reports its notice under the section just ended.
  - `crates/promptforge-internal/engine/src/execute/tests/scheduler/concurrency.rs`: two concurrently admitted tasks that each `call` a section issuing a chat, with the chats answered in reverse issue order, both complete. Nextest builds in debug, so the deleted `debug_assert_eq!` would fire here.
  - No test for the saturating release, since a debug build hits the kept `debug_assert!` first, and none for the `const` wrap, which is a compile-time property.
  - Focused command: `cargo nextest run --locked -p promptforge-engine --all-features model_task_notices scheduler::concurrency`.
  - Existing pins that must pass unchanged: `an_ending_owner_leaks_its_author_task_and_never_its_model_task` (`execute/tests/model_tasks.rs`, line 250), `cancelling_a_run_settles_every_live_task_with_one_terminal_before_the_run_ends` (`execute/tests/run_termination.rs`, line 78), and `run_limits_pins_all_six_defaults_and_the_untested_builders` in `config-limits.rs`.
- Verify: COMPONENT scope for `promptforge-engine`: `cargo fmt --all --check`; `cargo clippy --locked -p promptforge-engine --all-targets --all-features -- -D warnings`; `cargo nextest run --locked -p promptforge-engine --all-features`, then `cargo test --locked --doc -p promptforge-engine --all-features`.
- Commit: one commit with the scheduler, walk, chain, h1, dispatch, drive, tasks, and config-limits changes and the three tests.
- Done when: the three tests fail before the change and pass after it, no scheduler file names a call stack, and the engine suite passes.

</step-3>

<step-4>

### Step 4: Sweep dead items and stale text, then fold two directories

- Component: Internal crates sweep
- Piece: dead items, docs, comments, and the two directory folds, built jointly in one commit. None of them changes behavior, so no new test covers any one of them, and one pass of the full gates verifies all of them. Within the step, make the folds last, so every other edit, this step's and Steps 1 to 3's, already sits in the moved files.
- Todo: `dead-items`, `docs-sweep`, `comments-sweep`, `dir-folds`, `verify`
- Depends on: Steps 1 to 3. `vfs/README.md` describes Step 1's link semantics, the Lua docs follow Step 2, the comment edits in `scheduler/tasks.rs` and `scheduler/tool_call.rs` sit in the scheduler Step 3 edits, and the folds follow every edit to the moved files.
- Read: Product Requirements, the fourth Goal and the fourth Success criterion; Functional Specification, the opening paragraph; Technical Design, the File and public API changes bullets (the facade listing, Moved, and New); Testing Plan, the Regression bullets and Exit criteria; Decision Record, the file-move note under Assumptions; Project Survey.
- Build, dead items:
  - `crates/promptforge-internal/model-client/src/normalize.rs` (line 39; the plan's earlier text placed it under `client/`): remove `#[non_exhaustive]` from the crate-private `NormalizedTurn`.
  - `model-client/src/client.rs` (lines 29 to 31): remove the `StreamDelta` re-export and its comment. The re-export is not unused inside the crate: `client/read.rs` (line 18) and `client/stream.rs` (line 29) import `StreamDelta` through it with `use super::{...}`, and `client/stream-tests.rs` names it. Import it from `promptforge_types::wire::StreamDelta` in those files instead.
  - `model-client/src/lib.rs` (line 41): remove the metrics re-export (`CallMetrics`, `ClientTiming`, `LlamaTimings`, `Usage`, `VllmMetrics`).
  - Both removals break intra-doc links in `model-client/src/lib.rs`'s crate doc: `client::StreamDelta` (lines 7 and 23) and the five metrics names (lines 17 and 18). The denied `broken_intra_doc_links` lint turns those into a docs-build failure, so reword that doc: the metrics vocabulary and `StreamDelta` are canonical in `promptforge-types`, linked through `promptforge_types` paths or named as plain code, and the "re-exported through their historical paths" claim stays only for the `model` items it still covers. Update the `promptforge-types` dependency comment in `model-client/Cargo.toml`, which says the crate re-exports the metrics vocabulary.
  - The facade re-exports `StreamDelta` and the metrics types from `promptforge_types` directly (`crates/promptforge/src/lib.rs`, lines 77 and 190 to 195), and `harness-models` names `StreamDelta` only through the facade, so no other crate changes and `public-api.txt` stays identical. The compiler and `xtask api --check` confirm it.
  - Rename the temp-directory prefixes left from the facade's old name: `promptforge-api-prepare-` to `promptforge-prepare-` in `crates/promptforge/tests/suite/prepare.rs` (line 54), and `promptforge-api-vfs-` to `promptforge-engine-vfs-` in `crates/promptforge-internal/engine/src/execute/tests/suite/vfs.rs` (line 228).
- Build, docs:
  - Engine: correct `crates/promptforge-internal/engine/AGENTS.md` (lines 3 and 5) to describe the current public modules, read from `engine/src/lib.rs`, and the facade's actual re-exports. In `engine/README.md`, list the current effects (Chat, ToolCall, Store, Timer, TaskEvents), and drop the missing `LICENSE` link (line 13) and the unsupported "Rust 1.89 or later" claim (line 9). In `engine/src/lib.md` (line 3), stop saying the engine holds the parser and add TaskEvents to the effect list. Reword the `description` in `engine/Cargo.toml` (line 12), which still opens "prompt parser and".
  - Types: widen `types/README.md`, `types/AGENTS.md`, and the `types/Cargo.toml` description to the crate's full contents (among them `wire`, `models`, `tools`, `metrics`, `names`, `ids`, `replay`, `timestamp`, and `capabilities`, beside the untrusted guards, the cancellation tree, and the event vocabulary).
  - Lua: add the capability preludes, `input`, `tasks`, `messages`, and the VFS-backed store to `lua/README.md` and `lua/AGENTS.md`.
  - Model catalog: say in `crates/promptforge-internal/README.md` (the model-client paragraph, line 27) and in `model-client/README.md` that the model catalog types are defined in `promptforge-types` and re-exported by model-client.
  - VFS: write `crates/promptforge-internal/vfs/README.md` in the shape of its siblings' READMEs: what the crate holds (canonical paths, claims, routing, the memory and host backends, the mode policy, the declared store root and store view), that it is std only with the manifest test enforcing it, and the host backend's two resolutions as Step 1 left them.
  - Root: correct the root `AGENTS.md` statement that the types crate "contains the wire vocabulary only, never code" (line 54, the last clause of the Shared crates bullet; the plan's earlier text said line 55).
- Build, stale comments: rewrite each to match the code, or delete it when the code needs no comment.
  - Engine: `execute/context.rs` (lines 66 to 67, 80 to 81, and 84 to 85); `execute/section_context-construct.rs` (lines 6 to 8 and 26 to 32, adding the preludes step); `execute/section_context.rs` (lines 70 to 71); `error.rs` (lines 367 to 370); `execute.rs` (lines 129 to 130); `test_support/tokio_driver.rs` (lines 168 and 521); and `engine/Cargo.toml` (line 41).
  - VFS: `vfs/src/error.rs` (lines 9 to 14).
  - Model-client: `client/wire.rs` (lines 147 to 150), `normalize.rs` (lines 34 to 37, at `model-client/src/normalize.rs`), `model/error.rs` (line 45), and the duplicate `No Eq` comment in `model/options.rs` (lines 87 and 90; the plan's earlier text said `options.rs`).
  - Types: `models.rs` (line 242), and `event.rs` (lines 415 to 424), which must say `TaskNote` is reserved and not yet produced, as the facade's `event.md` says.
- Build, history comments: remove, or restate as a present-tense constraint, each comment that narrates history or cites an audit tag: `engine/src/test_support.rs` (lines 17 to 20 and 168; the plan's earlier text said 167); `engine/src/execute/config.rs` (lines 123 to 125); `engine/src/execute/requirements.rs` (lines 112 to 114); `engine/src/error.rs` (lines 43 and 522, the `F3` and `F4` tags); `engine/src/execute/scope.rs` (line 75, the `F7` tag); `engine/src/execute/section_context-construct.rs` (line 66); `engine/src/execute/run.rs` (lines 226 to 227); `scheduler/tasks.rs` (lines 173 to 174, 473 to 481, and 500 to 501 at `8622b227`, moved by Step 3); `scheduler/tool_call.rs` (lines 191 to 193); `vfs/src/handle.rs` (lines 1238 to 1241); and `parser/src/build.rs` (lines 1 to 6, the `PF-PARSER-012` tag).
- Build, directory folds, last:
  - Move `engine/src/execute/run-effect.rs` to `execute/run/effect.rs`, `run-effect-tests.rs` to `execute/run/effect-tests.rs`, and `run-tests.rs` to `execute/run/tests.rs`. In `execute/run.rs`, drop the `#[path]` attributes on `mod effect` (line 14) and `mod tests` (line 266). In the moved `run/effect.rs`, its tests attribute (line 393) becomes `#[path = "effect-tests.rs"]`, the `<module>-tests.rs` sibling convention.
  - Move `engine/src/test_support/recording-forward.rs` to `test_support/recording/forward.rs`, `recording-forward-tests.rs` to `test_support/recording/forward-tests.rs`, and `recording-observation.rs` to `test_support/recording/observation.rs`. In `test_support/recording.rs`, drop the `#[path]` attributes on `mod forward` (line 26) and `mod observation` (line 28). In the moved `recording/forward.rs`, its tests attribute (line 378) becomes `#[path = "forward-tests.rs"]`.
  - Move the files with a plain filesystem move; the session that commits stages the renames. No doc link or comment outside these `#[path]` attributes names the old paths.
- Tests: none new; the step changes no behavior. The existing suites of every touched package are the check, with the facade listing and the docs builds as the regression net.
  - Focused command: `cargo nextest run --locked -p promptforge-engine -p promptforge-model-client -p promptforge-types -p promptforge-parser -p promptforge-vfs -p promptforge-lua -p promptforge --all-features`, then `cargo +nightly-2026-09-05 xtask api --check` with no difference from `crates/promptforge/public-api.txt`.
- Verify: FULL scope, as the final step: the Project Survey's build, formatter, linter (including `cargo check -p gateway --no-default-features` and `cargo deny check`), docs (including the facade docs build, the engine's private-items build, and `cargo +nightly-2026-09-05 xtask api --check` with `crates/promptforge/public-api.txt` unchanged), and full-suite commands (the workspace run, its doctests, and the workshop family run), plus `cargo test -p build-xtask`, which the `AGENTS.md` Verification list names. When the step changes a guide source under `guide/src/`, also regenerate the combined guide with `cargo run --locked -q -p build-user-guide` and run `cargo xtask site --books-only`.
- Commit: one commit with the dead-item removals and their import and doc repairs, the docs and comment edits, the new `vfs/README.md`, the root `AGENTS.md` correction, and the six moved files.
- Done when: no listed doc or comment still says something the code contradicts, the six files sit in `execute/run/` and `test_support/recording/` and no `#[path]` attribute names a `run-*` or `recording-*` file, `public-api.txt` is unchanged, and every FULL gate passes.

</step-4>

- Exit: after Step 4, every Success criterion and Testing Plan check holds; the `_G` lock's criterion and test are covered by the separate change noted in the Decision Record.

</execution-plan>
