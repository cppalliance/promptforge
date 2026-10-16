---
name: Tool-call cap debt
overview: "Remove the one debt the tool-call-cap work added: the public Plugin contract still says only a cancel drops a stop-surviving call, but a withdrawal now drops it while the run goes on. Make Plugin::call the single statement of when the Harness drops a call, point the other public copies at it, and pin the changed occasion with one behavior test. Two doc ride-alongs join the same pass: a Plugin::call rule against waiting on another call of the same run (D1-14), and 'in flight or waiting' in RunControl::stop_round (D1-16)."
todos:
  - id: d1-1-docs
    content: "D1-1: make Plugin::call's # Invariants the full drop-occasion list; point the runvil-plugin crate doc and plugin-user-input (lines 33, 72-73) at it; make ToolDescriptor::survives_stop's field doc accurate. D1-14: add the no-waiting-on-another-call bullet to the same # Invariants. D1-16: 'in flight or waiting' in RunControl::stop_round (harness-control.rs line 51)"
    status: completed
  - id: d1-1-pin
    content: "D1-1: add cancelling_a_task_drops_its_question_to_the_operator to tests/it/harness-stop-cap.rs, holding Main's after round until the question is torn down"
    status: completed
  - id: verify
    content: Focused stop_cap tests, contract read-through, git grep for 'only a cancel', docs builds, docs-claims, API check with CARGO_BUILD_WARNINGS cleared, and the full exit gates
    status: completed
isProject: false
---

# Debt removal: the tool-call cap's drop contract

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository: `c:\Users\Vinnie\cursor\runvil`, branch `master`. Paths are relative to it, and line numbers were read at `9cdef9389`; search by name if they have moved.
  - Target: the three commits whose trailer is `Plan: vibe/2026-10-11-6-tool-call-cap.md`:
    - `1f7f88dd7` Report the tool calls each step withdraws
    - `1b8184f7d` Run at most the cap of tool calls, oldest first
    - `9cdef9389` Close plan: tool-call-cap
  - Baseline `bc49d9670`, the parent of the oldest target commit. Endpoint and disposition `9cdef9389`, which `origin/master` also points at. The worktree was clean and excluded.
  - The operator named no target. A repository-wide run would have covered no new work, because `origin/master` is at `HEAD`, so the target is the plan just executed.
  - Removal scope: D1-1, plus D1-14 and D1-16. Analysis rejected those two, but the user brought them into scope as ride-alongs to D1-1's doc pass.
- Cleanup goals:
  - Every public document that says when the Harness drops a Plugin call's future matches what the code does, so a Plugin author can rely on it.
  - The full list of drop occasions lives in one place, `Plugin::call`'s `# Invariants`. The other public copies state only their own fact and point there, so the next drop occasion needs one edit.
  - A behavior test pins the occasion that changed: the early end of a task drops its stop-surviving call while the run goes on.
  - D1-14: a Plugin author learns from `Plugin::call`'s invariants that a call must not wait for another call of the same run to finish, since the Harness now holds calls beyond its cap.
  - D1-16: a Host reads `RunControl::stop_round` without doubt about whether a stop reaches waiting calls.
- Non-goals:
  - No change to Harness or Engine behavior. A withdrawal keeps dropping stop-surviving calls.
  - No change to Stop, the cap, or what a stopped round returns.
  - No edit to `../runvil-docs`.
  - No deadlock detection for calls that wait on each other (D1-14 is a documented rule, not a mechanism).
  - The other rejected candidates stay as they are (Debt Inventory).
- Success criteria:
  - `Plugin::call`'s invariant names every drop occasion and says only a stop spares a stop-surviving call. The occasions are:
    - a stop;
    - the run's end, a cancel included;
    - the early end of the task that made the call: the task is cancelled, its owner ends, or an ancestor is aborted.
  - No public doc says "only a cancel drops it", or lists stop and cancel as the only occasions.
  - The new behavior test passes, and it would fail if a withdrawal spared stop-surviving calls.
  - D1-14: `Plugin::call`'s `# Invariants` has a bullet saying a call must not wait for another call of the same run to finish, because the Harness runs a bounded number of a run's calls at once and holds the rest.
  - D1-16: `RunControl::stop_round`'s doc says a stop reaches only the effects in flight or waiting when the loop sees it.
  - Every exit gate in the Testing Plan passes.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1, introduced by `1b8184f7d` on top of `1f7f88dd7`: a withdrawal drops a stop-surviving call while the run goes on, which contradicts the public Plugin contract.
    - Evidence, at `9cdef9389`:
      - `Reach::takes` (`crates/harness-internal/runner/src/effect_loop-flight.rs` lines 60-70) accepts any listed id for `Reach::Withdrawn` and ignores `survives_stop`. `Flights::abort` (lines 261-285) aborts each matching call in flight and drops each matching waiting call.
      - The Driver calls `drop_in_flight(Reach::Withdrawn(&withdrawn))` on every step that is not decided (`effect_loop.rs` lines 208-212), so the run goes on after the drop.
      - The Engine lists every tool call that `abort_effect` orphans (`crates/runvil-internal/engine/src/execute/scheduler/chain.rs` lines 297-307). That covers `tasks.cancel` (`cancel_task` to `abort_subtree`, `waits.rs` line 343), an owner ending with live tasks (`settle_owned_tasks` to `abandon_owned_tasks` to `abort_subtree`, `task_end.rs` lines 94-109 and 189), and an ancestor's abort (`abort_subtree`, `chain.rs` line 258). Script `tools.call` and model batches share the one `Continuation::ToolCall`, so `input.ask()` is listed too.
      - The ask tool sets `survives_stop(true)` (`crates/plugin-user-input/src/ask.rs` line 40).
      - Unchanged contract text:
        - `crates/runvil-plugin/src/plugin.rs` lines 84-86: "The Harness drops the returned future on a stop or a cancel, unless the tool's descriptor sets `survives_stop`, in which case only a cancel drops it."
        - `crates/plugin-user-input/src/lib.rs` line 33, and again at lines 72-73 under `## Invariants`: "only a cancel drops it."
    - Relationship to target work: the design record required "A withdrawn call never starts, and one already running is dropped", but it never listed the Plugin-facing docs among the files to update, and neither target commit touched them.
    - Impact:
      - A Plugin built outside this repository that marks a tool `survives_stop` and trusts "only a cancel drops it" may treat the drop as the run's end. It could then report a cancelled run to a person, or release run-wide state that later calls still need.
      - Nothing in the tree misbehaves today. Workshop's `WaitGuard` (`crates/workshop/agents/src/input-tool.rs` lines 20-47) closes each wait on drop, and the `InputBroker` invariant already requires that.
    - Reversal cost: low. Documentation in four files plus one test, with no behavior change.
    - Target state: the full list lives on `Plugin::call`, the other public copies point to it or state only their own fact, and a test pins the task's-early-end occasion for a stop-surviving call.
- Brought into scope by the user. Analysis rejected both, but each is a one-line doc edit in D1-1's pass:
  - D1-14, classed weak or speculative: tool calls that wait on each other can stall a run under the cap.
    - Evidence, at `9cdef9389`: before the target, every issued call ran at once. Now `Flights`' fill (`crates/harness-internal/runner/src/effect_loop-flight.rs` lines 161-174) runs at most the cap and holds the rest, oldest first.
    - Consequence: suppose a call can finish only after a later, waiting call of the same run makes progress, and the cap's worth of earlier calls all block that way. Then the run waits until a stop or a cancel.
    - Reach: no tool in the tree does this, but an MCP server with a producer and consumer tool pair could.
    - Why a doc: `ToolPerformer` is internal (the `harness` facade doesn't re-export it), so Plugin authors can't see the cap. `Plugin::call`'s `# Invariants` is where they read the rules.
    - Target state: one more bullet in that `# Invariants`.
  - D1-16, classed residual but acceptable: "only the effects in flight" in `RunControl::stop_round`.
    - Evidence: `crates/harness-internal/runner/src/harness-control.rs` lines 48-49, added by `1b8184f7d`, say a stop also drops waiting calls. Line 51, unchanged, says "A stop reaches only the effects in flight when the loop that drives the run sees it". The same commit made "in flight" and "waiting" separate states.
    - Target state: "only the effects in flight or waiting". The `harness` facade re-exports `RunControl`, so Hosts read this.
- Cheap fixes: none.
- Exposed pre-existing debt: none.
  - D1-19 is unrelated pre-existing debt: a run that ends without a cancel (a fatal answer, a stall, or a root that returns while a task waits) already dropped stop-surviving calls before the target.
  - D1-1 needs accurate wording anyway, and that wording covers D1-19 at no extra cost, so D1-19 gets no work item of its own.
- Rejected candidates: 18 more, beyond the two above that the user brought into scope.
  - Residual but acceptable, 8:
    - D1-2: the `withdrawn` field on the exhaustive `Step::Pending` variant. Its shape predates the target, and the user accepted public format changes.
    - D1-3: `EffectId`'s documented order, which codifies what the allocator already does.
    - D1-7: callers must handle `withdrawn` after `effects`. The order is documented and pinned on both sides.
    - D1-8: `Reach::Withdrawn` depends on a sorted list. The sort is documented and tested, and a miss would be harmless.
    - D1-13: stop-surviving calls count toward the cap. That's a chosen, tested trade-off.
    - D1-15: the task-cancel test is weaker than its name, but the same-step test catches the realistic regression.
    - D1-17: `effect_loop.rs` is at 499 lines. That's a size lead with no behavior consequence.
    - X1: three more copies of the occasion list are incomplete but say no "only". The public one, on `ToolDescriptor::survives_stop`, is updated in D1-1's pass anyway.
  - Weak or speculative, 2:
    - D1-6: two booleans on private `Flights::launch`, with two call sites and no bug.
    - D1-18: the default of 10 restated in prose and tests. They all agree today.
  - False, 7:
    - D1-4: `max_in_flight` is public only inside an internal crate.
    - D1-5: `Reach` is a private enum with one match.
    - D1-9: the Engine's withdrawn list is complete, has no repeats, and is empty once the run is decided.
    - D1-10: there's no stall or stranded waiting call, because waiting implies the cap's worth of calls are running.
    - D1-11: there's no lost wakeup for a call started inside `landed_now`.
    - D1-12: no performer future makes progress beyond the cap.
    - X2: nothing in the tree reads a dropped ask as a run-level event.
  - Unrelated pre-existing, 1: D1-19 (above).

</product-contract>
<implementation-contract>

## Technical Design

Documentation in five files plus one behavior test; no Harness, Engine, or Plugin behavior changes. The drop-occasion list moves to a single home, because these copies were written for two occasions (stop and cancel) and fell behind as occasions grew: a decided run without a cancel, waiting calls, and now a withdrawal.

- [crates/runvil-plugin/src/plugin.rs](crates/runvil-plugin/src/plugin.rs), the `# Invariants` bullet on `Plugin::call` (lines 84-86), becomes the full list. Suggested wording:

```text
- `call` is cancellation-aware. The Harness drops the returned future on
  a stop, when the run ends (a cancel included), and when the task that
  made the call ends early: it is cancelled, its owner ends, or an
  ancestor is aborted. Only a stop spares a call whose tool's descriptor
  sets `survives_stop`. After a stop or a task's early end, the run goes
  on.
```

  The same `# Invariants` gains one bullet for D1-14. It names no number, because the cap is the Harness's to set. Suggested wording:

```text
- `call` must not wait for another call of the same run to finish. The
  Harness runs a bounded number of a run's tool calls at once and holds
  the rest, so a call that waits on one held behind it can stall the run
  until a stop or a cancel.
```

- [crates/harness-internal/runner/src/harness-control.rs](crates/harness-internal/runner/src/harness-control.rs), for D1-16: in `RunControl::stop_round`'s doc, line 51 "A stop reaches only the effects in flight when the loop that drives the run sees it" becomes "A stop reaches only the effects in flight or waiting when the loop that drives the run sees it". Nothing else in that doc changes.
- [crates/runvil-plugin/src/lib.rs](crates/runvil-plugin/src/lib.rs), the crate-level bullet (lines 54-56), stops restating the list, for example: "[`Plugin::call`] is cancellation-aware: its future may be dropped before it finishes, on the occasions its invariants list."
- [crates/plugin-user-input/src/lib.rs](crates/plugin-user-input/src/lib.rs):
  - The overview sentence (line 33) becomes, for example: "The ask tool's descriptor survives stops: a stop leaves the question open. The other occasions [`Plugin::call`](runvil_plugin::Plugin::call) lists drop it, the early end of the task that asked included."
  - The `## Invariants` bullet (lines 72-73) becomes, for example: "The ask tool's descriptor sets `survives_stop`, so a stop leaves the operator's question open; every other occasion `Plugin::call` lists drops it."
- [crates/runvil-internal/types/src/tools/descriptor.rs](crates/runvil-internal/types/src/tools/descriptor.rs), the `survives_stop` field doc (lines 35-39), keeps its own sentence, made accurate:
  - Suggested: "A stop cancels every other call in flight and lets the run go on; the run's end and the early end of the task that made the call still end this one."
  - It can't link to `Plugin::call`, because `runvil-types` doesn't depend on `runvil-plugin`.
  - It is an Engine crate doc, so it names neither the Host nor how the Harness works (root `AGENTS.md`).
- [crates/harness-internal/runner/tests/it/harness-stop-cap.rs](crates/harness-internal/runner/tests/it/harness-stop-cap.rs) (177 lines) gains `cancelling_a_task_drops_its_question_to_the_operator`, through `Harness::run`:
  - Host and broker:
    - The Host is `with_asker(hold_host(&Arc::default()))`, with `operator.services()` from an `Operator` (`tests/it/scripted.rs`), a `PendingTimer`, and a `ScriptedBroker`.
    - The file's existing `calls_once` and `settle` helpers show the shape. `run_beside`, `until`, `answers_where`, and `calls` come from the parent `tests/it/harness.rs`.
  - Prompt:
    - The frontmatter declares `plugins: - user-input` and a `writer` model, with `models.default('writer')` in the H1 block.
    - `## Main` runs `local t = tasks.spawn('## Child')`, `models.infer('wait')`, `tasks.cancel(t)`, and `models.infer('after')`, then returns `'cancelled'`.
    - `## Child` runs `return (input.ask())`.
  - The broker holds back two of Main's rounds:
    - It answers the `wait` round only once the operator has been asked (`operator.asked == 1`).
    - It answers the `after` round only once the question has been torn down (`operator.abandoned == 1`).
  - Expect:
    - `RunOutcome::Completed { final_text: "cancelled" }`;
    - `operator.abandoned == 1`;
    - the ask's `ToolCall` answer recorded `Dropped` (`answers_where(&records, "ToolCall", calls(ASK_TOOL))`).
  - Why it pins the contract:
    - If a withdrawal spared stop-surviving calls, the question would stay open, and Main's `after` round would never be answered. The run would then never end, and `run_beside`'s 10-second `PATIENCE` would fail the test.
    - Holding `after` back is what makes the test meaningful. Without it, the run's end would drop the question through the decided branch's `Reach::All` and hide a regression.
    - The test passes at `9cdef9389` by design, because it pins the behavior being documented rather than a bug.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused, D1-1: `cargo nextest run --locked -p harness-runner --all-features harness::stop_cap::`. All three tests pass, the new one included.
- Contract check, D1-1:
  - Read the new `Plugin::call` bullet against `Reach::takes` (`effect_loop-flight.rs` lines 60-70), the Driver's withdrawal block (`effect_loop.rs` lines 208-212), and the decided branch's `drop_in_flight(Reach::All)` (`effect_loop.rs` around line 189).
  - Each occasion the bullet names must have a code path, and no code path may be left out.
  - `git grep -n "only a cancel" -- . ':(exclude)vibe'` returns nothing (exit code 1). It excludes `vibe/` because dated plan records, this plan's own copy included, quote the old text.
- Contract check, D1-14: read the new bullet against fill (`effect_loop-flight.rs` lines 161-174). The Harness holds calls beyond the cap and starts one only when a running call lands or is dropped, so a call that blocks on a held one keeps its slot.
- Contract check, D1-16: read the edited sentence against `act_on_stop` and `abort(Reach::AllButSurvivors)`. A stop seen by the loop takes the waiting calls that don't survive stops as well as those in flight, and nothing started after the loop saw it.
- Docs and wording, D1-1:
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`. Rustdoc's `broken_intra_doc_links` denies, so this checks the new `Plugin::call` links.
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`.
  - `node --test crates/workshop/ui/test/docs-claims.mjs`, which scans `## Invariants` wording and the Engine wording rule.
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`, because `ToolDescriptor` is facade surface. Clear the variable first: if `CARGO_BUILD_WARNINGS='deny'` is still set from a clippy run, nightly cargo's `workspace-hack` manifest warnings become fatal.
- Regression, unchanged behavior: every existing test passes untouched. That includes `tests/it/effect_loop-cap.rs`, `tests/it/harness-stop.rs`, `crates/plugin-user-input/tests/it/ask.rs`, and the Workshop `server/tests/it/agents/turns.rs` ("a stop cancels no question").
- Exit gates:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - The Workshop crates, with the gateway sidecar staged:
    - Stage it: `cargo build --locked -p gateway --no-default-features`, then `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`.
    - Run `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`.
    - Always run `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc` afterward.
  - `cargo fmt --all --check`
  - `cargo test -p build-xtask`
  - The docs, wording, and API checks above.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - Fix the documents, not the behavior. The tool-call-cap design record already decided that "A withdrawn call never starts, and one already running is dropped". The documents are catching up, and undoing the edit would be another doc edit, so this is not treated as a hard-to-reverse public-interface change.
  - Give the occasion list one home, `Plugin::call`'s `# Invariants`.
    - The `runvil-plugin` crate doc and `plugin-user-input` keep only their own fact (a stop leaves the question open) and point to `Plugin::call`.
    - The consequence is that one edit covers the next drop occasion.
    - The record shows the cost of the old way: D1-1, D1-19, and X1 are three copies that fell behind the same list.
  - `ToolDescriptor::survives_stop` keeps its own accurate sentence, because `runvil-types` can't depend on `runvil-plugin`.
  - Pin the changed occasion with a behavior test, in `harness-stop-cap.rs` (177 lines) rather than `effect_loop-cap.rs` (461 lines). It uses the real fixture ask tool, so it pins exactly what the operator-question docs promise.
  - D1-19's case is covered by the accurate wording, with no work item of its own.
  - D1-14 is a documented rule, not a Harness mechanism. No tool in the tree waits on another call of the same run, and the Harness can't see what a call waits on.
- User-resolved choices:
  - No architecture choices were needed. The behavior was settled in the tool-call-cap plan.
  - The user brought D1-14 and D1-16 into scope as ride-alongs to D1-1's doc pass, after analysis had rejected them.
- Rejected alternatives:
  - Make `Reach::Withdrawn` spare stop-surviving calls, so the old "only a cancel drops it" stays true.
    - A cancelled task's question would then stay open with nobody to use the answer, and it would hold a slot under the cap.
    - It would also contradict the tool-call-cap design record.
  - Restate the full list in every copy. Reason: that's the drift that produced this finding.
  - Leave the documents as they are. Reason: the public contract would stay contradicted.
  - Ride-alongs left out: the internal `ToolPerformer::survives_stop` doc and the `InputBroker` example sentence. Neither says "only", and neither misleads.
  - Detecting or breaking waits between calls of one run inside the Harness (D1-14). Reason: no tool in the tree does it, and the Harness can't see what a call waits on.
- Assumptions and risks:
  - The impact on Plugins outside this repository is inferred, not observed. The user said earlier that no one depends on the public formats yet.
  - The `../runvil-docs` book was not examined for further copies of the contract.
  - Two rejections, D1-11 and D1-12, rest on `futures-util`'s documented `FuturesUnordered` behavior. The pinned source was not re-read.

### Deferred and Out of Scope

- Copies of the drop contract in `../runvil-docs`. Revisit at the next docs sync.
- D1-15, a stronger task-cancel test in `effect_loop-cap.rs`, where the main chain keeps working after the cancel so the run's end can't hide a missed drop. Revisit the next time that file changes.
- D1-17, splitting `crates/harness-internal/runner/src/effect_loop.rs` (499 lines) before its next change. Revisit with the next Driver change.
- D1-2, `#[non_exhaustive]` on `Step::Pending`. That choice predates the target. Revisit before the facade's first stable release.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the gateway (the workspace default member); the desktop app is `cargo build --locked -p workshop`. Crate build scripts bundle the TypeScript UIs, so a fresh checkout first needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`; drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. A single JS test file: `node --test <path-to-file.mjs>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, with the same `--all-features` exception for the three workshop crates. Workshop UI packages: `npm test --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs`. The workspace run includes `build-xtask`, which holds the boundary and structural checks.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features` for the headless build shape. On PowerShell set `$env:CARGO_BUILD_WARNINGS='deny'` first. Never run a standalone `cargo check --workspace`.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus `RUSTDOCFLAGS="-D warnings" cargo doc -p runvil --no-deps` for the facade with default features. When the `runvil` facade's public surface changes, also run `cargo +<nightly pinned in crates/build-xtask/src/api/toolchain.rs> xtask api --check`, which compares against `crates/runvil/public-api.txt`.
- Test placement and naming conventions:
  - Unit tests live in a sibling file wired from the module as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `effect.rs` beside `effect-tests.rs`). A module with three or more test files uses a `tests/` subdirectory instead (for example `runvil-internal/vfs/src/handle/tests/`). Some older files keep an inline `mod tests { }`.
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` for the `runvil` and `harness` facades, `tests/it/main.rs` for the gateway app, workshop server, and `plugin-mcp`. Each topic is a `mod` file; shared helpers sit in `support.rs` or `common/`. Prompt fixtures live under `tests/prompts/` (`valid`, `invalid`, `execution`).
  - Test functions are snake_case sentences stating the behavior, such as `a_run_cancelled_before_it_begins_ends_cancelled_and_is_never_recorded`; a `test_` prefix is rare.
  - Test-only hooks sit behind a `test-fixtures` Cargo feature (gateway and workshop crates) or `test-support` (Engine crates and `runvil-plugin`); `build-xtask` checks they do not leak into normal builds.
  - JS tests are `*.test.mjs` beside the source or `*.mjs` under `test/`, run by `node --test`.
- Directory map:
  - `crates/`: all code. Top-level public crates (`runvil`, `runvil-plugin`, `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`), Plugins (`plugin-web`, `plugin-mcp`, `plugin-user-input`), shared utilities (`shared-error-source`, `shared-loopback`, and `shared-ui`, a TypeScript and CSS package rather than a crate), tooling (`build-xtask` for structural checks and `cargo xtask`, `build-workshop` behind the `cargo workshop` alias, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`), and `workspace-hack` for cargo-hakari.
  - `crates/runvil-internal/`: private Engine crates `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`.
  - `crates/harness-internal/runner`: the private Harness crate `harness-runner`.
  - `crates/gateway/`: private gateway crates; `app` is the `runvil-gateway` binary, beside `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and the `stt/` speech subsystem whose public member is `api` (`gateway-stt`).
  - `crates/workshop/`: the Workshop Host; `desktop` (Tauri app, package `workshop`), `server` and `server-api`, tier crates `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, `workspace`, and the npm workspaces `ui`, `look`, `platform`.
  - `guide/`: user guide and docs site sources. `prompts/`: example prompts. `tools/`: Node scripts for gateway sidecar staging and live TTS checks, each with a `.test.mjs`. `vibe/`: dated plan and run records. `local/`: gitignored machine-local gateway config and fixtures.
  - `.github/workflows/`: CI, release, and nightly jobs. `.githooks/`: pre-commit runs fmt; pre-push runs docs-claims, the headless check, workspace clippy, and cargo-deny. `.config/`: nextest and hakari config. `.cursor/rules/`: Workshop architecture and SPA rules.
- Component boundaries (enforced by `cargo test -p build-xtask`):
  - Engine crates (`runvil*`) depend on no gateway, workshop, Harness, or Plugin crate. Other families reach the Engine only through `runvil` and `runvil-plugin`; `crates/runvil-internal/` is private to those two.
  - Harness crates (`harness*`) depend outside their family only on `runvil`, `runvil-plugin`, and `workspace-hack`. Others reach the Harness only through `harness` and `harness-gateway-client`; `harness-runner` is private to `harness`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*`, and outside libraries. The only product crate allowed to depend on a Plugin is `harness-gateway-client`, which implements `plugin-web`'s `SearchProvider`.
  - Gateway crates depend on no runvil, workshop, or Harness crate. Their container crates are private to the family, and Workshop sees the gateway only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop tiers flow one way: server, then features, then services, then vocabulary. Same-tier crates do not depend on each other, except `workshop-registry` on `workshop-protocol`. Each workshop crate root names its tier. The desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`.
  - `shared-*` crates depend on no product crate.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized terms with exactly one meaning each (AGENTS.md Definitions). Engine crates say "the caller" and never mention the Host. `docs-claims.mjs` enforces the wording in every AGENTS.md, `## Invariants` crate doc, and `.cursor/rules` file.
  - Every member inherits strict workspace lints: clippy `all` and `pedantic` at deny, `unwrap_used` and `expect_used` denied, `#[allow]` banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` warned. `clippy.toml` bans installing process-global state (panic hook, logger, tracing subscriber, crypto provider) outside binary entry points.
  - Crate roots open with a `//!` doc, many with a `## Invariants` section.
  - Source directories are flat: a subdirectory needs three or more files, otherwise use `parent-label.rs` siblings wired with `#[path]`; convert in either direction when touching a group on the wrong side of the line.
  - Dependencies are declared once in `[workspace.dependencies]`, with comments explaining each pin and disabled feature.
  - Error and status messages are written for model consumption: concise, naming what is missing with required versus actual. Comments explain only non-obvious constraints, and platform workarounds cite the upstream issue URL.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder must round-trip exactly (`serde_json` with `float_roundtrip`, sorted keys, finite numbers).
  - Long-running gateway work reports through `gateway-progress`. Workshop UI CSS uses only `@workshop/look` tokens, and persisted UI state goes through `ui-storage` to `ui-state.json` or the workspace file.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Make Plugin::call the one statement of when a call is dropped [completed]

- Component: `none`
- Tests:
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`
  - `git grep -n "only a cancel" -- . ':(exclude)vibe'`, which must print nothing (exit code 1)

This step changes documentation only. It covers D1-1's contract text plus the two ride-alongs, D1-14 and D1-16. No Harness, Engine, or Plugin behavior changes: a withdrawal keeps dropping stop-surviving calls, and Stop, the cap, and what a stopped round returns stay as they are. Don't edit `../runvil-docs`, and don't make `Reach::Withdrawn` spare stop-surviving calls. Line numbers are at `9cdef9389`; search by name if they have moved.

- `crates/runvil-plugin/src/plugin.rs`, `Plugin::call`'s `# Invariants`:
  - Replace the cancellation-aware bullet (lines 84-86, ending "in which case only a cancel drops it") with the full drop-occasion list, using the Technical Design's suggested wording. It must name every occasion: a stop; the run's end, a cancel included; and the early end of the task that made the call (the task is cancelled, its owner ends, or an ancestor is aborted). It must say that only a stop spares a call whose tool's descriptor sets `survives_stop`, and that the run goes on after a stop or a task's early end.
  - Add a D1-14 bullet to the same `# Invariants`: a call must not wait for another call of the same run to finish. The reason is that the Harness runs a bounded number of a run's tool calls at once and holds the rest, so a call that waits on one held behind it can stall the run until a stop or a cancel. Name no number, because the cap is the Harness's to set.
- `crates/runvil-plugin/src/lib.rs`, the crate-level "[`Plugin::call`] is cancellation-aware" bullet (lines 54-56): stop restating the occasions and point at `Plugin::call`'s invariants, as the Technical Design suggests.
- `crates/plugin-user-input/src/lib.rs`:
  - The overview sentence (line 33): keep only this crate's own fact, that a stop leaves the question open, and say every other occasion [`Plugin::call`](runvil_plugin::Plugin::call) lists drops it, the early end of the task that asked included.
  - The `## Invariants` bullet (lines 72-73): the ask tool's descriptor sets `survives_stop`, so a stop leaves the operator's question open; every other occasion `Plugin::call` lists drops it.
- `crates/runvil-internal/types/src/tools/descriptor.rs`, the `survives_stop` field doc (lines 35-39): replace "a cancel still ends this one" with an accurate sentence of its own, for example "A stop cancels every other call in flight and lets the run go on; the run's end and the early end of the task that made the call still end this one." Don't link to `Plugin::call`, because `runvil-types` doesn't depend on `runvil-plugin`. This is an Engine crate doc, so it names neither the Host nor how the Harness works. Leave the `survives_stop` builder method's doc (lines 69-70) as it is.
- `crates/harness-internal/runner/src/harness-control.rs`, `RunControl::stop_round`'s doc, line 51: "A stop reaches only the effects in flight when the loop that drives the run sees it" becomes "A stop reaches only the effects in flight or waiting when the loop that drives the run sees it". Nothing else in that doc changes.
- Leave the internal `ToolPerformer::survives_stop` doc (`crates/harness-internal/runner/src/performers.rs`) and the `InputBroker` example sentence unchanged. Neither says "only", and neither misleads.
- Every `.rs` file stays at or under 500 lines; each crate's build script fails a file over 500.

Acceptance:

- No public doc says "only a cancel drops it", or lists a stop and a cancel as the only occasions. The `git grep` excludes `vibe/` because dated plan records, this plan's seed copy included, quote the old text.
- D1-1 contract check: read the new `Plugin::call` bullet against `Reach::takes` (`crates/harness-internal/runner/src/effect_loop-flight.rs` lines 60-70), the Driver's withdrawal block (`effect_loop.rs` lines 208-212), and the decided branch's `drop_in_flight(Reach::All)` (`effect_loop.rs` around line 189). Each occasion the bullet names has a code path, and no code path is left out.
- D1-14 contract check: read the new bullet against fill (`effect_loop-flight.rs` lines 161-174). The Harness holds calls beyond the cap and starts one only when a running call lands or is dropped, so a call that blocks on a held one keeps its slot.
- D1-16 contract check: read the edited sentence against `act_on_stop` and `abort(Reach::AllButSurvivors)`. A stop seen by the loop takes the waiting calls that don't survive stops as well as those in flight, and nothing that started after the loop saw it.
- Rustdoc's `broken_intra_doc_links` denies under `-D warnings`, so the first docs build checks the new `Plugin::call` links. `docs-claims.mjs` scans `## Invariants` wording and the Engine wording rule. The API check runs because `ToolDescriptor` is facade surface. Clear `CARGO_BUILD_WARNINGS` first, as written, because a leftover `deny` makes nightly cargo's `workspace-hack` manifest warnings fatal.

</step-1>

<step-2>

### Step 2: Pin that cancelling a task drops its question to the operator [completed]

- Component: `none`
- Tests:
  - `cargo nextest run --locked -p harness-runner --all-features harness::stop_cap::`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `cargo build --locked -p gateway --no-default-features`
  - `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/runvil-gateway.exe`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`, always, even when a Workshop command fails
  - `cargo fmt --all --check`
  - `cargo test -p build-xtask`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `Remove-Item Env:CARGO_BUILD_WARNINGS -ErrorAction SilentlyContinue; cargo +nightly-2026-09-05 xtask api --check`
  - `git grep -n "only a cancel" -- . ':(exclude)vibe'`, which must print nothing (exit code 1)

The first command is this step's focused test: all three `stop_cap` tests pass, the new one included. The rest are the plan's exit gates, run here because both steps are in by now.

Add `#[tokio::test] async fn cancelling_a_task_drops_its_question_to_the_operator` to `crates/harness-internal/runner/tests/it/harness-stop-cap.rs` (177 lines). It drives the run through `Harness::run` with `run_beside`, as the Technical Design describes. The test passes at `9cdef9389` by design, because it pins the behavior Step 1 documents rather than a bug. No production code changes. In place of a failing-first run, check that it fails when `Reach::takes` (`crates/harness-internal/runner/src/effect_loop-flight.rs` lines 60-70) spares stop-surviving calls for `Reach::Withdrawn`, then restore that file exactly.

- Host and broker:
  - Write `let (operator, _answers) = Operator::new();` and keep the sender bound for the whole test. If it is dropped, `Operator::wait` fails with "the operator left" instead of being torn down, and `abandoned` stays 0.
  - Build `Harness::new(recorder.clone(), Arc::new(broker), Arc::new(PendingTimer::default()), Arc::new(with_asker(hold_host(&Arc::default()))), operator.services())` with a `MemoryRecorder`.
  - Follow the shape of the file's existing tests and helpers (`calls_once`, `settle`). `run_beside`, `until`, `answers_where`, and `calls` come from the parent `tests/it/harness.rs`. `Operator`, `ScriptedBroker`, `hold_host`, `with_asker`, and `reply` come from `crate::scripted`.
- Prompt: the frontmatter declares `plugins: - user-input` and a `writer` model, with `models.default('writer')` in the H1 block.
  - `## Main` runs `local t = tasks.spawn('## Child')`, `models.infer('wait')`, `tasks.cancel(t)`, and `models.infer('after')`, then returns `'cancelled'`.
  - `## Child` runs `return (input.ask())`.
- The `ScriptedBroker` tells Main's two rounds apart by the last message's content, and holds both back:
  - It answers the `wait` round only once the operator has been asked (`operator.asked` reaches 1).
  - It answers the `after` round only once the question has been torn down (`operator.abandoned` reaches 1).
- Expect:
  - `report.outcome` is `RunOutcome::Completed { final_text: "cancelled" }`.
  - `operator.abandoned` is 1.
  - The ask's `ToolCall` answer is recorded `Dropped`: `answers_where(&records, "ToolCall", calls(ASK_TOOL))` equals `vec![json!("Dropped")]`.
- Why it pins the contract, so the hold-backs must not be weakened:
  - If a withdrawal spared stop-surviving calls, the question would stay open and Main's `after` round would never be answered. The run would never end, and `run_beside`'s 10-second `PATIENCE` would fail the test.
  - Holding `after` back is what makes the test meaningful. Without it, the run's end would drop the question through the decided branch's `Reach::All` and hide a regression.
- Update the file's `//!` doc (lines 1-3) and the `stop_cap` clause of the parent module doc in `tests/it/harness.rs` (lines 7-8) so they also name this case: a task's early end drops its stop-surviving call while the run goes on.
- Leave every existing test untouched. They must still pass, including `tests/it/effect_loop-cap.rs`, `tests/it/harness-stop.rs`, `crates/plugin-user-input/tests/it/ask.rs`, and the Workshop `server/tests/it/agents/turns.rs` ("a stop cancels no question").
- Every `.rs` file stays at or under 500 lines; each crate's build script fails a file over 500.

</step-2>

</execution-plan>
