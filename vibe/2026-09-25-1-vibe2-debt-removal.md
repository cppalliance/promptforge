---
name: Debt collector vibe2
overview: "Remove the four debts that the 39 vibe2 commits since upstream/master (1fd82c62..a6b9a747) added and that remain at HEAD: a heartbeat regression test that can no longer fail, an editor Overwrite path that skips the timed-out-save state machine, stale module names in the workshop-server crate docs with no mechanical check, and a Linux-dead import that fails the Linux clippy CI job."
todos:
  - id: debt-04-linux-clippy
    content: "DEBT-04: gate use super::*; in workspace/tests/jail.rs to Windows; verify workshop-workspace clippy and jail tests"
    status: pending
  - id: debt-01-heartbeat
    content: "DEBT-01: restore real-time quiet window in startup_convergence.rs; prove with 5 runs and heartbeat mutation"
    status: pending
  - id: debt-02-overwrite
    content: "DEBT-02: add writeCurrent helper in editor-panel.ts for save() and overwrite(); add cases (a) and (b) to editor-save-timeout.mjs; mutation proof"
    status: pending
  - id: debt-03-docs
    content: "DEBT-03: intra-doc-link module inventories in workshop-server agents.rs and lib.rs; add private-items rustdoc step to check-workshop in ci.yml; link mutation proof"
    status: pending
  - id: exit-checks
    content: Run workshop clippy, nextest, UI npm test, and the new docs step
    status: pending
isProject: false
---

# Debt Removal Plan - promptforge vibe2 since upstream/master

<product-contract>

## Product Requirements

**Scope and target work**
- Repository: `C:\Users\Vinnie\cursor\promptforge2`, branch `vibe2`.
- Baseline: `1fd82c62`. This is `upstream/master`, and it is also the merge base with `HEAD`. The local ref was not fetched.
- Endpoint: `a6b9a747` (`HEAD`).
- Target: 39 commits under two plans:
  - `vibe/2026-09-24-2-workshop-crates-cleanup.md`: `b483266c` through `ce10a8eb`.
  - `vibe/2026-09-24-2-issues-69-59-70.md`: `a7458096` through `a6b9a747`.
- Disposition: the worktree. It has no tracked edits; the only untracked files are stale build artifacts under `crates/workshop/shell/`.

**Cleanup goals**
- DEBT-01: the heartbeat "refresh stops after restore" test can fail again.
- DEBT-02: no editor write path sends a token the editor knows is stale after a timed-out write.
- DEBT-03: the workshop-server module inventories name modules that exist, and a compiler check keeps them honest.
- DEBT-04: the Linux `clippy` CI job no longer fails on the Windows-only glob import in `jail.rs`.

**Non-goals**
- The pre-existing abandoned-write race (see Debt Inventory).
- Prose drift in READMEs, AGENTS.md and the guide, which rustdoc cannot check.
- The residual candidates.
- Edits to `vibe/archdoc.md` or the plan files.

**Success criteria**
- Each debt's check in the Testing Plan passes.
- The mutation proofs fail as described and then pass once reverted.
- The workshop CI partition passes, including the new docs step.
- The Linux `clippy` CI job passes the next time this code runs in CI.

## Functional Specification

### Debt Inventory

**DEBT-01 - Heartbeat quiet-window assertion cannot detect a regression (introduced, `6b23dd75`)**
- Evidence: [crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs](crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs), lines 147-157. The pre-target `tokio::time::sleep(TEST_INTERVAL * 4)` became `pause(); advance(TEST_INTERVAL * 4).await; resume();` followed by a synchronous `assert_eq!` on `state.requests`.
- Why the check is blind:
  - In tokio 1.53.1, `advance` yields exactly once.
  - A regressed retry needs two real loopback HTTP round trips (`/health`, then `/v1/models`) on the same current-thread runtime before the counter moves.
  - So the assertion passes whether or not retries continue.
- Impact:
  - This is the only guard for heartbeat retry termination. `crates/workshop/gateway/src/heartbeat-tests.rs` defers to it.
  - Plan step 23 later refactored `heartbeat.rs` (228 lines) and was certified against this blind check.
  - It contradicts the cleanup plan's Step 4 contract that the tests "assert what they asserted before".
- Reversal cost: low, test-only.
- Target state: the assertion observes real time again.

**DEBT-02 - `overwrite()` bypasses the unknown-token state (introduced, `6d44740d`)**
- Evidence: [crates/workshop/ui/src/parts/editor/editor-panel.ts](crates/workshop/ui/src/parts/editor/editor-panel.ts).
  - `save()` (lines 182-230) sets `lastSentText` and moves to `tokenUnknown = true` on a 408.
  - `overwrite()` (lines 437-461) does neither. `6d44740d` edited its success branch only.
- Reachable path:
  1. A 409 opens the conflict dialog while the editor still holds the stale token T0.
  2. The user clicks Overwrite. It reads T1, writes, and gets a 408. The editor still holds T0 and `tokenUnknown` stays false.
  3. The next `save()` sends T0.
- A second variant: a stale `lastSentText` skews the next reconcile.
- This contradicts the plan's acceptance criterion ("sends no stale token") and the commit body ("no save path forwards a token the editor does not currently know").
- Recurrence: `f5cf157c` (2026-08-27, a different plan) already repaired `overwrite()` for lagging behind `save()`'s bookkeeping.
- Severity: low, because the server's token check fails closed and answers a 409 with a misleading dialog.
- Reversal cost: low, one private UI file.
- Target state: `save()` and `overwrite()` share one write path.

**DEBT-03 - Stale module names in workshop-server crate docs (worsened, `4c0f465b`, `5a67ec1b`, `2c0cb1eb`)**
- Evidence:
  - [crates/workshop/server/src/agents.rs](crates/workshop/server/src/agents.rs) lines 1-2 say "the `/ws` workshop socket (`session`)". No `session` module exists; `/ws` now lives in `crate::workshop_socket`.
  - [crates/workshop/server/src/lib.rs](crates/workshop/server/src/lib.rs) lines 13-14 still place `/ws` inside `agents`.
  - `lib.rs` line 18 says "assembled in `app.rs`", but the helpers now live in `app/compose.rs`.
- How it survived: the move, a second edit to the same line, and a dedicated doc sweep ("Correct the workshop's code-level docs") all missed it.
- Same cause in earlier plans: `2cee387a` (api-firewall), `f61d2570` (api-runtime-debt) and `b9ed2843` (workspace-debt-removal) each needed a manual prose sweep after a restructure.
- Enabling condition: the inventories use plain backticks, and CI never builds rustdoc for the workshop crates. The `docs` job in [.github/workflows/ci.yml](.github/workflows/ci.yml) line 141 excludes them, and `check-workshop` has no docs step.
- Reversal cost: low.
- Target state: the inventories use intra-doc links, and a rustdoc build with private items and denied warnings runs in CI.

**DEBT-04 - Linux-dead glob import fails the clippy job (introduced, `8a17ee0b`)**
- Evidence: CI run [36110606135](https://github.com/cppalliance/promptforge/actions/runs/36110606135) at `ce10a8eb` failed only in `clippy`, which denies warnings. The error is `unused import: super::*` at [crates/workshop/workspace/src/workspace/tests/jail.rs](crates/workshop/workspace/src/workspace/tests/jail.rs) line 11. `ci-green` failed only because it aggregates the other jobs. The analysis pass missed this; CI found it.
- Cause: every item that uses the glob (`Path`, `PathBuf`, `fs`, `Workspace`, `WorkspaceError`, `granted_dir`, `simplified`) is `#[cfg(windows)]`. The ungated `symlink_unavailable` and its two tests use nothing from the parent. The local gates run only on Windows, so they could not see it.
- Scope: this is the only Linux-only error.
  - The Linux `test` job compiled every crate in clippy's scope with all features and test targets, and emitted exactly this one rustc warning.
  - Clippy stopped before 9 crates. In those crates the PR touched platform-gated files only through string and constant edits.
- Present at disposition: `jail.rs` has no post-target diff.
- Reversal cost: trivial. Target state: the import compiles only on Windows.

**Exposed pre-existing debt (reported only, not debt added)**
- DEBT-X1: an abandoned write can land after a later successful write and silently revert a file the UI shows as saved.
- Mechanism:
  - `with_deadline` in `crates/workshop/support/src/deadline.rs` abandons the request rather than cancelling it.
  - `Workspace::write_file` checks the token before an unconditional rename, with no per-path lock.
- Origin: `f508f0f8` (2026-08-27). The target narrowed the path, so it now requires the user to click Overwrite.
- What the target added is statements only:
  - The plan's claim that "the worst case is the conflict dialog" (plan lines 164, 488, 504).
  - A mislabeled fourth case in `crates/workshop/ui/test/editor-save-timeout.mjs`.
- A real fix changes the workspace crate's write semantics. That is a separate data-integrity item.

**Rejected candidates: 67 across three partitions, each challenged by a fresh reviewer**
- 22 residual-but-acceptable: real, but with no reachable consequence or already protected. Examples:
  - Service tokens split from their default registration: production's single `main.ts` entry registers both.
  - The engine's mock transport still applies a whole-request timeout: it is test-only, no present run reaches it, and harness behavior tests protect the contract.
  - The chat-gate zero-deadline quiet check: later assertions in the same test still catch the regression.
  - The relay's copy of the error renderer.
  - The gateway icon copies, which sit under an existing sync rule.
- 19 weak/speculative: structural leads with no demonstrated consequence, such as parameter clusters, visibility widening and module size.
- 19 false: the refactors (compose split, supervisor split, socket framing split, run-loop phases, `/prompts/contract` move, renames) change no behavior, wire string or persisted key. The `saveAs()` half of DEBT-02 is also false: a 408 there leaves `this.path`'s token valid.
- 7 unrelated pre-existing, for example the harness writing a literal `flags: 0`.

</product-contract>
<implementation-contract>

## Technical Design

**DEBT-01 (test only)**
- In `startup_convergence.rs`, replace the three lines `pause`/`advance`/`resume` with `tokio::time::sleep(TEST_INTERVAL * 4).await;`. That is 100 ms of real time; `TEST_INTERVAL` is 25 ms in `heartbeat_loop.rs`.
- Replace the comment above it. The new comment should say why the window must be real time: the probes are real loopback HTTP on the test runtime, and a paused-clock advance cannot drive them.

**DEBT-02 (private UI change in `editor-panel.ts`)**
- Add one private method that performs the PUT for this panel's own file and owns all write-outcome bookkeeping:

```ts
private async writeCurrent(path: string, text: string, expectedToken: string | null): Promise<void> {
  this.lastSentText = text;
  try {
    const written = await this.writer()(path, text, expectedToken);
    this.token = written.token;
    this.tokenUnknown = false;
    this.surface.markSaved(text);
  } catch (error: unknown) {
    if (isDeadlineElapsed(error)) {
      this.tokenUnknown = true;
      this.showError("The save timed out; the file may or may not have been written.");
    } else if (isModifiedConflict(error)) {
      this.showConflictDialog();
    } else {
      throw error;
    }
  }
}
```

- `save()` keeps its guard, text capture and reconcile read, and replaces lines 214-217 with a call to `writeCurrent`.
- `overwrite()` keeps its guard and fresh read, then calls `writeCurrent(this.path, text, fresh.token)`.
- Both keep their existing outer `catch` that calls `showError` for read failures.
- `saveAs()` stays outside the helper. It writes a different path, so a 408 there must not mark `this.path`'s token unknown.
- Update the doc comments on `save()` and `overwrite()` to say that both route through `writeCurrent`.
- No wire, persisted or public change is involved.

**DEBT-03 (docs and CI)**
- `agents.rs`:
  - Drop `/ws` from the inventory.
  - Link the children as intra-doc links ([`socket`], [`relay`], [`state`], [`bindings`]).
  - Note that `/ws` is [`crate::workshop_socket`].
- `lib.rs`:
  - Lines 12-20: say that the sessions subsystem in [`agents`] serves `/agents/ws` and `/v1/models`, and that [`workshop_socket`] serves `/ws`.
  - Change "assembled in `app.rs`" to [`app`] with the helpers in [`app::compose`].
- `ci.yml`: add a step to the `check-workshop` job after "Doctests (workshop)", at line 200:

```yaml
      - name: Docs (workshop-server, private items)
        env:
          RUSTDOCFLAGS: -D warnings
        run: cargo doc --locked --no-deps -p workshop-server --document-private-items
```

- Run this command locally first. Fix every warning it surfaces in workshop-server; all of them are doc-text fixes of the same cause.
- If a fix needs anything other than doc text, or the warnings exceed roughly 30 sites, stop and report before continuing.

**DEBT-04 (test module only)**
- Gate the import at `jail.rs` line 11, matching the precedent in `crates/gateway/local/src/server-tests.rs:10-11`:

```rust
#[cfg(windows)]
use super::*;
```

</implementation-contract>
<verification-contract>

## Testing Plan

**DEBT-01**
- Focused: `cargo nextest run --locked -p workshop-server heartbeat_loop` passes 5 runs in a row.
- Mutation proof:
  1. Temporarily make the heartbeat keep calling `refresh_sources` after the selection is restored, for example by forcing the "source incomplete" condition true in `crates/workshop/gateway/src/heartbeat.rs`.
  2. Confirm "selection restoration ends refresh retries" fails.
  3. Revert.

**DEBT-02**
- Add two cases to [crates/workshop/ui/test/editor-save-timeout.mjs](crates/workshop/ui/test/editor-save-timeout.mjs), following its existing injected `readFile`/`writeFile` sections:
  - Case (a): save answers 409 on T0, then Overwrite (reads T1) answers 408, then save. Assert that no write carries T0, that the save performs a reconcile read, and that the 408 message appears.
  - Case (b): an unknown-token mismatch opens the dialog, then Overwrite answers 408, then the disk holds the Overwrite text, then save. Assert that the save adopts the disk token and writes without reopening the dialog.
- Mutation proof: temporarily restore the direct `this.writer()` call in `overwrite()`. Case (a) fails. Revert.
- Regression: the existing four timeout cases, plus `editor-panel.mjs` and `editor-save-race.mjs` (the Overwrite success and in-flight typing paths), still pass.

**DEBT-03**
- `rg "\(\`session\`\)" crates/workshop/server/src` returns nothing.
- The new `cargo doc` command passes locally.
- Mutation proof: temporarily rename `workshop_socket` in the `agents.rs` link to `session`. The docs step fails with `broken_intra_doc_links`. Revert.

**DEBT-04**
- Windows: `cargo clippy --locked -p workshop-workspace --all-targets --all-features -- -D warnings` and `cargo nextest run --locked -p workshop-workspace jail` pass. The gated tests still compile and use the glob.
- Linux: the `clippy` CI job passes on the next CI run of this code. This is checked in CI only, because WSL has no Rust toolchain, and a cross-target check from Windows fails on C build dependencies (`aws-lc-sys`, `mlua-sys`, `simsimd`).

**Exit checks**
- `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
- `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
- `npm test --prefix crates/workshop/ui`
- The new docs step.

</verification-contract>
<decision-record>

## Decision Record

**Reversible decisions**
- DEBT-01:
  - Chosen: restore the 100 ms real-time window, costing 100 ms of test time.
  - Rejected: a tick counter under `test-fixtures` in workshop-gateway, which is new test surface for one assertion. Also rejected: paused-clock auto-advance, which interacts badly with real TCP.
- DEBT-02:
  - Chosen: one shared write helper.
  - Rejected: copying the 408 branch into `overwrite()`. That would be the third hand-copy of the same bookkeeping, after `f5cf157c`.
  - `saveAs()` is excluded from the helper on purpose (see Technical Design).
  - `lastSentText` needs no reset. It is read only while `tokenUnknown` is true, and only the helper sets that flag, immediately after assigning `lastSentText`.
- DEBT-03:
  - Chosen: intra-doc links plus a rustdoc build that denies warnings. This is a compiler check, not a structural ratchet.
  - Rejected: a tidy script that parses `//!` inventories (a bespoke source parser), and a manual "grep the old name" plan rule (the discipline that already failed).
  - The step covers workshop-server only, where the instance lives (see Deferred and Out of Scope).
- DEBT-04:
  - Chosen: a `#[cfg(windows)]` gate on the import.
  - Rejected: `#[allow(unused_imports)]`, which hides the signal, and a nested `#[cfg(windows)] mod`, which is churn for one line.

**User-resolved architecture choices**
- None required. No retained remedy touches a public interface, a persisted or wire format, component ownership, dependency direction, or a trust boundary.

**Assumptions and risks**
- The DEBT-01 and DEBT-02 consequences were inferred from endpoint code and tokio 1.53.1 source, not observed by running. The mutation proofs settle them.
- `upstream/master` was not fetched, so newer upstream commits are not considered.
- The DEBT-03 docs step may surface existing rustdoc warnings. The stop threshold above bounds that.

### Deferred and Out of Scope

- DEBT-X1 and its statements: the server write race, the cleanup plan's lines 164, 488 and 504, and the mislabeled fourth case in `editor-save-timeout.mjs`. The conflict-dialog wording ("modified outside the editor") also belongs with X1. Revisit as its own data-integrity item; a real fix changes the workspace crate's write semantics.
- Extending the private-items docs step to `workshop` and `workshop-server-api`. Revisit once their rustdoc warning volume is known.
- The chat-gate quiet-check comment in `crates/workshop/server/tests/it/chat_gate.rs`, and all other residual candidates. Revisit a residual when its consequence becomes reachable, for example a second production UI entry bundle (the service-token split) or an engine test that streams past its `request_timeout` (the mock transport).
- The untracked `crates/workshop/shell/` artifacts. They are clone-local; delete them by hand if wanted.
- Any edit to `vibe/archdoc.md` or the `vibe/` plan files.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <crate>`. Plain `cargo build` builds only the default member `crates/gateway/app` (binary `promptforge-gateway`). Desktop app: `cargo build --locked -p workshop`; desktop release orchestration: `cargo workshop` (alias for `run -p build-workshop --`). The UI bundles are built by crate build scripts through `build-ui`, which needs `npm ci --prefix crates/workshop/ui` and `npm ci --prefix crates/gateway/config-ui/ui` first. Windows links with `rust-lld` and the static CRT per `.cargo/config.toml`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-filter>`; for `workshop`, `workshop-server`, and `workshop-server-api` drop `--all-features`. Integration tests in one binary: add `--test it` (or `--test suite` for `promptforge`). UI tests: `node --test <file>.mjs` from the UI directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, then `cargo test -p <crate> --all-features --doc` (workshop crates without `--all-features`; `workshop-server` also has `cargo nextest run --locked -p workshop-server --features headless`). UI components: `npm test` in `crates/workshop/ui` or `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. The workspace run includes `build-xtask`, the structural harness (`cargo test -p build-xtask`). UI suites: `npm test` in both UI directories.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`. Never run a standalone `cargo check --workspace`. UI type checks: `npm run typecheck` in both UI directories. Supply chain: `cargo deny check` (pre-push runs it when installed).
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps` (default features), both with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`); user guide: `mdbook build guide`. Facade surface: `cargo +<pinned nightly> xtask api --check`, nightly named in `crates/build-xtask/src/api/toolchain.rs`, checked against `crates/promptforge/public-api.txt`.
- Test placement and naming conventions:
  - Unit tests either inline as `#[cfg(test)] mod tests { ... }` or in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `tidy.rs` with `tidy-tests.rs`); a `tests/` subdirectory under `src/` appears once a module's tests reach three files.
  - Integration tests are one binary per crate at `tests/it/main.rs` (`tests/suite/main.rs` in `promptforge`) that declares topic modules plus a `support.rs` helper module; prompt fixtures sit under `tests/prompts/`. Benches exist only in `promptforge-engine` and `promptforge-lua` (criterion).
  - Test function names are behavior sentences in snake_case (`a_pending_timer_is_torn_down_by_a_cancel`, `workshop_tier_dependencies_flow_one_way`). Async tests use `#[tokio::test]`. Test roots open with `#![expect(clippy::expect_used, clippy::unwrap_used, reason = "...")]`; `clippy.toml` allows unwrap and expect in tests.
  - UI tests are Node `node:test` `.mjs` files: `crates/workshop/ui/test/*.mjs` (with `test/helpers/`), and beside source as `src/**/*.test.mjs` in `crates/gateway/config-ui/ui`; `tools/*.test.mjs` sit beside their scripts.
  - Behavior changes ship with tests in the same change; nextest caps the whisper-backed STT packages in a `heavy` test group.
- Directory map:
  - `crates/` - the public and shared layer: root crates `promptforge` (facade), `gateway-api-types`, `gateway-api-discovery`, `harness-api`, `shared-error-source`, `shared-loopback`, `workspace-hack`, and `build-*` tooling (`build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`), plus `shared-ui` (TypeScript and CSS package, not a Rust crate).
  - `crates/promptforge-internal/` - private engine family: `engine`, `types`, `vfs`, `lua`, `parser`, `store`, `model-client`.
  - `crates/gateway/` - private gateway family: `app`, `cloud-providers`, `config`, `config-ui` (with its `ui/` SPA), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `crates/harness/` - private harness family: `runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, `web-search`.
  - `crates/workshop/` - private Workshop family: `desktop` (Tauri app, package `workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`, and `ui/` (the SPA).
  - `guide/` - mdBook user guide and contributor docs. `prompts/` - sample Markdown prompts. `tools/` - Node release scripts with tests. `vibe/` - plans, archdoc, and design notes. `images/` - banners. `.github/` - CI workflows and fixtures. `.config/` - nextest and hakari. `.githooks/` - pre-commit and pre-push.
  - Root config: `Cargo.toml` (explicit member list, lints, workspace deps), `rust-toolchain.toml` (stable), `rustfmt.toml`, `clippy.toml`, `deny.toml`, `dist-workspace.toml` (cargo-dist), `AGENTS.md` (repository rules).
- Component boundaries:
  - `promptforge` is the only crate outside its family that may reach `promptforge-internal/*`; promptforge crates never depend on gateway, workshop, or harness crates. The engine is a sans-I/O state machine exchanging effects and events.
  - Gateway exposes only `gateway-api-types` and `gateway-api-discovery`; nothing outside may depend into `crates/gateway/`, and gateway crates never depend on promptforge, workshop, or harness. `gateway-stt` is the only family-visible STT crate. It runs as a separate process reached over HTTP and WebSocket plus the discovery file.
  - Harness exposes only `harness-api`; harness crates may depend on `promptforge`, the gateway public pair, and `shared-*`, never on workshop or private gateway crates. It is the engine's only production host.
  - Workshop crates may depend on `harness-api`, `promptforge`, the gateway public pair, and `shared-*` only. The desktop app depends on `workshop-server-api`, never `workshop-server`. Inside the family, tiers flow one way: server, then features, then services, then vocabulary.
  - `shared-*` depend on no product crates. `build-*` crates are meta tooling exempt from container privacy; only `build-ui` is depended on, as a build dependency. Every member depends on `workspace-hack`.
  - Family container crates may depend only on `crates/` root crates and their own siblings. Rules bind normal, dev, build, and target-specific dependencies, and `cargo test -p build-xtask` enforces them. The archdoc names a CLI component, but no dedicated CLI crate exists in the tree.
- Conventions summary:
  - Rust 2024 edition on stable; workspace lints forbid `unsafe_code` (explicit boundaries only, each unsafe block preceded by its safety invariants), warn on `missing_docs` and `unreachable_pub`, and deny clippy `all`, `pedantic`, `unwrap_used`, and `expect_used`.
  - Every `workshop-*` and `harness-*` lib.rs opens with a `//!` doc holding a `## Invariants` marker; files in marker crates stay at or under 500 lines.
  - Source directories are flat: one or two child modules live as `<parent>-<label>.rs` siblings wired with `#[path]`; three or more become a subdirectory.
  - Reuse an existing facility before building new machinery; no new structural checks without explicit user approval; Cargo features gate real constraints only.
  - Error and status messages are concise and self-contained for model consumption. Comments explain only non-obvious constraints and cite upstream issue URLs for workarounds.
  - Run-log JSON round-trips exactly (sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`). Third-party errors are wrapped through `shared-error-source`.
  - SPA rules: CSS beside its TypeScript, `--ws-*` tokens instead of raw values, no `localStorage` (state goes to `ui-state.json` or the `.pfwork` workspace file through the server).

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Gate the jail tests' glob import to Windows [completed]

- Component: workshop-workspace
- Component placement: first of three. `workshop-server` depends on this services-tier crate, and this is the only item that fixes a CI job already failing (Linux `clippy`, run 36110606135), so landing it first gives every later push a meaningful Linux clippy result.
- Piece: jail test module imports. It is the component's only piece, so it is built alone.
- Covers: DEBT-04.
- Artifacts:
  - `crates/workshop/workspace/src/workspace/tests/jail.rs` line 11, `use super::*;`.
  - Precedent: `crates/gateway/local/src/server-tests.rs` lines 10-11.
- Changes:
  - Add `#[cfg(windows)]` on the line directly above `use super::*;`.
  - Leave `symlink_unavailable` and its two ungated tests as they are; they use nothing from the parent module.
- Verification (Windows):
  - `cargo clippy --locked -p workshop-workspace --all-targets --all-features -- -D warnings` passes.
  - `cargo nextest run --locked -p workshop-workspace jail` passes, and the Windows-gated tests still compile against the glob.
  - Linux proof: the `clippy` job passes on the next CI run of `vibe2`. Record it as pending, not as a blocker. Do not attempt a local Linux check: WSL has no Rust toolchain, and a cross-target check from Windows fails on `aws-lc-sys`, `mlua-sys`, and `simsimd`.
- Staged files: `crates/workshop/workspace/src/workspace/tests/jail.rs` only.

</step-1>

<step-2>

### Step 2: Route editor save and overwrite through one write path [completed]

- Component: workshop-ui
- Component placement: second of three. `crates/workshop/server/build.rs` bundles `crates/workshop/ui/src/main.ts` into the server's embedded assets, so the UI is an input to `workshop-server`. Landing it before the server steps means the closing exit checks, which rebuild that bundle, exercise the finished UI.
- Piece: editor write path. It is the component's only piece. The helper and its tests are built jointly in this one step because the new test cases exist to prove the helper.
- Covers: DEBT-02.
- Artifacts:
  - `crates/workshop/ui/src/parts/editor/editor-panel.ts`: a new private method `writeCurrent(path, text, expectedToken)`; `save()` (lines 182-230, direct write and its bookkeeping at lines 214-217); `overwrite()` (lines 437-461, direct write and its bookkeeping at lines 448-451); `saveAs()` (line 240), which stays unchanged.
  - `crates/workshop/ui/test/editor-save-timeout.mjs`: new cases (a) and (b), and the header comment's case list.
- Changes:
  - Add `writeCurrent` as sketched in the Technical Design. It alone sets `lastSentText`, updates `token`, clears or sets `tokenUnknown`, calls `surface.markSaved`, shows the 408 message, and opens the 409 conflict dialog; any other error is rethrown.
  - `save()`: keep its guard, text capture, and reconcile read; replace lines 214-217 with `await this.writeCurrent(this.path, text, expectedToken)`.
  - `overwrite()`: keep its guard and fresh read; replace lines 448-451 with `await this.writeCurrent(this.path, text, fresh.token)`.
  - Keep the existing outer `catch` in both methods, which calls `showError` for read failures.
  - Update the doc comments on `save()` and `overwrite()` to say both route through `writeCurrent`.
  - Leave `saveAs()` outside the helper: it writes a different path, so a 408 there must not mark `this.path`'s token unknown.
  - Add case (a) and case (b), using the file's existing scripted reader and writer:
    - Case (a): save answers 409 on T0, then Overwrite (reads T1) answers 408, then save. Assert that no write carries T0, that the save performs a reconcile read, and that the 408 message appears.
    - Case (b): an unknown-token mismatch opens the dialog (a timed-out save, then a save whose reconcile read does not match), then Overwrite answers 408, then the disk holds the Overwrite text, then save. Assert that the save adopts the disk token and writes without reopening the dialog. Add the two cases to the header comment's list; leave the existing fourth case's wording alone, because relabeling it belongs to deferred DEBT-X1.
- Verification, from `crates/workshop/ui`:
  - `node test/editor-save-timeout.mjs` passes all six cases.
  - `node test/editor-panel.mjs` and `node test/editor-save-race.mjs` pass (the Overwrite success and in-flight typing paths).
  - `npm run typecheck` passes.
  - Mutation proof: temporarily restore the direct `this.writer()` call in `overwrite()`, confirm case (a) fails, then revert and rerun green.
- Staged files: `crates/workshop/ui/src/parts/editor/editor-panel.ts` and `crates/workshop/ui/test/editor-save-timeout.mjs` only.

</step-2>

<step-3>

### Step 3: Restore the heartbeat test's real-time quiet window

- Component: workshop-server
- Component placement: third of three. It is the top tier: it depends on `workshop-workspace` and embeds the UI bundle. It goes last so the closing exit checks, which include the docs step this component adds, see every change.
- Piece: heartbeat regression test. Built sequentially before the docs piece in Step 4: the two share no files and neither needs the other, and the docs piece goes second because it runs the exit checks.
- Covers: DEBT-01.
- Artifacts:
  - `crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs` lines 147-157: the paused-clock comment (lines 148-149) and `tokio::time::pause`/`advance`/`resume` (lines 150-152).
  - `TEST_INTERVAL` (25 ms) in `crates/workshop/server/tests/it/heartbeat_loop.rs` line 67.
  - Mutation target only, never committed: the `if !refresh.profiles_ready || !refresh.catalog_ready` condition before `refresh_sources` in `crates/workshop/gateway/src/heartbeat.rs` (line 269).
- Changes:
  - Replace the three paused-clock lines with `tokio::time::sleep(TEST_INTERVAL * 4).await;`, a 100 ms real-time window.
  - Replace the comment above it with one saying the window must be real time: the probes are real loopback HTTP on the test runtime, and a paused-clock advance cannot drive them.
  - Keep `requests_after_restore` and the `assert_eq!` unchanged.
- Verification:
  - `cargo nextest run --locked -p workshop-server heartbeat_loop` passes 5 runs in a row.
  - Mutation proof: temporarily force the "source incomplete" condition true in `heartbeat.rs` so refresh keeps running after the selection is restored; confirm the assertion "selection restoration ends refresh retries" fails; revert and rerun green.
  - `cargo clippy -p workshop-server --all-targets -- -D warnings` passes.
- Staged files: `crates/workshop/server/tests/it/heartbeat_loop/startup_convergence.rs` only. `crates/workshop/gateway/src/heartbeat.rs` must be unmodified after the mutation proof.

</step-3>

<step-4>

### Step 4: Link the workshop-server module inventories and check them in CI

- Component: workshop-server
- Piece: crate docs and CI docs gate. Built sequentially after Step 3. It runs the plan's exit checks, because they include the docs step it adds.
- Covers: DEBT-03 and the Testing Plan's exit checks.
- Artifacts:
  - `crates/workshop/server/src/agents.rs` lines 1-5, the module inventory.
  - `crates/workshop/server/src/lib.rs` lines 12-20, the subsystem inventory and the "assembled in `app.rs`" sentence.
  - Link targets that exist today: `crate::workshop_socket` (`workshop_socket.rs`), `app` (`app.rs`), `app::compose` (`app/compose.rs`), and the `agents` children `socket`, `relay`, `state`, and `bindings`.
  - `.github/workflows/ci.yml`: the `check-workshop` job, directly after the "Doctests (workshop)" step (lines 199-200).
- Changes:
  - `agents.rs`: drop `/ws` and `session` from the inventory, write the children as intra-doc links ([`socket`], [`relay`], [`state`], [`bindings`]), and note that `/ws` is served by [`crate::workshop_socket`].
  - `lib.rs`: say the sessions subsystem in [`agents`] serves `/agents/ws` and `/v1/models` and that [`workshop_socket`] serves `/ws`; replace "assembled in `app.rs`" with [`app`], with the helpers in [`app::compose`].
  - `ci.yml`: insert the "Docs (workshop-server, private items)" step exactly as written in the Technical Design.
  - Run the docs command locally in PowerShell: `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps -p workshop-server --document-private-items`. The non-headless build bundles the UI, so run `npm ci --prefix crates/workshop/ui` first if its `node_modules` is missing.
  - Fix every warning it reports as doc text in `workshop-server`. Stop and report before continuing if any fix needs more than doc text, or if the warnings exceed roughly 30 sites.
- Verification:
  - ``rg '\(`session`\)' crates/workshop/server/src`` returns nothing.
  - The docs command passes.
  - Mutation proof: temporarily rename `workshop_socket` in the `agents.rs` link to `session`, confirm the docs command fails with `broken_intra_doc_links`, then revert and rerun green.
  - Exit checks, with this step's changes in the tree and before committing: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`; `npm test --prefix crates/workshop/ui`; and the docs command.
  - To cover the rest of the `check-workshop` partition named in the success criteria, also run `cargo nextest run --locked -p workshop-server --features headless` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
  - If an exit check fails because of an earlier step's item, stop and report it instead of fixing it in this commit.
- Staged files: `crates/workshop/server/src/agents.rs`, `crates/workshop/server/src/lib.rs`, `.github/workflows/ci.yml`, and any `workshop-server` doc-text fixes the docs build required.

</step-4>

</execution-plan>
