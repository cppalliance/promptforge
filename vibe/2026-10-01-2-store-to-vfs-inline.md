---
name: Store to Vfs inline
overview: "Rename the Engine's Store effect vocabulary to Vfs forms, then have the Harness answer Vfs effects inline and delete StorePerformer (change 1 of the host-boundary report). Two steps, two commits, no push. Self-contained: a fresh agent can execute it from this file alone."
todos:
  - id: step-1-rename-store-to-vfs
    content: "Step 1: rename the Store effect vocabulary to Vfs across the Engine, Lua types, facade, Harness, tests and docs; bless public-api.txt; commit 1 (pure rename)"
    status: pending
  - id: step-2-answer-vfs-inline
    content: "Step 2: answer Effect::Vfs inline in the Harness effect loop; delete StorePerformer, VfsStore and Performers.store; fix tests and docs, including the vibe/archdoc.md performer sentence; commit 2"
    status: pending
isProject: false
---

# Rename Store to Vfs and answer Vfs effects inline

<product-contract>

## Product Requirements

The Engine calls its file effect "Store" although every one of its operations is a VFS operation made through the run's store view, and the Harness performs that effect through a `StorePerformer` seam on tokio's blocking pool that only forwards to the VFS. This change renames the vocabulary on the effect path to Vfs forms, keeping `store` only where it is Lua's name for the declared store. It also makes the Harness answer the Vfs effect inline in its effect loop and deletes the seam. Prompt authors and models see no change. This plan restates everything it needs, so a reader needs no other document.

- Problem and users:
  - The Engine calls its file effect "Store" (`Effect::Store`, `StoreOp`, `perform_store_op`, sixteen `Event::Store...` variants), but every one of those operations is a VFS operation made through the run's store view.
  - "Store" is Lua sugar that maps `store.*` onto a VFS root. Names below the Lua layer should say VFS, so every later change in the owner's series is written against final names. This is change 1 of nine in the owner's series "Move the harness's I/O to the host in nine changes, dependencies first" (2026-09-30); that series is not required reading.
  - The Harness performs the effect through a `StorePerformer` seam on tokio's blocking pool, although the only production implementation forwards straight to the VFS.
  - `spawn_blocking` is tokio-only. Change 9 of the series (drop tokio from the Harness) needs the VFS effect answered without it, and the `StorePerformer` seam adds nothing once it is.
  - Affected: Host developers (the `Effect` vocabulary, `perform_vfs_op`, event kinds and run-log keys), Engine and Harness maintainers. Prompt authors and models see no change.
- Goals:
  - Every name on the effect path says Vfs. A residual search finds only the kept names listed under Technical Design.
  - The Harness answers `Effect::Vfs` on the spot. `StorePerformer`, `VfsStore` and `Performers.store` are gone.
  - The rename and the behavior change land as two separately reviewable changes, the rename first having no behavior change (see Decision Record).
- Non-goals:
  - No change to the Lua `store` table, `VfsRefBuilder::store`, `acquire_store`, the Lua protocol names, or the Lua-visible error kind `"store"`.
  - No offload hook for hosts, no new panic-to-error mapping, no trace warning, no merge of `VfsOp` with the existing `vfs::Op`, no new tools or tool tiers.
  - No Cicerone run, no push, and nothing under `vibe/` edited except the one `vibe/archdoc.md` sentence named under Technical Design.
- Success criteria: the residual searches in the Testing Plan return nothing relevant, the gates pass, two work commits exist on top of the starting HEAD, and the tree is clean.
- Constraints:
  - Repository `c:\Users\Vinnie\cursor\promptforge`, branch `master`. At planning time HEAD was `2ea6bb6b` with a clean tree (re-checked on 2026-10-01); note HEAD before starting, because it is the rollback point.
  - Never touch the sibling directories `promptforge2`, `promptforge3` or `promptforge-design`. Never edit an existing dated plan record under `vibe/`; `vibe/archdoc.md` is a living document and the one file there this change edits. Do not push.
  - Engine, Harness and Host are capitalized terms with one meaning each. Engine: `crates/promptforge/` and `crates/promptforge-internal/`; it parses a prompt and steps a run, and emits an effect whenever it needs a model reply, a tool result, a timer or a file. Harness: `crates/harness/` and `crates/harness-internal/`; it steps the Engine, performs every effect, returns each answer and keeps the run log. Host: an application such as Workshop that runs prompts through the Harness.
  - Prose: plain English, no em dashes, no double dashes outside code spans, error messages written for a model reader, comments only for non-obvious constraints.
  - Crates with an `## Invariants` marker keep every file at or under 500 lines (`crates/harness-internal/runner/src/effect_loop.rs` is 451). Source directories stay flat, with kebab-case sibling files wired by `#[path]`.
  - Workspace lints deny `unwrap_used` and `expect_used`, warn on `missing_docs` and `unreachable_pub`, and deny broken rustdoc links.
  - The facade pages are hand-edited. Repository policy requires a facade-page update run (Cicerone) for facade renames; the owner waived it for this change, as for the task-event removal. Do not edit the rule text in `AGENTS.md`.
  - Behavior changes ship with tests in the same change. Add no structural check (source parser, allowlist, count, ceiling).
- Open questions: None

## Functional Specification

After the change the Engine's file effect, its answer and record types, its operation and outcome types, its error kind, and its sixteen lifecycle events all say Vfs. The Harness performs a Vfs effect on the spot: no spawned task, no channel, and no performer trait. Event `kind` strings and run-log record keys change shape; nothing else observable changes. A backend panic ends the run `Cancelled`, as a panicking performer does today.

- Actors and workflows:
  - Host developer driving the Engine directly: answers `Effect::Vfs { access, op }` with `perform_vfs_op(&access, op)` from the facade, then resumes the run. Through the Harness there is nothing to do.
  - Event and log consumers: event `kind` strings change from `store_*` to `vfs_*`, and the run log's effect and answer record keys change from `"Store"` to `"Vfs"`.
  - Prompt authors and models: nothing changes. `store.*` and `pcall` error kind `"store"` behave as before.
- Inputs and outputs: the rename map is under Technical Design. No other API changes.
- States and validation (inline answering):
  - The Harness appends the effect record, performs the operation on its own thread, appends the answer record and resumes the run, all before it steps the Engine again.
  - A mixed step logs effect and answer records interleaved: `Chat_E, Vfs1_E, Vfs1_A, Vfs2_E, Vfs2_A, ..., Chat_A`.
  - If the backend panics, the Harness logs an error and answers `Dropped`; the Engine turns that into `Interrupted`, so the run ends `Cancelled`. This matches today's behavior when a performer panics.
  - Real-disk operations now block the Host's executor thread while they run. The owner accepted this.
- Errors and recovery:
  - If any step finds a consumer of a renamed item outside Rust and `vibe/`, or finds that `Run::step` does not behave as the Technical Design verified facts say, stop and report instead of working around it.
  - Nothing is pushed, so the HEAD noted before starting is the rollback point.
- Security and privacy behavior: None
- Acceptance criteria:
  - The residual searches in the Testing Plan return nothing relevant.
  - `crates/promptforge/public-api.txt` changes only by renaming items: no item is added or dropped.
  - The workspace gates in the Testing Plan pass after each of the two changes.
  - The Harness component line in `vibe/archdoc.md` no longer says "one performer per effect kind" and states that the Vfs effect is answered inline.
  - `git log --oneline` shows exactly two new work commits above the starting HEAD (plan-progress bookkeeping commits do not count, see Decision Record), and `git status` is clean.

</product-contract>
<implementation-contract>

## Technical Design

Two bodies of work. The rename work is mechanical and compiles only as a whole, because each crate's dependents break until the Harness crates are updated. The inline work is one local change in the Harness effect loop plus deletions, and it depends on the renamed names. Verified facts, locators and the file lists below were checked on 2026-10-01 at HEAD `2ea6bb6b`; line numbers are locators, so find the named symbol, because lines shift as edits land. The compiler and `rg` are the real pointers.

- Architecture:
  - Verified facts about the Engine and Harness, checked against the source:
    - Calling `Run::step` again is safe. With nothing ready but effects still pending it returns `Pending` with no effects, and the Engine reports its own stall only when it has nothing ready and nothing pending (`crates/promptforge-internal/engine/src/execute/scheduler/drive.rs`, lines 48-56). Resuming some effects of a batch before the rest is valid; an unknown or duplicate effect id ends the run with an internal error.
    - The VFS scope ends at `Step::Done` or when the scheduler drops (`drive.rs` lines 62-65, and `crates/promptforge-internal/engine/src/execute/scheduler.rs` near line 279), not when an answer lands. An inline operation therefore cannot hold an `Access` past its answer.
    - The Engine already works both ways. Its serial test driver (`crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs`) answers `Effect::Store` inline, and its tokio test driver (`crates/promptforge-internal/engine/src/test_support/tokio_driver.rs`) offloads to `spawn_blocking`.
    - A `Dropped` answer to a store effect is accepted by the Engine and becomes `Error::Interrupted` through the generic dropped path (`crates/promptforge-internal/engine/src/execute/scheduler/apply.rs`, lines 42 and 74).
    - `Effect::Store` carries `access: Arc<Access>`, and `Access` is `Send + Sync` (`crates/promptforge-internal/engine/src/execute/run/effect.rs` lines 104-109; `crates/promptforge-internal/vfs/src/operations.rs` line 145).
    - `Performers`, `StorePerformer` and `VfsStore` are not exported by the `harness` facade. `OutputError` is exported; `InputFileError` is not (it is reachable internally through `PrepareError::Input`).
    - Nothing outside Rust, markdown and `vibe/` names the renamed items except `crates/promptforge/public-api.txt`: Workshop, gateway and the guide have no hits, and there is no CHANGELOG.
    - No workspace `Cargo.toml` sets `panic = "abort"`, `harness-runner` depends on `tracing`, and no `catch_unwind` exists under `crates/harness-internal/` yet.
    - The pinned nightly `nightly-2026-09-05` (named in `crates/build-xtask/src/api/toolchain.rs`) and `cargo-nextest` 0.9.128 are installed.
  - Effect loop after the inline work. The rename work leaves the loop as it is today: the Vfs effect is still spawned on the blocking pool through `StorePerformer`.

    ```mermaid
    flowchart TD
        stepRun[run.step] --> commitEffect[commit_effect]
        commitEffect --> kindCheck{effect kind}
        kindCheck -->|"Chat, Tool, Timer"| spawnPerformer[spawn performer]
        kindCheck -->|"Vfs, inline"| inlineVfs[perform_vfs_op]
        inlineVfs --> answerNow["answer, resume"]
        answerNow --> anyInline{answered inline}
        spawnPerformer --> anyInline
        anyInline -->|yes| stepRun
        anyInline -->|no| awaitAnswer[await_answer]
        awaitAnswer --> stepRun
    ```

- Modules and interfaces:
  - Rename map. Rename to `Vfs` forms:
    - `Effect::Store`, `EffectAnswer::Store`, `EffectRecord::Store`, `AnswerRecord::Store` in `crates/promptforge-internal/engine/src/execute/run/effect.rs`. The records are externally tagged, so the run-log keys change from `"Store"` to `"Vfs"`.
    - `StoreOp` (defined in `crates/promptforge-internal/lua/src/protocol/request.rs`) to `VfsOp`, and `StoreOutcome` (in `crates/promptforge-internal/lua/src/protocol/answer.rs`) to `VfsOutcome`. Both are re-exported through the Engine and the facade `vfs` module. Their serialized variants (`Write`, `Append`, `Read`, `ReadNumbered`, `StrReplace`, `Delete`, `Glob`, `Exists`; `Unit`, `Text`, `Paths`, `Bool`) do not change.
    - `perform_store_op` to `perform_vfs_op` (`crates/promptforge-internal/engine/src/execute.rs`, `crates/promptforge-internal/engine/src/lib.rs`, the facade `vfs` module in `crates/promptforge/src/lib.rs`).
    - `RunErrorKind::Store` to `RunErrorKind::Vfs`: the variant and its one mapping line in `crates/promptforge-internal/engine/src/execute/error.rs`. Its doc must say it also covers "the run's handle declares no store".
    - The 16 `Event::Store{Write,Append,Read,ReadNumbered,Replace,Delete,Glob,Exists}{Succeeded,Failed}` variants (`crates/promptforge-internal/types/src/event.rs`, lines 250-280) to `Event::Vfs...`, and the 16 `lifecycle::STORE_*` constants (`crates/promptforge-internal/types/src/event-lifecycle.rs`, lines 75-90) to `VFS_*`. The wire kinds change from `store_*` to `vfs_*`.
    - Engine scheduler names on the effect path, all under `crates/promptforge-internal/engine/src/execute/scheduler/`: `dispatch_store`, `store_observations` and `classify_store_failure` (`dispatch.rs`); `Continuation::Store` and `accept_store` (`apply.rs`); `StoreContinuation` and the `Continuation::Store` variant (`pending.rs`).
    - Harness error variants `InputFileError::Store` (`crates/harness-internal/runner/src/files.rs`) and `OutputError::Store` (`crates/harness-internal/sessions/src/session/files.rs`) to `Vfs`. This goes beyond the series' explicit list but follows its rule "keep `store` only in Lua".
  - Keep as is:
    - The Lua `store` table, `VfsRefBuilder::store`, `VfsRef::acquire_store`, and the `store_view` VFS function.
    - Lua crate protocol names: `Request::Store`, `Answer::Store`, `parse_store`, `engine_globals-store.rs`, and the `store_*` functions and `store_op` field in `__impl_coro.lua`.
    - The Lua-visible `ErrorKind::Store` (string `"store"`), and the Engine's `Error::Store`, `Error::store`, `Error::store_op`, `store_error_value_fields` and the `"store"` status string from `blocked_on`. All are Lua-facing (`crates/promptforge-internal/engine/src/error/convert.rs`, `crates/promptforge-internal/engine/src/error/value.rs`, `crates/promptforge-internal/engine/src/execute/scheduler/dispatch.rs`).
    - Prompt fixture names `crates/promptforge-internal/engine/tests/prompts/execution/store-*.md`, and everything under `vibe/` except the one `vibe/archdoc.md` sentence the inline work updates.
  - Doc comments: the new `Effect::Vfs` doc must say it is one operation on the run's store view (the eight `store.*` calls) and that other code touching the VFS does not appear as this effect. The `VfsOp` doc must say how it differs from the existing `vfs::Op` (the policy and watcher operation kind). Revise the doc comment of every renamed item in the same change.
  - Inline answering, in `crates/harness-internal/runner/src/effect_loop.rs`, in the per-effect loop of `Driver::drive` (around line 188), shaped like this:

    ```rust
    let mut answered_inline = false;
    for (id, provenance, effect) in effects {
        self.commit_effect(id, &provenance, &effect).await?;
        if let Effect::Vfs { access, op } = effect {
            let answer = answer_vfs(access, op);
            self.commit_answer(id, &provenance, &answer).await?;
            self.run.resume(id, answer);
            answered_inline = true;
        } else {
            self.perform(id, provenance, effect);
        }
    }
    if answered_inline {
        continue;
    }
    if self.outstanding.is_empty() {
        return Err(DriveError::Stalled);
    }
    self.await_answer().await?;
    ```

    The decided and cancelled branch above it is unchanged: it already drops every effect through `drop_effect`.
  - Traps in that code:
    - `perform` currently starts with `let answer = Answering::new(self.tx.clone(), id);`. An `Answering` guard that is dropped without posting sends `Dropped` on the channel. The Vfs path must never create one.
    - `Effect` is an exhaustive enum, and `unwrap_used` and `expect_used` are denied. Keep `perform` total without a panic. Preferred shape: `perform` returns `Option<(Arc<Access>, VfsOp)>`, handing a Vfs effect back to `drive`, which answers it inline; the other kinds spawn as before and return `None`. Do not use a no-op arm for Vfs in `perform`: a silently ignored effect would surface only as a false `Stalled`. The `if let` above is a sketch, not a mandate, and another total shape is acceptable if it avoids both a panic and a silent drop.
    - Do not step between inline answers. Resume all of the batch's inline answers inside the loop, then step once, so cross-chain interleaving matches "answer all, then one step".
    - `continue` before the `Stalled` check is what prevents a false `Stalled` when every effect of a step was a Vfs effect. Keep `Stalled` for "nothing answered inline and nothing out".
  - `answer_vfs(access: Arc<Access>, op: VfsOp) -> EffectAnswer` lives in `crates/harness-internal/runner/src/effect_loop-answering.rs` (74 lines), replacing `perform_store`. It runs `perform_vfs_op(&access, op)` inside `catch_unwind(AssertUnwindSafe(..))`, drops the access, and returns `EffectAnswer::Vfs(result)`. On a panic it logs `tracing::error!` with the effect id and task, and returns `EffectAnswer::Dropped`. `Answering` stays for the async kinds. The helper lives there because `effect_loop.rs` has only 49 lines of headroom under the 500-line ceiling.
- File and public API changes:
  - Rename work, in dependency order. The build is red from the first rename until the Harness crates are updated; run `cargo check -p CRATE` for a crate only once its dependents are updated, and expect the first full green build after the Harness crates. The lists below come from planning-time surveys and a residual search (71 matching files); the compiler and `rg` are the real pointers.
    - Types, under `crates/promptforge-internal/types/src/`: rename the 16 variants in `event.rs` (inside the `events!` macro) and the constants in `event-lifecycle.rs`, and revise their doc comments. Update `event-tests.rs` (`lifecycle_of`, the golden near line 376, the `ALL` loops) and `emitter-tests.rs` (lines 78-81).
    - Lua, under `crates/promptforge-internal/lua/src/`: rename the two types where they are defined, their re-exports (`lib.rs`, `protocol.rs`) and all uses (`engine_globals.rs`, `engine_globals-store.rs`, `protocol/parse/store.rs`, `protocol/render.rs`, `error.rs`, and the `Effect::Store` mention in a doc comment in `coro.rs` near line 430). Switch `engine_globals-store.rs` to the `VFS_*` constants. Update the literal kinds in `tests-recording.rs` (lines 65-99) and the tests that name the old items (`tests/logging.rs`, `tests/section_vm.rs`, `tests/store_errors.rs`, `tests/store_reports.rs`). Leave the Lua protocol names alone.
    - Engine, under `crates/promptforge-internal/engine/src/`:
      - `execute/run/effect.rs`: the four types, `Effect::record`, the answer conversions and their docs. `execute.rs` and `lib.rs`: `perform_vfs_op` and its re-exports. `execute/protocol.rs`: the re-exports. `execute/error.rs`: the kind and its mapping. `lua.rs` and `error/convert.rs` name the old items too: rename what is on the effect path and keep the Lua-facing names.
      - `execute/scheduler.rs` and `execute/scheduler/` (`dispatch.rs`, `apply.rs`, `pending.rs`): the scheduler names, the `lifecycle::VFS_*` constants, and `Effect::Vfs`.
      - Test mirrors that fail silently if missed: the `Observation` variants in `test_support/recording/` (`observation.rs`, `forward.rs`, `forward-tests-variants.rs`), whose Display text must still snake_case to the wire kind (asserted in `execute/tests/observations.rs`, lines 56-59); the test drivers (`execute/tests/serial_driver.rs`, `test_support/tokio_driver.rs`); `execute/run/effect-tests.rs` (JSON keys, lines 160-201); and the other engine tests that name the old items (`execute/run/tests.rs`, `execute/tests/effects.rs`, `provenance.rs`, `preludes.rs`, `model_and_reply.rs`, `execute/tests/suite/vfs.rs`, `fanout.rs`, `exec_flow/store_failures.rs`, `exec_flow/run_setup.rs`).
    - Facade, `crates/promptforge/src/lib.rs`: the three re-export lines (lines 140-142, in `pub mod vfs`), plus the `effect` module's items.
    - Harness: `crates/harness-internal/runner/src/files.rs` (`InputFileError`, `perform_vfs_op`, `VfsOp`, `VfsOutcome`); `crates/harness-internal/sessions/src/session/files.rs` (`OutputError`); `crates/harness-internal/runner/src/performers.rs`, `performers-builtin.rs` and `effect_loop.rs` (types only: `StorePerformer::perform` takes `VfsOp` and returns `VfsOutcome`, and the loop's `Effect::Vfs` arm still spawns); `crates/harness-internal/plugins/tests/it/support.rs` (`EffectAnswer::Store`, `perform_store_op` and a doc comment); `crates/harness-internal/runner/tests/it/effect_loop.rs` (JSON keys at lines 154 and 158); `crates/harness-internal/models/tests/it/end_to_end.rs` (JSON keys, lines 245 and 276); `crates/harness-internal/runner/tests/it/prepare-files.rs` (line 136, `InputFileError::Store`); `crates/harness/tests/suite/launch.rs` (lines 23-28, `OutputError::Store`).
    - Build tooling: `crates/build-xtask/src/facade_shape-tests.rs` (line 197) holds `pub use promptforge_lua::StoreOp;` as a valid re-export example. Change it to the new name.
    - Docs (hand-edited):
      - `crates/promptforge/src/*.md`: 20 fenced doctest blocks name the old symbols (`lib.md` 5, `event.md` 3, `effect.md` 3, `vfs.md` 2, `transport.md` 2, and one each in `model.md`, `prompt.md`, `capabilities.md`, `timestamp.md`, `replay.md`). Hidden `#` lines break first.
      - `crates/promptforge/src/vfs.md` has headings `StoreOp`, `StoreOutcome` and `perform_store_op` (lines 462, 477, 540) that intra-doc links resolve against; rename headings and links together. Intra-doc links to the old names also sit in `lib.md`, `model.md`, `effect.md` and `capabilities.md` of the same directory, and in doc comments in `crates/promptforge-internal/engine/src/execute.rs`, `crates/promptforge-internal/engine/src/execute/run/effect.rs`, `crates/promptforge-internal/lua/src/lib.rs`, `crates/promptforge-internal/lua/src/protocol.rs` and `crates/harness-internal/plugins/tests/it/support.rs`.
      - `crates/promptforge/src/event.md` (lines 356-371) holds the 16-variant table. `crates/promptforge/src/effect.md` (lines 346 and 374) documents the serde `"Store"` keys, which become `"Vfs"`. The `vfs.md` sentence that the outcome's serde form is stable stays true, because only the type name moves.
      - `crates/harness/src/vfs.md` (lines 116, 120, 244, 248: `OutputError::Store` and the run-kind prose) and the `OutputError` row in `crates/harness/src/lib.md` (line 798).
      - `tools/cicerone/plans/promptforge.md` (lines 462-469) and `crates/promptforge-internal/lua/AGENTS.md` (line 8).
    - Public API: regenerate `crates/promptforge/public-api.txt` with `cargo +nightly-2026-09-05 xtask api --bless`. 112 lines mention Store (64 are the event lines) and all of them are renames; no kept name appears in the file. The diff must be renames only.
  - Inline work, in `crates/harness-internal/runner/` unless noted:
    - `src/effect_loop.rs` and `src/effect_loop-answering.rs`: the loop change, `answer_vfs`, and the removal of the blocking-pool spawn from `perform()`, as described under Modules and interfaces. Delete the `perform_store` and `spawn_blocking_tagged` imports. Leave `spawn_blocking_tagged` itself in `src/spawn.rs`: it is public, has its own test (`tests/it/spawn.rs`), and the build tooling has no dead-code check on it. Change 9 removes it.
    - `src/performers.rs` and `src/performers-builtin.rs`: delete the `StorePerformer` trait, the `VfsStore` type and its re-export, and the `Performers.store` field. Fix the module docs ("three performers" becomes two).
    - Update every consumer of `Performers.store`, `StorePerformer` and `VfsStore`: `src/prepare.rs` (lines 46 and 329-332) and the tests listed below. `crates/harness-internal/sessions/src/session/run.rs` and `crates/harness-internal/models/src/performer.rs` need no edit: neither names the store slot (checked 2026-10-01).
    - Reword the stale blocking-pool text: the module header of `src/effect_loop.rs` (lines 14-18) and the `drop_outstanding` doc (lines 251-256).
    - `vibe/archdoc.md` (repository root `vibe/`, not under `crates/`), the Harness component line (line 10): replace "one performer per effect kind" with wording that says the Harness has a performer for each chat, tool-call and timer effect and answers each Vfs effect inline in its effect loop. Change nothing else in the file: the `store.*` mention in invariant A9 and the "declared store root and the store view" phrase in the VFS layer line are Lua and VFS names that stay. Use the capitalized terms Engine, Harness and Host.
    - Tests, all under `tests/it/`:
      - `support.rs`: drop the `StorePerformer` impls (`Unused`, `UnitStore`, `SlowStore`) and the `store:` slot in `unused()`. Add a panicking test double built from the facade's `promptforge::vfs::Vfs` and `VfsAccess` traits (not a `Performers` field, which no longer exists): wrap `MemoryBackend` and its access, delegate every required method, and panic in one chosen operation. Mount it in the test's `VfsRef` so a store operation reaches it through the run's real store view. `AcquireContext`, `ExecId`, `VfsPath` and `VfsError` are also exported by the facade.
      - `effect_loop.rs`: the test `records_are_events_then_effects_then_answers_per_step` (line 100) sets `performers.store = Arc::new(UnitStore)` at line 103; rewrite it to run its store operation against a `VfsRef` built with `MemoryBackend`, as `prepare-files.rs` does. Delete `a_slow_store_operation_is_awaited_before_done` (line 395, its `performers.store = Arc::new(SlowStore {...})` at line 399; its premise, a blocking-pool operation joined on cancel, is gone) and fix the header comment that mentions it.
      - `performers.rs` (lines 188-210): the `the_vfs_store_performs...` test drops `VfsStore` and the `store` assignment (line 191) and now exercises the inline path.
      - New tests: (1) a run made only of Vfs operations completes with no `Stalled`, and the log has exactly one answer per effect; (2) a Vfs operation and a pending tool in one step: use a tool that blocks on a test-controlled gate, and a Vfs chain whose first operation is followed by a second effect. Assert that the second effect's records appear in the log before the gate opens. This proves the inline-resumed chain steps again before the loop awaits, with no timing assertion; (3) a panicking backend yields a recorded `Dropped` answer and does not unwind the run.
      - Unchanged: the other `prepare*.rs` tests and `crates/harness-internal/sessions/tests/it/session-files.rs` exercise staging and prompt-level `store.*`, not offload timing.
- Data, persistence, failure, security, and privacy constraints:
  - Two separate shape changes ship in the rename work: event kinds (`store_*` to `vfs_*`) and run-log record keys (`"Store"` to `"Vfs"`). No run logs exist, so there are no serde aliases and no migration.
  - Mixed steps now log effect and answer records interleaved. Nothing in the repository requires all of a step's effect records first (`crates/harness-internal/runner/src/lib.rs` says only that an answer is logged before its resume, and `assert_one_answer_per_effect` needs each answer after its effect).
  - A backend panic is logged and answered `Dropped`, so the run ends `Cancelled`; the panic never unwinds the effect loop. Real-disk operations block the Host's executor thread, and VFS operations from different chains run one at a time in issue order.
  - Files in crates with the marker stay at or under 500 lines. After the inline work, recount `effect_loop.rs` (451 today) and `effect_loop-answering.rs` (74 today).
  - Removal rules for the inline work: delete, never stub. Delete any test, helper or import that only the removed code used.
  - Security and privacy: no change. The inline path adds no new trust boundary; the operation runs against the same `Access` the performer received.

</implementation-contract>
<verification-contract>

## Testing Plan

The rename touches most crates and the compiler is its main check, so each of the two changes ends with the full gate set once. While working, run the touched crate's tests. The rename writes no new test, because a pure rename has no new behavior to pin and the existing suites are the check. The inline work adds three tests, rewrites two and deletes one, and its residual searches prove that no old name survives.

- Unit:
  - Rename work: no new test. The existing suites, mirrored by the renamed `Observation` variants, the `Event` variants and the Lua `tests-recording.rs` literals, are the check.
  - Inline work adds the three tests listed under Technical Design (Vfs-only run with no `Stalled`; Vfs chain resumes before a gated tool answers; panicking backend records `Dropped`), rewrites `records_are_events_then_effects_then_answers_per_step` and the `the_vfs_store_performs...` test, and deletes `a_slow_store_operation_is_awaited_before_done`.
  - While working, run `cargo nextest run --locked -p CRATE --all-features` for the touched crate; for `promptforge`, `promptforge-engine` or `harness` also run `cargo test --locked --doc -p CRATE`, since nextest does not run doctests. Crates for this change: `promptforge-types`, `promptforge-lua`, `promptforge-engine`, `promptforge`, `harness-runner`, `harness-sessions`, `harness-models`, `harness-plugins`, `harness`, `build-xtask`.
- Integration and end-to-end:
  - Gate set, run at the end of each change, one command at a time (shell notes are in the Project Survey):
    - `cargo fmt --all`, then `cargo fmt --all --check`
    - `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
    - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
    - `cargo test --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc` (the facade doctests are the likeliest break)
    - `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api; cargo doc --locked -p promptforge --no-deps; cargo doc --locked -p harness --no-deps; cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items; $env:RUSTDOCFLAGS=""` (the facade commands build without `--all-features`, and the last command checks intra-doc links in private Engine items that the first command skips)
    - `cargo xtask site --books-only` (the user guide docs gate)
    - `cargo +nightly-2026-09-05 xtask api --check` (in the rename work, run `--bless` first, then inspect the diff: renames only, no item added or dropped)
    - `cargo test --locked -p build-xtask`
    - `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only` (the nightly-only fixtures; `facade_shape-tests.rs` is edited)
    - `cargo clippy --locked -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, after each change so both are bisectable. Workshop names none of the renamed items, so a compile check is enough.
  - The existing `records_are_events_then_effects_then_answers_per_step` test must still pass with `"Vfs"` keys, which shows effect and answer adjacency for a serial store-then-tool run.
- Regression, security, and performance:
  - Residual searches, run from the repository root with `rg`, expected empty (judge any hit):
    - After the rename work: `rg -n -e "\bStoreOp\b" -e "\bStoreOutcome\b" -e "\bperform_store_op\b" -e "Effect::Store\b" -e "EffectAnswer::Store\b" -e "EffectRecord::Store\b" -e "AnswerRecord::Store\b" -e "RunErrorKind::Store\b" -e "Event::Store" -e "OutputError::Store\b" -e "InputFileError::Store\b" -e "STORE_(WRITE|APPEND|READ|REPLACE|DELETE|GLOB|EXISTS)" -e "store_(write|append|read|read_numbered|replace|delete|glob|exists)_(succeeded|failed)" -e "StoreContinuation" -e "Continuation::Store" -e "dispatch_store" -e "accept_store" -e "classify_store_failure" -e "store_observations" . -g "!vibe/**" -g "!target/**" -g "!target-msrv/**" -g "!node_modules/**"`
    - After the inline work, also: `rg -n -e StorePerformer -e VfsStore -e perform_store . -g "!vibe/**" -g "!target/**" -g "!target-msrv/**" -g "!node_modules/**"`. The only remaining `spawn_blocking_tagged` hits are `crates/harness-internal/runner/src/spawn.rs`, its test, and the invariant text in the runner's `lib.rs`.
    - After the inline work, `rg -n "one performer per effect kind" vibe/archdoc.md` returns nothing, and `git diff` of `vibe/archdoc.md` shows only the Harness component line changed.
  - The kept names (`Request::Store`, `Answer::Store`, `parse_store`, `Error::Store`, `ErrorKind::Store`, `acquire_store`, `store_view`, `VfsRefBuilder::store`, `store-*.md` fixtures) must still exist; do not rename them to satisfy a search.
  - Behavior checks for the inline work (the new tests): a Vfs-only run completes with no `Stalled` and one answer per effect; a Vfs operation beside a pending tool or timer resumes its chain before the loop awaits; a panicking backend records `Dropped` and does not unwind the run.
  - Performance: no measurement. The owner accepted that real-disk operations block the Host's executor thread and that chains' VFS operations run one at a time.
- Exit criteria:
  - Both changes pass the gate set and their residual searches.
  - `git log --oneline` shows two work commits above the starting HEAD (plan-progress bookkeeping commits do not count), nothing is pushed, and `git status` is clean.

</verification-contract>
<decision-record>

## Decision Record

The owner chose the rename scope and the waiver of the Cicerone run, accepted inline answering after hearing its costs, and asked that no trace warning be added. The rest follows from the owner's series and the source survey. The decisions below cover scope, change shape, inline answering and its costs, panic behavior, gates and documentation; each states its rationale.

- Decisions:
  - Rename scope: everything on the effect path, plus the shared types `StoreOp` and `StoreOutcome` and the two Harness error variants. Keep the Lua crate's own protocol names, the Lua-visible error kind `"store"`, and the Engine's Lua-facing `Error::Store`. Rationale: the owner's rule is "keep `store` only in Lua and in `VfsRefBuilder::store`", and the kept items are Lua's surface.
  - Two separate commits: a pure rename with no behavior change, and the inline behavior change. Rationale: the rename is compiler-checked and reviewable on its own, and the inline change is then a small diff. Subjects: "Rename the Store effect vocabulary to Vfs" and "Answer Vfs effects inline in the Harness". Each message is written from its staged diff. The plan authorizes committing each change without asking. Do not push.
  - Inline answering is accepted. The Harness performs the VFS operation on the thread that runs the loop. Rationale: the one production `StorePerformer` only forwards to the VFS, `spawn_blocking` is tokio-only, and change 9 removes tokio. Cost accepted by the owner: a real-disk operation blocks the Host's executor thread, and VFS operations from different chains run one at a time in issue order. Claims are happens-before based, so verdicts do not depend on timing.
    - Why inline is cheap to undo: the Engine does not care when an answer arrives, and its two test drivers already cover both timings. A revert touches one loop arm, one helper, an optional `Performers` field, and a few tests, and no public API (`Performers` is not in the facade).
    - After change 9 there is no `spawn_blocking`, so a later offload means designing a Host-supplied hook, not undoing a line. That design belongs to change 9.
    - Inline can spread. One store operation per call is cheap; a tool that performs thousands of VFS operations per call (a shell, for example) would block the executor for its whole duration. Do not copy this shortcut into tool code; such a tool needs its own offload story from the start.
  - No trace warning for slow inline operations (the owner's decision).
  - Resume all of a batch's inline answers, then step once. Rationale: it keeps cross-chain interleaving the same as "answer all, then one step". Stepping between answers would change it.
  - A backend panic is logged and answered `Dropped`, so the run ends `Cancelled`. Rationale: it matches today's guard behavior. Mapping a panic to a `VfsError::Backend` answer would be friendlier, but it is a behavior change outside this change.
  - `spawn_blocking_tagged` stays. Rationale: it is public, tested, and change 9 owns its removal.
  - Facade pages are hand-edited and no Cicerone run happens. Rationale: a run takes hours, and the owner waived it for this change.
  - The `Effect::Vfs` doc defines the effect narrowly: one operation on the store view. Rationale: other code (future tools that touch the VFS) will not appear as this effect, and an unqualified name would repeat the confusion that started this work ("the only Vfs effects are from the store").
  - Update `vibe/archdoc.md` in the inline work, limited to the Harness component line. Rationale: the owner asked that this plan edit it, because its "one performer per effect kind" sentence becomes false once the Vfs effect is answered inline. This is the one exception to leaving `vibe/` alone, and it applies to the living architecture document only, never to a dated plan record.
  - The gate set includes the user guide docs build, the Harness and Engine private-item docs builds, and the nightly-only `build-xtask` fixtures, in addition to the workspace gates. Rationale: the repository's docs and CI gates cover them, the rename edits private doc comments and a fixture in `crates/build-xtask/src/facade_shape-tests.rs`, and `cargo doc --workspace` does not check private items.
- Rejected alternatives:
  - Keeping `StorePerformer` as an optional seam for Hosts: it adds nothing today, and re-adding a seam later is cheap. Revisit with change 9.
  - Serde aliases or a deprecation path for the old names and keys: no run logs exist, and the API is pre-release.
  - Folding the Vfs effect into the tool-call path, or giving tools the chain's access handle: discussed as separate future work, not part of this change.
  - Running the Cicerone update: hours of work, waived.
- Assumptions, risks, and notes:
  - Assumption: the tree is clean at the start. Check `git status` first; stop if it is dirty.
  - Assumption: tracking this plan's progress may add new files under `vibe/` (a dated copy of this plan and an active-plan pointer, later removed by a closing commit). They are new records, not edits of an existing record, and their commits are bookkeeping, not work commits.
  - Risk: the rename is one compile unit across the types, Lua, Engine, facade and Harness crates, so expect several fix rounds. Work in the listed order.
  - Risk: three mirrors must change together or tests fail silently: the `Event` variants, the Engine's `Observation` test mirror, and the Lua crate's `tests-recording.rs` literals.
  - Risk: `VfsOp` sits beside `vfs::Op` (the policy and watcher operation kind). Say so in the `VfsOp` doc comment.
  - Risk: facade doctests with hidden `#` lines are the likeliest break; rerun them after each page edit.
  - Risk: the 500-line ceiling on `effect_loop.rs` (451 today).
  - Risk: the panicking test double must implement every required `VfsAccess` method; delegate to a `MemoryBackend` access and panic in one operation rather than hand-writing storage.
  - Risk: `cargo xtask site --books-only` may fail for a reason unrelated to this change; if it does, record the failure and report it instead of skipping the gate.
  - Risk: `vibe/archdoc.md` says the Harness has "one performer per effect kind", which the inline work makes false because the Vfs effect has no performer. The sub-agents that implement and review the work read `vibe/archdoc.md`, so it describes the pre-change Harness until the inline work edits it.
  - Note: site counts and line numbers come from planning-time surveys and a 2026-10-01 fact check, and may be off by a few. The fact check corrected the doctest block count to 20 and showed that two Harness files the earlier lists named need no edit.
  - Confidence: high. The rename is compiler-checked, and the re-step assumption is confirmed in the Engine source.

### Deferred and Out of Scope

- Deferred: a Host-supplied offload hook for blocking VFS work, until change 9 (dropping tokio).
- Deferred: any tool tier or tool-call path that touches the VFS, until the separate future work on the tool-call path.
- Out of scope: `c:\Users\Vinnie\cursor\promptforge2`, `promptforge3` and `promptforge-design`.
- Out of scope: anything else under `vibe/` (only the `vibe/archdoc.md` Harness component line is in scope), and the rule text in `AGENTS.md`.
- Out of scope: a new panic-to-error mapping, a merge of `VfsOp` with `vfs::Op`, structural checks for the old names, and pushing.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: None for this change. Clippy and nextest compile every target it touches, and the repository forbids a standalone `cargo check --workspace` beside clippy. CI builds only the gateway and workshop binaries (`cargo build --locked -p gateway`, `cargo build --locked -p workshop`), which this change does not touch.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features`, optionally narrowed with a test-name substring (the per-crate form of the repository's workspace command). Nextest does not run doctests, so for `promptforge`, `promptforge-engine` or `harness` also run `cargo test --locked --doc -p <crate>`.
- Component test command pattern: the same per-crate form, run for each touched crate.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests with `cargo test --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`. Workshop crates run separately: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
- Linter command: `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, and for Workshop `cargo clippy --locked -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
- Formatter check command: `cargo fmt --all --check`. Run `cargo fmt --all` first, because it rewrites files.
- Docs command: with `RUSTDOCFLAGS="-D warnings"`, `cargo doc --locked --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, `cargo doc --locked -p promptforge --no-deps` and `cargo doc --locked -p harness --no-deps` (both without `--all-features`), and `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`; user guide: `cargo xtask site --books-only`.
- Complete gate list: `AGENTS.md` (Verification section) and `.github/workflows/ci.yml`; the commands above are the ones this change needs.
- Other gates: `cargo +nightly-2026-09-05 xtask api --check` (the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs`; `--bless` regenerates `crates/promptforge/public-api.txt`), `cargo test --locked -p build-xtask`, and the nightly-only fixtures `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`. CI also runs unrelated gates (gateway, STT, npm typecheck and build, `cargo deny`, `cargo audit`) that this change does not touch, and fails a job whose build dirties the tree.
- Test placement and naming conventions:
  - The Engine keeps unit tests in `crates/promptforge-internal/engine/src/execute/tests/` (snake_case files by topic); the Lua crate keeps `*-tests.rs` siblings beside the module they test.
  - The `harness-internal` crates use one integration binary per crate under `tests/it/` with shared fixtures in `support.rs`; the `promptforge` and `harness` facades use `tests/suite/`.
  - Test names are descriptive snake_case sentences, for example `a_slow_store_operation_is_awaited_before_done`.
  - `clippy.toml` allows `unwrap` and `expect` in tests only.
- Directory map:
  - Top level: `.cargo`, `.config`, `.cursor`, `.githooks`, `.github`, `cabinet`, `crates`, `guide`, `images`, `local`, `prompts`, `target`, `target-msrv`, `tools`, `vibe`.
  - `crates/promptforge/` is the Engine's public facade (`src/*.md` pages, `public-api.txt`); `crates/promptforge-internal/` holds `engine`, `types`, `parser`, `lua`, `vfs` and `model-client`.
  - `crates/harness/` is the Harness facade; `crates/harness-internal/` holds `runner`, `models`, `plugins`, `log`, `sessions`, `web`, `webfetch` and `web-search`.
  - `crates/build-xtask/` holds the `xtask` commands (`api`, `site`) and the structural checks; `tools/cicerone/` holds the doc-generation plans; `vibe/` holds dated plan records and `vibe/archdoc.md`.
- Component boundaries: dependencies flow Host to Harness to Engine. The Harness depends only on the `promptforge` facade among product crates, and outside crates reach the Harness only through `harness`. The Engine parses and steps and never performs I/O. A crate inside a family container may depend only on crates at the `crates/` root and its own siblings. `promptforge`, promptforge-*, harness-*, gateway-* and workshop-* crates follow the product-boundary matrix enforced by `cargo test -p build-xtask`.
- Conventions summary: workspace lints forbid `unsafe_code` and deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used`; `missing_docs` and `unreachable_pub` warn; rustdoc broken links are denied. Files in crates with an `## Invariants` marker stay at or under 500 lines. Error and status messages are written for a model reader. JSON that reaches the run log round-trips exactly with sorted keys. `.gitattributes` sets `* text=auto eol=lf`. A plan cannot introduce a structural check without the owner's explicit approval.
- Crate package names for this change: `promptforge-types`, `promptforge-lua`, `promptforge-engine`, `promptforge`, `harness-runner`, `harness-sessions`, `harness-models`, `harness-plugins`, `harness`, `build-xtask`.
- Environment (Windows, PowerShell 5.1):
  - Toolchain: `rust-toolchain.toml` says `stable`, matching CI. The base commit `2ea6bb6b` expects Rust 1.99.0 or newer (it removed `#[expect(clippy::float_cmp)]` because 1.99.0 no longer flags those assertions), and the local `stable` was updated to 1.99.0 on 2026-10-01. If `rustc --version` reports older than 1.99.0, clippy fails in `crates/gateway/app/src/tray/logic-tests.rs`, which this change does not touch: update `stable` instead of editing the gateway crate.
  - Run cargo commands one at a time, because they share a target-directory lock. Do not append `2>&1` to cargo commands, because PowerShell turns cargo's stderr into error records; judge a cargo step by its result lines and `$LASTEXITCODE`.
  - Use `rg` in the shell for exhaustive searches; the Grep tool truncates long lines and ignores paths outside the workspace.
  - PowerShell redirection writes UTF-16, so write files that git or tools read with the Write tool (UTF-8, no byte-order mark). Set `$env:PYTHONIOENCODING="utf-8"` before running Python, and read and write Python files with `encoding="utf-8"` and `newline=""`.
  - git's "CRLF will be replaced by LF" warnings are benign (`eol=lf`).
  - `cargo fmt --all` rewrites files, so run it before the check. Never put an edit and the check that observes it in one parallel batch.
  - The pinned nightly for `xtask api` is named in `crates/build-xtask/src/api/toolchain.rs`; this plan writes `nightly-2026-09-05`, and if the file pins a different one at execution time, use that.

</project-survey>
<execution-plan>

## Execution Instructions

Before the first step: read this whole file, confirm `git status` is clean and the branch is `master`, and note `git rev-parse HEAD` (the rollback point). A finished step's heading gains ` [completed]`. Step 2 depends on Step 1: it uses the renamed names, and it deletes the `StorePerformer` seam that Step 1 keeps so that Step 1 stays a pure rename.

<step-1>

### Step 1: Rename the Store effect vocabulary to Vfs [completed]

- Component: none
- Artifacts: the types, Lua, Engine, facade and Harness crates and their tests; `crates/build-xtask/src/facade_shape-tests.rs`; the facade and Harness doc pages; `tools/cicerone/plans/promptforge.md`; `crates/promptforge-internal/lua/AGENTS.md`; `crates/promptforge/public-api.txt`. Every file and symbol is listed under Technical Design, "File and public API changes", rename work. Key symbols: `Effect::Vfs`, `EffectAnswer::Vfs`, `EffectRecord::Vfs`, `AnswerRecord::Vfs`, `VfsOp`, `VfsOutcome`, `perform_vfs_op`, `RunErrorKind::Vfs`, the 16 `Event::Vfs...` variants and `lifecycle::VFS_*` constants, `InputFileError::Vfs`, `OutputError::Vfs`.
- Work. The build is red until item 4 finishes, so do not chase intermediate compile errors in crates whose dependencies are not yet renamed. Do the mechanical rename with a script over the rename map (whole-word patterns, kept names excluded: `Request::Store`, `Answer::Store`, `Error::Store`, `ErrorKind::Store`, `acquire_store`, `store_view`, `VfsRefBuilder::store`), then fix the remainder by hand. The doc pages (item 5) can be edited by a parallel sub-agent once the rename map is fixed; run the doctests only after the code compiles.
  1. Types crate: rename the 16 event variants and the lifecycle constants, revise their docs, and fix `event-tests.rs` and `emitter-tests.rs`.
  2. Lua crate: rename `StoreOp` and `StoreOutcome` with their re-exports and uses, switch to the `VFS_*` constants, and fix the `tests-recording.rs` literals. Keep the Lua protocol names.
  3. Engine: rename the four effect and record types, `perform_vfs_op`, `RunErrorKind::Vfs` (and its mapping), the scheduler names, the `Observation` test mirrors, the test drivers, and the tests with JSON keys. Write the `Effect::Vfs` and `VfsOp` doc comments as Technical Design says.
  4. Facade `lib.rs` re-exports, then the Harness crates and their tests, then the `build-xtask` fixture. Keep `StorePerformer` and `Performers.store` for now, with their signatures moved to `VfsOp` and `VfsOutcome`; the effect loop still spawns the blocking task. This change is a pure rename.
  5. Docs: hand-edit the facade pages, the Harness pages, the Cicerone plan lines and the Lua `AGENTS.md` line. Run the facade doctests after each page edit.
  6. Run `cargo +nightly-2026-09-05 xtask api --bless` and read `git diff crates/promptforge/public-api.txt`: every removed line must pair with an added line that differs only by `Store` becoming `Vfs`. Then run `--check`.
- Tests: no new test. The coding agent runs the focused tests of every touched crate (`cargo nextest run --locked -p <crate> --all-features`, plus `cargo test --locked --doc -p <crate>` for `promptforge`, `promptforge-engine` and `harness`), the `xtask api --bless` diff check from item 6, and the "After the rename work" residual search, and fixes every hit that is not a kept name. It does not run the full gate set. The verification pass runs every command of the Testing Plan's gate set as written there, including the Workshop clippy line.
- Commit (made by the session that owns staging, not by the coding agent): stage everything and commit with the subject "Rename the Store effect vocabulary to Vfs", with the message written from the staged diff. Commit without asking. Do not push.

</step-1>
<step-2>

### Step 2: Answer Vfs effects inline in the Harness [completed]

- Component: none
- Artifacts: `crates/harness-internal/runner/src/effect_loop.rs`, `effect_loop-answering.rs`, `performers.rs`, `performers-builtin.rs`, `prepare.rs`, the tests `crates/harness-internal/runner/tests/it/support.rs`, `effect_loop.rs` and `performers.rs`, and `vibe/archdoc.md` (the Harness component line only). Key symbols: `Driver::drive`, `Driver::perform`, `answer_vfs`, `Answering`, `StorePerformer`, `VfsStore`, `Performers.store`. Every edit is listed under Technical Design, "File and public API changes", inline work.
- Work, in this order:
  1. Add `answer_vfs` to `effect_loop-answering.rs` and wire it into the per-effect loop of `Driver::drive` as the sketch shows. Heed the traps: no `Answering` guard on the inline path, `perform` stays total without a panic, resume the whole batch's inline answers and step once, and `continue` before the `Stalled` check.
  2. Remove the blocking-pool spawn and its imports. Delete the `StorePerformer` trait, `VfsStore`, its re-export and `Performers.store`, and update every consumer (`prepare.rs` and the runner tests).
  3. Tests: drop the `StorePerformer` fakes, add the panicking `Vfs` test double, rewrite `records_are_events_then_effects_then_answers_per_step` and the `the_vfs_store_performs...` test, delete `a_slow_store_operation_is_awaited_before_done` and fix its header comment, and add the three new tests. Run them before the docs.
  4. Docs: reword the `effect_loop.rs` module header and the `drop_outstanding` doc, fix the `performers.rs` module docs, and update the Harness component line in `vibe/archdoc.md` as Technical Design says.
  5. Recount `effect_loop.rs` and `effect_loop-answering.rs` against the 500-line ceiling; split first if needed.
- Tests: the coding agent runs the focused tests of every touched crate (`cargo nextest run --locked -p <crate> --all-features`, plus `cargo test --locked --doc -p <crate>` for `promptforge`, `promptforge-engine` and `harness`), both residual searches, and the `vibe/archdoc.md` check from the Testing Plan, and fixes every hit that is not a kept name. It does not run the full gate set. The verification pass runs every command of the Testing Plan's gate set as written there, including the Workshop clippy line.
- Commit (made by the session that owns staging, not by the coding agent): stage everything and commit with the subject "Answer Vfs effects inline in the Harness", with the message written from the staged diff. Commit without asking. Do not push.
- Final report to the owner: both commit hashes, the starting HEAD, any stop condition hit, any deviation from this plan, how each residual-search hit was judged, and the new text of the `vibe/archdoc.md` Harness component line.

</step-2>

</execution-plan>

