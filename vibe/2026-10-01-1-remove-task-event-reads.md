---
name: Remove task event reads
overview: "Delete the task history read completely, with no trace outside vibe/: the Lua tasks.events function, the model's task_events built-in, the TaskEvents effect and its records, the harness performer and the log's per-task read path, guide chapter 16, and every test, doc, comment and example that mentions them. Events are still emitted and logged; only reading them back is removed. The work lands as three commits, one per step, with no Cicerone run and no push. Self-contained: a fresh agent can execute it from this file alone."
todos:
  - id: step-1-task-history-code
    content: "Step 1: remove the Engine, Lua protocol and Harness runner code, tests and prose, hand-edit the facade pages, bless public-api.txt"
    status: pending
  - id: step-2-run-log-read-path
    content: "Step 2: remove the harness-log per-task read path and fix its tests"
    status: pending
  - id: step-3-guide-chapter
    content: "Step 3: delete guide chapter 16, renumber 17 and 18, fix every link and per-site sentence (including crates/harness/src/vfs.md), regenerate exports, run the residual searches"
    status: pending
isProject: false
---

# Remove task event reads

<product-contract>

## Product Requirements

A task can currently read the history of events another task reported, through a Lua function, a model built-in and an Engine effect that the Harness answers from the run log. The owner decided on a complete cut, because nothing else uses the feature and it forces the Harness log to be readable during a run. Events are still emitted and logged; only reading them back is removed, with no trace left outside `vibe/`.

- Problem and users:
  - The read path forces the Harness log to be readable during a run. That blocks planned work on a host-owned log and on dropping tokio from the Harness.
  - Nothing in the repository or outside it uses the feature besides its own tests and docs. The only Lua callers are prompts embedded in Rust tests (`crates/promptforge-internal/engine/src/execute/tests/task_events.rs`, `crates/harness-internal/runner/tests/it/performers.rs`).
  - Affected users are prompt authors (Lua), models (built-in tools), Host developers, Engine and Harness maintainers, and guide readers.
- Goals:
  - Remove all three entry points and the mechanism behind them in the Engine, the Lua protocol, the Harness runner and the Harness log.
  - Remove guide chapter 16 and every test, doc, comment and example that mentions the feature, so no trace remains outside `vibe/`.
  - Leave the verification green and the work as three new commits, one per step.
- Non-goals:
  - No redesign or replacement of the read. If it returns it will be redesigned.
  - No stub, alias, deprecation note, comment or retired-name guard for the removed names, and no structural check or retired-symbol seed for them.
  - No change to event emission, the `Event` variants, `provenance.seq`, or the log's stored `task_id` and `task_seq`.
  - No push and no Cicerone run.
- Success criteria: The residual searches in the Testing Plan return nothing relevant, the verification in the Testing Plan passes, `git status` is clean, three work commits exist (plus the run tool's closing commit), and nothing is pushed.
- Constraints:
  - Repository `c:\Users\Vinnie\cursor\promptforge`, branch `master` tracking `origin/master`. Never touch `c:\Users\Vinnie\cursor\promptforge2`, `promptforge3` or `promptforge-design` (a separate repo of research notes). Never edit an existing record under `vibe/` (dated plan records). The run tool's own additions there (this plan's dated copy and the `vibe/ACTIVE` pointer) are allowed, and the residual searches skip `vibe/`. Do not push.
  - Write Engine, Harness and Host capitalized, with these meanings only. Engine: the `promptforge` crates under `crates/promptforge/` and `crates/promptforge-internal/`, which parse a prompt, step a run and emit effects. Harness: the `harness` crates under `crates/harness/` and `crates/harness-internal/`, which step the Engine, perform effects, return answers and keep the run log. Host: an application such as Workshop or Papergate that runs prompts through the Harness and makes every policy decision.
  - Prose: plain English, no em dashes, no double dashes in prose (flags inside code spans are fine), guide code fences open with four backticks, one paragraph per line in guide Markdown.
  - Structure: 500-line file ceiling in crates that have an `## Invariants` marker (split first, then edit; the Engine states the marker in `crates/promptforge-internal/engine/src/lib.rs`), flat source directories, error messages written for a model reader.
  - The Cicerone update run that the repository requires when a facade item is removed (`AGENTS.md`, line 98) is waived by the owner for this change, because a run takes hours. Hand-edit the facade pages and leave the rule text in `AGENTS.md` untouched.
- Open questions: None

## Functional Specification

After the change a prompt author, a model and a Host see the same task features minus the history read, and the Engine and Harness stop answering any request for a task's events. Everything that records events keeps working unchanged. The guide teaches authors who can no longer read events.

- Actors and workflows:
  - Prompt author: `tasks.events` is gone. `tasks.status`, `tasks.note` and the author checkpoint `log(message)` (guide chapter 05) stay.
  - Model: the task tools become four, in this fixed order: `task`, `task_cancel`, `task_status`, `await_tasks`. Completion notices stay.
  - Engine: the effect kinds become four: `Chat`, `ToolCall`, `Store`, `Timer`.
  - Harness: the runner supplies three performers (`TokioTimer`, `VfsStore`, `ActivatedTools`). The Host still logs every event, and the run log and `Session::transcript` keep working.
  - Guide reader: chapter 16 is gone, chapters 17 and 18 become 16 and 17, and the tasks heading reads "The model's status and cancel tools".
- Inputs and outputs:
  - The removed names (`tasks.events`, `task_events`, `TaskEvents` and the rest) get no special handling anywhere. `task_events` stops being a reserved built-in name.
  - Engine and Harness public API: `Effect`, `EffectRecord`, `EffectAnswer` and `AnswerRecord` each lose their `TaskEvents` variant.
- States and validation:
  - Every `Event` variant and every event emission stays, and so does `provenance.seq`.
  - The phrase "task events" meaning the lifecycle events `task_started` and friends stays (for example the sentence at `crates/promptforge/src/ids.md` line 7).
  - Unrelated senses of "history" stay: the `chat` agent's message history, conversation history, and the task history rendered to the model in task notices (`guide/src/language/09-the-store.md`, the sentence linking to "Task notices to the model"; judge it, but it describes what the model sees and is expected to stay).
- Errors and recovery:
  - If any step finds a use of the feature outside tests and docs, stop and report instead of working around it.
  - Nothing is pushed, so the commit that was HEAD before the run is the rollback point. Note it before starting.
- Security and privacy behavior:
  - The model-facing wrapping of history reads under the reader's nonce disappears with the feature. Trust statements for the remaining built-ins stay.
  - Guide chapter 04 keeps its nonce clause and drops only the clause about the run's events.
- Acceptance criteria:
  - The final residual searches find no hit outside `vibe/`, `target/` and `node_modules/` except judged unrelated senses.
  - The guide has 17 language chapters, with the links, anchors and exports consistent.
  - `crates/promptforge/public-api.txt` loses exactly ten `TaskEvents` lines.
  - Event emission, log append and `Session::transcript` tests still pass, apart from the tests deleted with the feature.

</product-contract>
<implementation-contract>

## Technical Design

The feature is one vertical cut through four layers that share a single mechanism. The Engine, Lua protocol and Harness runner must change together, the log crate depends on the runner change, and the guide is independent of the code but has generated exports. The public API snapshot loses ten lines, and hand-edited facade pages replace the waived regeneration run.

- Architecture:
  - Three entry points converge on one effect, which the Harness answers by reading the run log. All of it goes. The Harness log stays append-only during a run and is read only by session transcript views and reconnect.
  - Entry points: Lua `tasks.events(task, { last = seq })` in `crates/promptforge-internal/lua/src/__impl_tasks.lua` yields a `task_events` request; the model's built-in tool `task_events { id, last? }` is the fifth of its task tools; the Engine arm in `crates/promptforge-internal/engine/src/execute/scheduler/task_events.rs` issues `Effect::TaskEvents { task, last }`.

  ```mermaid
  flowchart LR
      luaCall["tasks.events"] --> request["Request"]
      request --> effect["Effect"]
      modelCall["task_events"] --> effect
      effect --> performer["LogTaskEvents"]
      performer --> logRead["events_for_task"]
  ```

- Modules and interfaces (everything below is removed):
  - Engine (`promptforge-engine`): `Effect::TaskEvents`, `EffectRecord::TaskEvents`, `EffectAnswer::TaskEvents`, `AnswerRecord::TaskEvents` and their two conversions; `Continuation::TaskEvents`; `TaskEventsReader`; the `task_history` accessor in `execute.rs`; the `task_events` built-in tool and its schema.
  - Lua (`promptforge-lua`): `tasks.events`; `Request::TaskEvents`; `Answer::TaskEvents`; `parse_task_events`; `event_sequence`.
  - Harness runner (`harness-runner`): `TaskEventsPerformer`; the `task_events` field of `Performers`; `LogTaskEvents`.
  - Harness log (`harness-log`): `RunLog::events_for_task`; `RecordFilter::task`; `SELECT_TASK_RECORDS`; the `records_by_task` index.
  - Kept on purpose: `task_id_argument` and `model_task` (cancel and status use them), the `tasks` blocked label for `JoinAny`, the `kind` and `last` log filters, and the helpers `text_of` and `drive_scripted` (they move).
- File and public API changes:
  - Engine source (`crates/promptforge-internal/engine/src`):
    - Delete `execute/scheduler/task_events.rs`; remove `mod task_events;` and the module-doc lines that name it from `execute/scheduler.rs`, keeping the doc list well-formed.
    - Remove the `Continuation::TaskEvents` variant, its import and its two arms in `execute/scheduler/pending.rs` and `execute/scheduler/apply.rs`.
    - `execute/scheduler/dispatch.rs`: remove the module-doc mention, the `Request::TaskEvents` alternative in the `Some("tasks")` blocked-label arm (the label stays for `JoinAny`) and the `Request::TaskEvents` arm.
    - `execute/scheduler/builtins.rs`, `builtins-schemas.rs`, `tool_call.rs`: remove `"task_events"` from the reserved-name lists, the dispatch arm and the schema block, and every comment that names it. Change "five" and "the fifth" to "four". Then delete whatever only the removed arm used (check `BuiltinOutcome::Issued` and the leaf-effect path of the built-in answers). If a test or comment still says the built-ins number five, fix it.
    - `execute/run/effect.rs`: remove the `TaskEvents` variant of the four types and the two conversions.
    - `execute.rs`: remove `task_history` and its doc. `test_support.rs`, `test_support/tokio_driver.rs` and `tokio_driver-performers.rs`: remove the `TaskEvents` arms, docs and imports. Keep returning collected events to callers; remove any history buffer that only existed to answer reads.
    - Prose: `lib.md` and `README.md` (effects become `Chat`, `ToolCall`, `Store`, `Timer`; delete "read of a task's reported history"). `crates/promptforge-internal/types/AGENTS.md`: delete the bullet about read-side history.
  - Engine tests (`execute/tests`):
    - First move `text_of` and `drive_scripted` (with the imports `model_task_context_with`, `NullObserver`, `SlowTool` and `perform_locally` that it needs) out of `task_events.rs` into `serial_driver.rs`. Repoint `run_inputs.rs` (`text_of`) and `model_task_trust.rs` (both).
    - Delete `task_events.rs` and `mod task_events;` in `execute/tests.rs`.
    - Delete these tests and fix the header comment each leaves behind: `the_task_events_read_wraps_a_forging_history_under_the_readers_nonce` in `model_task_trust.rs`, `a_task_events_answer_reports_the_turn_it_was_dispatched_under` in `batch_turn.rs`, `a_task_events_effect_records_its_task_and_last_bound` in `execute/run/tests.rs`.
    - Edit `tool_call_arm.rs` and `model_tasks.rs` (drop `task_events` from the name list and the expected string) and `serial_driver.rs` (the doc, the `task_history` import and the two arms).
    - Rename lookalike helpers, which extract task lifecycle observations and are not the removed read: free function `task_events` in `model_task_acceptance.rs` and `fanout_acceptance.rs` becomes `task_lifecycle`; `Recorder::task_events` in `waits.rs` becomes `observations_of`. Fix every caller (`run_termination.rs`, `model_task_ids_and_scope.rs`, `timeouts.rs` and the acceptance suites).
  - Lua source and tests (`crates/promptforge-internal/lua/src`): delete `tasks_events` (comment and function) and the `events = tasks_events` export in `__impl_tasks.lua`; remove `Request::TaskEvents` and its doc in `protocol/request.rs`; remove `parse_task_events` from `protocol/parse.rs` (import and arm) and `protocol/parse/tasks.rs` (function, and fix the module doc); remove `Answer::TaskEvents`, its `map_err` arm and the `Event` import if nothing else uses it from `protocol/answer.rs`; delete `event_sequence` and its two arms from `protocol/render.rs`. Tests: the `a_task_events_answer_resumes_event_tables...` test and its imports in `protocol/tests/answer.rs`; the two `task_events_*` tests in `protocol/tests/parse_tasks.rs`.
  - Harness runner (`crates/harness-internal/runner`):
    - `src/performers.rs`: remove the trait, the field and the `LogTaskEvents` re-export; the doc that lists "four" runner-supplied performers becomes three.
    - `src/performers-builtin.rs`: delete `LogTaskEvents`, its `Debug` and trait impls and the trait import.
    - `src/prepare.rs`: remove the import and the `task_events: Arc::new(LogTaskEvents::new(...))` construction; drop variables that become unused.
    - `src/effect_loop.rs`: remove the `Effect::TaskEvents` arm. Reword the module doc and the `SharedLog` doc so they no longer give the removed read as a reason, keeping the behavior (events are appended before the step's effects are issued). Before rewording the `SharedLog` mutex comment, check who else reads the log (`Session::transcript` in `harness-sessions`) and state that accurately.
    - Tests: `tests/it/support.rs` (the import, the `TaskEventsPerformer for Unused` impl and the field) and `tests/it/performers.rs` (the import and the two tests `task_events_returns_the_tasks_slice...` and `task_events_of_a_task_that_never_logged...`).
  - Facade pages (hand-edited; scan `lib.md`, `event.md`, `model.md`, `tools.md` and `capabilities.md` in `crates/promptforge/src` for effect lists that still name five kinds):
    - `crates/promptforge/src/effect.md` (about 430 lines; read the whole page first). Its two runnable tours share a greeter prompt that calls `tasks.events`; rewrite both to four effect kinds (model round, tool call, store operation, timer).
    - Setup in both tours: description becomes "Asks a model and a tool at once, and waits."; delete the Lua line `local history = tasks.events(ask)`; the return line becomes `return results[1].result .. ' / ' .. results[2].result`; remove `use promptforge::event::Event;` if it becomes unused.
    - Helper code: `answer(effect: Effect, log: &[Event])` becomes `answer(effect: Effect)` with no `TaskEvents` arm; the tour-1 comment becomes "Answer each effect with the answer of its own kind."; in `drive`, drop the `log` vector and write `Step::Pending { mut effects, .. }`; in tour 2 the `events` vector and `answer(effect, &events)` go the same way. Remove "committing each step's events first" from comments and prose.
    - Asserts: tour 1 expects `"hi there / HI THERE"`. Tour 2 counts issued effects and logs one line per effect; re-derive every count, kind list and assertion from a real run, because the run now issues one fewer effect.
    - Prose: delete "or read a task's history" and "and task history reads" in the opening; "four kinds of outside work" (drop the `[task](crate::ids)` link if only used there); "asks for all four"; delete the `TaskEvents` bullet in the numbered notes; note 3 ("ending in `task_succeeded`, the last event in `Ask`'s history") becomes "Both runs return the same text."; delete the `TaskEvents` row of the diagram and change "any of the five" to "any of the four"; delete the clause about history reads growing the log in the `AnswerRecord` note; remove every `TaskEvents` row or bullet in the reference tables at the end.
    - `crates/harness/src/lib.md`: delete "and a read of a task's history" so it reads "An agent that starts background tasks can also ask for a timer."
    - `crates/harness/src/vfs.md`: one link points at `.../language/17-limits-and-errors.html#...`; it must follow the renumbering to `16-limits-and-errors.html`.
    - `tools/cicerone/plans/promptforge.md`: the canned-example instruction names "a timer, and a task history read"; remove the history read.
    - Leave `crates/promptforge/src/ids.md` alone (its "task events" means lifecycle events).
  - Public API: regenerate `crates/promptforge/public-api.txt` with the xtask `api --bless` command on the pinned nightly (`nightly-2026-09-05`, defined in `crates/build-xtask/src/api/toolchain.rs`). The diff must remove only the ten `TaskEvents` lines: two on `AnswerRecord`, three on `Effect`, two on `EffectAnswer`, three on `EffectRecord`.
  - Harness log (`crates/harness-internal/log`):
    - Remove `RunLog::events_for_task` from `src/read.rs`, `RecordFilter::task` and its docs from `src/record.rs`, `SELECT_TASK_RECORDS`, the `records_by_task` index and the doc lines about slicing a run by task from `src/schema.rs`, and the `match filter.task` branch in `RunLog::records` (keep the plain query). The `kind` and `last` filters stay.
    - `README.md`: line 3 no longer says the run is indexed by task, sliced by task, or read by a `TaskEvents` performer. It keeps: append-only record of every run's effects, answers and events; session transcript views and reconnect read it.
    - Tests: in `tests/it/read.rs` fix the header, delete `events_for_task_returns_one_task_in_task_seq_order_when_tasks_interleave`, `events_for_task_last_n_keeps_the_final_n_by_task_seq` and `events_for_task_is_empty_for_a_task_that_never_logged`, and in `the_event_readers_refuse_an_unknown_run` delete the `events_for_task` assertion and keep the transcript one. Remove helpers that become unused (`task_seqs`, `expected`, possibly `interleaved_run`, `record`) after checking each is still referenced. Remove `task: None` from every `RecordFilter { ... }` literal (`tests/it/append.rs`, `tests/it/fidelity.rs`; `rg -n "task:" crates/harness-internal` also finds literals in runner and models tests) and delete any test that slices by task.
  - Guide (`guide/`):
    - Delete `guide/src/language/16-task-events.md`; rename `17-limits-and-errors.md` to `16-limits-and-errors.md` and `18-quick-reference.md` to `17-quick-reference.md` with `git mv`.
    - Rewrite the two names in every link under `guide/src` (about 100 occurrences) with a short Python script, and in `crates/harness/src/vfs.md`. No checked-in chapter list exists: `crates/build-user-guide/src/main.rs` scans `guide/src/language/` and generates the book's `SUMMARY.md`, and `guide/books/language/book.toml` lists no chapters. `guide/landing`, `guide/books`, `guide/chrome` and `guide/CONTRIBUTING.md` hold no chapter names.
    - In `15-tasks.md` rename the heading to `## The model's status and cancel tools`, delete the `### History reads` subsection, and replace the anchor `#the-models-status-cancel-and-history-tools` with `#the-models-status-and-cancel-tools` in every link (quick-reference rows and any others; find with `rg`).
    - Per-site edits (locate with `rg` for the removed names; the rule: delete every link into the old chapter 16 and every clause or sentence that exists to describe reading or naming events, keep statements of language behavior that stand on their own):
      - 01: delete the bullet about reading a task's history.
      - 04: delete the clause "The same holds for the run's events (...)" and keep the nonce clause.
      - 05: delete the link to tool call events but keep its sentence; delete the sentence pointing to the events chapter. The `log` section of chapter 05 stays.
      - 10: delete the trailing event sentence and link.
      - 11: delete the paragraph that names events together with the event listing right after it, as a unit; delete the event clauses and links at three other sites, keeping the error behavior.
      - 12: delete the sentence about events; the built-in names become four (`task`, `task_cancel`, `task_status`, `await_tasks`).
      - 15 (about 17 sites): delete the `tasks.events` table row and sentences; four built-ins in the fixed order above, with the `task_events` sentences gone; the `tasks` blocked label reads "a wait on tasks, timed or not"; drop the `task_started` link where it only served the removed read; drop "history read" wording; delete the two sentences saying the Engine keeps each notice in the owner section's history; remove `task_events` from the sentence and the example refusals; remove the `task_events` result, trust and fault sentences and keep the other built-ins' trust statements.
      - 16 (old 17): delete the `tasks.events` bullet and the clause about `model_turn_failed` events.
      - 17 (old 18): delete the `tasks.events` rows (two) and the `task_events` rows (three); edit the built-ins row (four); edit the `task_not_owned` row to "A wait, status read, or cancel names a task...".
      - `guide/src/introduction.md`: delete "and task events" from the list of topics.
    - After the edits, scan for leftovers and judge each hit: `rg -n -e "provenance" -e "model_turn" -e "_started" -e "_succeeded" -e "_failed" -e "_finished" -e "assistant_reply" -e "tool_result" -e "task_notice" guide/src/language`. Keep hits that are not about events (for example error kind names).
- Data, persistence, failure, security, and privacy constraints:
  - Log schema: the DDL in `crates/harness-internal/log/src/schema.rs` is applied on every open with `IF NOT EXISTS` and no schema version, fingerprint or migration (`crates/harness-internal/log/src/append.rs`). Logs persist on disk, so existing databases keep an unused `records_by_task` index; that is harmless and needs no migration. The `task_id` and `task_seq` columns and the `Record` fields stay as the stored provenance of each record.
  - File ceiling: `crates/promptforge-internal/engine/src/execute/tests/serial_driver.rs` is 423 lines today. About 20 lines of helpers move in and a few lines of its own come out, so it should stay under 500; recount after the move.
  - Public URLs: renumbering moves the published pages for old chapters 17 and 18. Old links break and no redirects are planned.
  - Error messages that are reworded stay written for a model reader.
  - Removal rules: delete, never stub. No alias, deprecation note, comment, retired-name guard or retired-symbol seed for the removed names. Delete any test, helper or import that only the removed code used. Each commit compiles, passes clippy with `-D warnings`, and is green for the crates it touches.
  - Existing dated records under `vibe/` are never edited.

</implementation-contract>
<verification-contract>

## Testing Plan

Verification is light and per step. Each step runs the touched crates' tests, formatter check and lint, plus the few commands that guard what that step changes; the last step runs the workspace suite once. Tests that only exercised the feature are deleted with it, and every other test stays green. Residual searches prove no trace is left outside `vibe/`.

- Unit:
  - Engine, Lua, runner, log and sessions tests pass unchanged apart from the deleted feature tests, the edited name lists and the renamed helpers. No new test is written: a deletion has no behavior to pin, and a retired-name guard is excluded by decision.
  - For each touched crate: `cargo nextest run --locked -p CRATE --all-features`, `cargo fmt --all --check` (run `cargo fmt --all` first, because it rewrites files) and `cargo clippy --locked -p CRATE --all-targets --all-features -- -D warnings`.
- Integration and end-to-end:
  - Doctests for the touched doc-bearing crates (`promptforge`, `promptforge-engine`, `harness`): `cargo test --locked --doc -p CRATE`. They are the likeliest break because of `effect.md`.
  - Docs build for the facade and the Engine: `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked --no-deps -p promptforge -p promptforge-engine; $env:RUSTDOCFLAGS=""`.
  - Guide: `cargo run --locked -q -p build-user-guide` (no check mode; it rewrites all three exports), then `git diff --stat -- guide` must list only the language chapters and `guide/promptforge-language-guide.md` with the gateway and workshop exports unchanged, then `cargo xtask site --books-only`. That site build includes a link check, but it covers guide HTML only, not facade pages.
  - Docs-claims guard: `node --test test/docs-claims.mjs` run from `crates/workshop/ui` (the file `crates/workshop/ui/test/docs-claims.mjs`). It reads every `AGENTS.md` in the repository, including the edited `crates/promptforge-internal/types/AGENTS.md`, so run it in the step that edits that file.
- Regression, security, and performance:
  - Public API: after `cargo +nightly-2026-09-05 xtask api --bless`, `git diff crates/promptforge/public-api.txt` removes only the ten `TaskEvents` lines, then `cargo +nightly-2026-09-05 xtask api --check` passes.
  - Structural rules: `cargo test --locked -p build-xtask` (500-line ceiling, boundaries) in the step that moves helpers into `serial_driver.rs`.
  - Facade scan: `rg -n -i -e "five kinds" -e "five effects" -e "any of the five" -e "history read" -e "task history" crates/promptforge/src crates/harness/src` returns nothing relevant. There is no separate proofreading pass: the step review reads `effect.md`, and the doctests and the docs build catch broken code and links.
  - Trust and nonce coverage for the remaining built-ins stays; only the history-read tests go.
  - Residual searches, run from the repository root, expected empty (judge any remaining hit; unrelated senses of "history" stay):
    - `rg -n -i -e TaskEvents -e task_events -e "tasks\.events" -e task_history -e LogTaskEvents -e events_for_task -e event_sequence -e readable_task -e 16-task-events -e "history read" -e "reported history" -e "task's history" . -g "!vibe/**" -g "!target/**" -g "!node_modules/**"`
    - Case-sensitive `rg -n "Task Events" . -g "!vibe/**" -g "!target/**" -g "!node_modules/**"`; the capital-E title appears nowhere else.
    - Renumbering stragglers: `rg -n -e 17-limits-and-errors -e 18-quick-reference -e the-models-status-cancel-and-history-tools . -g "!vibe/**" -g "!target/**" -g "!node_modules/**"`. This catches the `crates/harness/src/vfs.md` link and any export drift.
    - Leftover counts: `rg -n -i -e "\bfive\b" -e "\bfifth\b" crates/promptforge-internal crates/harness-internal crates/promptforge crates/harness guide/src | rg -i "task|built-?in|effect|performer"`, and `rg -c "task_events" guide/promptforge-language-guide.md` must report 0.
  - No performance work; the change only removes code paths.
- Exit criteria:
  - The first two steps pass their touched-crate tests, formatter check and lint, plus the commands their Tests lines name. The last step passes the workspace suite once: formatter check, workspace clippy, workspace nextest (Workshop crates excluded) and the facade and Engine docs build, plus the guide site build and the residual searches.
  - Workspace doctests, the Workshop clippy and nextest, the UI suites and the headless gateway check are not rerun. The doctests of the three doc-bearing crates run in the first step, and no Workshop or gateway file names a removed item; the only code outside the Harness crates that touches `RecordFilter` calls `RecordFilter::default()`.
  - The residual searches above return nothing relevant.
  - `git status` is clean, `git log --oneline` shows three work commits on top of the commit that was HEAD before the run (plus the run tool's closing commit), and nothing is pushed.

</verification-contract>
<decision-record>

## Decision Record

The owner chose a complete cut with no leftovers, and fixed the guide shape and a three-step, three-commit shape. The remaining calls settle how light the verification is, which stray links must follow the chapter renumbering, and where the docs-claims guard runs.

- Decisions:
  - Complete cut of the task history read, with no stub, alias, deprecation note, comment or retired-name guard. Rationale: nothing uses the feature besides its own tests and docs, and it forces the log to be readable during a run. The owner's words: "complete cut"; if the feature returns it will be redesigned.
  - Events keep their `provenance.seq` and everything else. Only the read goes.
  - The log crate's per-task read support goes too, because only the removed feature used it. The `task_id` and `task_seq` columns stay as the stored provenance of each record.
  - Test helpers that merely share the name `task_events` are renamed so a residual search is clean.
  - The guide drops chapter 16 and renumbers 17 and 18 to 16 and 17. The heading "The model's status, cancel, and history tools" becomes "The model's status and cancel tools". The guide teaches authors who can no longer read events, so every link into old chapter 16 and every clause that exists to describe reading or naming events goes; statements of language behavior that stand on their own stay.
  - The work is exactly three steps and three work commits, one per step, with these subjects: "Remove the task history read" (Engine, Lua protocol, Harness runner, their tests, facade pages, `public-api.txt`), "Remove the log's per-task read path" (`harness-log`), "Remove the Task Events chapter" (guide, its exports, and every link to the renumbered chapters including `crates/harness/src/vfs.md`). The owner chose these three large steps over small steps when asked, because the plan had already fixed three commits ("History ends as exactly three new commits and no fourth").
  - The run tool adds its own closing commit that deletes `vibe/ACTIVE`; that commit and the dated plan copy under `vibe/` belong to the tool and are not extra work. Step 1 is large because the Engine, Lua protocol and Harness runner compile only together, so the step text gives an inside-out order that keeps the build green while working.
  - Components in dependency order, one step each, built one after another. The code removal comes first because everything else depends on the effect and `LogTaskEvents` being gone. The log read path comes second because `events_for_task` still has callers until `LogTaskEvents` is gone. The guide comes last: it is independent of the code, but its exports are generated once and its checks are the final gate.
  - Facade pages are hand-edited and no Cicerone run happens. Rationale: a run takes hours, and the owner waived it for this change.
  - There is no separate facade proofreading subagent. Rationale: the owner asked for light verification, the step review reads `effect.md`, and the doctests and the `-D warnings` docs build catch broken code and links.
  - Verification is light, at the owner's request ("don't go overboard on verify"): per-crate tests, formatter check and clippy at the end of the first two steps, and one workspace pass in the last step. Workshop, UI, headless gateway and workspace doctest gates are left out. Rationale: no Workshop or gateway file names a removed item, and the only code outside the Harness crates that touches `RecordFilter` calls `RecordFilter::default()`. Revisit if the compiler or CI shows a break there.
  - `crates/harness/src/vfs.md` is edited with the guide work. Rationale: it links to `17-limits-and-errors.html`, and the books-only site check does not cover facade pages.
  - The docs-claims guard runs as `node --test test/docs-claims.mjs` from `crates/workshop/ui`, in the step that edits `types/AGENTS.md`. Rationale: it reads every `AGENTS.md` in the repository, and the guide step edits none.
  - The pinned nightly is written as `nightly-2026-09-05` in commands; if `crates/build-xtask/src/api/toolchain.rs` pins a different one at execution time, use that.
- Rejected alternatives:
  - Leaving a stub, alias, deprecation note or retired-name guard: the owner wants nothing left behind. Revisit if the feature is redesigned.
  - Keeping the log's per-task read for a future host-owned log: only the removed feature used it. Revisit when that design needs per-task slicing.
  - Running the Cicerone update for the facade pages: it takes hours. Revisit at the next regular Cicerone run, which would also settle any house-style drift from the hand edits.
  - Keeping a gap or stub at chapter 16: the owner chose renumbering. Revisit if the feature returns.
  - Many small steps with one commit each: the owner chose three large steps. Revisit if Step 1 proves too large to review or fix.
  - A separate proofreading subagent and the repository's full gate list (Workshop clippy and nextest, UI suites, headless gateway check, workspace doctests and docs): the owner asked for light verification. Revisit if a break shows up in a crate the light run skips.
  - A fourth work commit for follow-ups: the owner said no.
- Assumptions, risks, and notes:
  - Assumption: at planning time the tree was clean and in sync with `origin/master`. The run tool stops on a dirty worktree, so commit or stash first.
  - Note: the inventory at planning time was 39 non-Markdown files (23 in the Engine, 8 in Lua, 6 in the runner, 2 in the log) and 18 Markdown files outside `vibe/` and the generated guide export. Nothing in Workshop, `harness-sessions`, `harness-models`, `gateway`, examples or scripts mentions the feature.
  - Note: source line numbers are left out on purpose, because they shift as edits land. The compiler and `rg` are the pointers.
  - Risk: `effect.md` doctests are the likeliest break; rerun after each edit.
  - Risk: Step 1 is one atomic compile unit across five crates, so the coder may need several review and fix rounds. Work inside-out in the order the step lists.
  - Risk: a hand-edited facade page can miss a stale sentence or drift from the Cicerone house style. The scan, the doctests and the `-D warnings` docs build cover the checkable part; style drift is accepted.
  - Risk: removing an `Effect` variant breaks any exhaustive match. None was found outside the four crates, and the compiler will say.
  - Risk: facade pages are checked only by rustdoc, which verifies intra-doc links but not external URLs, so a stale chapter URL there is silent. The renumbering residual search covers it.
  - Risk: the 500-line ceiling on `serial_driver.rs` (423 lines today).
  - Risk: published chapter URLs for old 17 and 18 change, and old links break.
  - Risk: existing on-disk run logs keep an unused `records_by_task` index; harmless.
  - Risk: if any step turns up a use of the feature outside tests and docs, stop and report.
  - Note: the link count (about 100) and the `effect.md` size (about 430 lines) come from planning-time reads and may be off by a few.
  - Confidence: high that the cut is safe and complete as scoped, because every mention was inventoried with `rg` and re-checked by independent reads of the repository. Medium on the tour-2 numbers in `effect.md` until they are re-derived from a real run.

### Deferred and Out of Scope

- Deferred: a host-owned log and dropping tokio from the Harness. This removal unblocks both; revisit when that work is planned.
- Deferred: any redesigned way to read task history. Revisit only if a concrete use appears.
- Deferred: regenerating the facade pages with Cicerone. Revisit at the next regular run.
- Out of scope: `c:\Users\Vinnie\cursor\promptforge2`, `promptforge3` and `promptforge-design`.
- Out of scope: anything under `vibe/`.
- Out of scope: the rule text in `AGENTS.md`.
- Out of scope: structural checks or retired-symbol seeds for the removed names.
- Out of scope: pushing.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: None for this plan. Clippy and nextest compile every target this change touches, so no separate build runs. The repository's own build, `cargo build --locked -p gateway`, is unrelated to this change.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features` for each touched crate, optionally narrowed with a test-name substring; nextest does not run doctests, so for a touched `promptforge`, `promptforge-engine` or `harness` also run `cargo test --locked --doc -p <crate>`; a single Node test file runs as `node --test <path>` from `crates/workshop/ui`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for each touched crate, then `cargo test --locked --doc -p <crate>` when the crate is `promptforge`, `promptforge-engine` or `harness`. The Workshop trio and the UI suites are not part of this plan's run.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`. Workspace doctests, the Workshop trio and the UI `npm test` suites are left out of this plan's run: no Workshop or gateway file names a removed item, the only code outside the Harness crates that touches `RecordFilter` calls `RecordFilter::default()`, and the first step runs the doctests of the three doc-bearing crates it touches. The repository's complete gate list is in `AGENTS.md` (Verification section) and `.github/workflows/ci.yml`.
- Linter command: per touched crate `cargo clippy --locked -p <crate> --all-targets --all-features -- -D warnings`; for the full scope `cargo clippy --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`. The Workshop clippy, the headless gate `cargo check -p gateway --no-default-features` and the UI typecheck are left out for the same reason. Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (the pre-commit hook runs it). Run `cargo fmt --all` first, because it rewrites files.
- Docs command: on PowerShell `$env:RUSTDOCFLAGS = "-D warnings"; cargo doc --locked --no-deps -p promptforge -p promptforge-engine -p harness; $env:RUSTDOCFLAGS = ""`. Other docs gates stay out of the generic run and appear in a step's Tests line when that step needs them: the user guide `cargo xtask site --books-only`, and the facade surface `cargo +<pinned nightly> xtask api --check`, where the nightly is named in `crates/build-xtask/src/api/toolchain.rs`.
- Test placement and naming conventions:
  - Rust crates use stable `rust-toolchain.toml` (edition 2024); `clippy.toml` allows `unwrap` and `expect` in tests only.
  - Unit tests live inside the crate's `src/`: the Engine keeps them in `crates/promptforge-internal/engine/src/execute/tests/` (snake_case files by topic, such as `model_tasks.rs`, with kebab-case siblings like `tasks-chain-end.rs` for a group under three files); the Lua crate keeps `*-tests.rs` siblings beside the module they test (`coro-tests.rs`, `dispatch-tests.rs`).
  - Integration tests use one binary per crate with a `main.rs` and modules beside it: `tests/suite/` for the `promptforge` and `harness` facades, `tests/it/` for the `harness-internal` crates and the gateway (`--test it`); shared fixtures sit in `support.rs`; Engine prompt fixtures are Markdown files under `crates/promptforge-internal/engine/tests/prompts/{execution,invalid}/` named in kebab-case.
  - Test function names are descriptive sentences in snake_case, for example `a_direct_launch_recovers_the_lease_from_a_terminated_owner`.
  - UI tests are Node test files: `crates/workshop/ui/test/*.mjs` and `crates/workshop/ui/src/**/*.test.mjs`, including the `docs-claims.mjs` guard that enforces the Engine, Harness and Host vocabulary in every `AGENTS.md`, `## Invariants` crate doc and `.cursor/rules` file; repository tools keep theirs in `tools/*.test.mjs`.
  - Behavior changes ship with tests in the same change; per `AGENTS.md`, structural checks are added only with explicit user approval.
- Directory map:
  - `crates/promptforge/` is the Engine's public facade, with `public-api.txt` and facade pages as `src/*.md`; `crates/promptforge-internal/` holds `engine`, `types`, `parser`, `lua`, `vfs` and `model-client`.
  - `crates/harness/` is the Harness facade; `crates/harness-internal/` holds `runner`, `models`, `plugins`, `log`, `sessions`, `web`, `webfetch` and `web-search`.
  - `crates/gateway/` is a private container (`app`, `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and `stt/` with its own four crates), beside the public root crates `gateway-api-types` and `gateway-api-discovery`.
  - `crates/workshop/` holds the desktop app, the in-process `server`, `server-api`, the subsystem crates (`gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`) and the TypeScript packages `ui`, `look` and `platform` under one npm workspace.
  - `crates/shared-*` are the cross-product crates (`shared-error-source`, `shared-loopback`), `crates/shared-ui` is the TypeScript and CSS package for the Gateway config UI, `crates/build-*` are meta tooling (`build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`), and `crates/workspace-hack` is the cargo-hakari unification crate.
  - `guide/` holds the three books (`src/language/`, `src/gateway/`, `src/workshop/`) plus `CONTRIBUTING.md`; `tools/` holds `cicerone.md` with its scripts and plans and the Node sidecar scripts; `vibe/` holds `archdoc.md` and dated plan records; `.github/workflows/` holds CI and release workflows; `.githooks/` holds `pre-commit` and `pre-push`; `.cargo/config.toml` defines the `xtask` and `workshop` aliases; `.config/` holds `nextest.toml` and `hakari.toml`; `cabinet/`, `local/`, `prompts/`, `images/`, `target/` and `target-msrv/` hold working material and build output.
- Component boundaries:
  - Dependencies flow Host to Harness to Engine: the Harness depends only on `promptforge` among product crates and is reached by outside crates only through `harness`; the Engine parses and steps and never performs I/O; Workshop crates may name only the gateway public pair, the `promptforge` API and `harness`.
  - Each family has one public root crate and a manifestless private container (`promptforge-internal`, `harness-internal`, `gateway`, `workshop`); a container crate may depend only on crates at the `crates/` root and its own siblings, and `build-*` crates are exempt.
  - Gateway crates never depend on `promptforge`, Harness or Workshop crates; `promptforge-*` crates never depend on gateway, Workshop or Harness crates; shared crates depend on no product crate; the desktop app depends on `workshop-server-api`, never on `workshop-server`.
  - Workshop tiers run one way: server, features, services, vocabulary; the SPA's lazy panels never import the entry bundle.
- Conventions summary:
  - Workspace lints forbid `unsafe_code`, deny clippy `all` and `pedantic`, and deny `unwrap_used` and `expect_used`; `missing_docs` and `unreachable_pub` warn; rustdoc broken links are denied.
  - Files in crates with an `## Invariants` marker stay at or under 500 lines; source directories are flat, and a subdirectory needs at least three files or becomes kebab-case siblings wired with `#[path]`.
  - Engine, Harness and Host are capitalized terms with one meaning each; prose uses plain English with no em dashes and no double dashes outside code spans; guide code fences open with four backticks and each guide paragraph is one line.
  - Error and status messages are written for a model reader; JSON that reaches the run log round-trips exactly with sorted keys and no `preserve_order`.
  - Comments explain non-obvious constraints and every workaround cites an upstream issue URL; Cargo features gate real constraints only; dependency pins have an explaining comment in the root `Cargo.toml`.
  - CI (`.github/workflows/ci.yml`) gates on fmt, clippy, test, docs, Workshop on Windows and Linux, UI, supply chain and the API surface, and fails when a build dirties the tree; `pre-push` runs the headless gateway check, clippy and `cargo deny check`.
  - Environment (Windows, PowerShell 5.1): run cargo commands one at a time, because they share a target-directory lock. Do not append `2>&1` to cargo commands, because PowerShell turns cargo's stderr into error records; judge a cargo step by its result lines and `$LASTEXITCODE`. Use `rg` in the shell for exhaustive searches; the Grep tool truncates long lines and ignores paths outside the workspace. PowerShell redirection writes UTF-16, so write files that git or tools read with the Write tool (UTF-8, no byte-order mark). Set `$env:PYTHONIOENCODING="utf-8"` before running Python, and read and write Python files with `encoding="utf-8"` and `newline=""`. git's "CRLF will be replaced by LF" warnings are benign (`eol=lf`). Never put an edit and the check that observes it in one parallel batch.
  - This plan removes a feature: delete, never stub, and add no retired-name guard or structural check. The run tool adds this plan's dated copy and `vibe/ACTIVE` under `vibe/`; existing dated records there are never edited.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Remove the task history read [completed]

- Component: task-history-code
- Artifacts: the Engine (`crates/promptforge-internal/engine`), the Lua protocol (`crates/promptforge-internal/lua`), the Harness runner (`crates/harness-internal/runner`), the types prose (`crates/promptforge-internal/types/AGENTS.md`), the facade pages `crates/promptforge/src/effect.md` and `crates/harness/src/lib.md`, `tools/cicerone/plans/promptforge.md`, and `crates/promptforge/public-api.txt`. Every file, symbol and rewrite is listed under Technical Design, "File and public API changes" (Engine source, Engine tests, Lua source and tests, Harness runner, Facade pages, Public API).
- Work, inside-out so the build stays green while you go (the Engine, Lua and runner changes compile only together once the effect variant goes):
  - First rewrite both tours in `effect.md` so the greeter prompt no longer calls `tasks.events`, and re-derive the tour-2 numbers from a real doctest run. This works while the feature still exists.
  - Then remove the two entry points: `tasks_events` and its export in `__impl_tasks.lua`, and the `task_events` built-in in `builtins.rs`, `builtins-schemas.rs` and `tool_call.rs`. Move `text_of` and `drive_scripted` into `serial_driver.rs` and repoint `run_inputs.rs` and `model_task_trust.rs` before deleting `execute/tests/task_events.rs`. Delete the listed tests and the two runner tests in `tests/it/performers.rs` that run `tasks.events` prompts, and edit the name lists.
  - Then remove the scheduler path and the Lua protocol types: `scheduler/task_events.rs`, `Continuation::TaskEvents`, the `dispatch.rs` and `apply.rs` arms, `Request::TaskEvents`, `parse_task_events`, `Answer::TaskEvents`, `event_sequence`, and their tests.
  - Then remove the effect and its performer together: the four `TaskEvents` variants and two conversions in `execute/run/effect.rs`, `task_history`, the `test_support` arms, `TaskEventsPerformer`, the `Performers` field, `LogTaskEvents`, the `prepare.rs` construction, the `effect_loop.rs` arm and the runner test support. Finish `effect.md` (the `TaskEvents` arm, tables and prose), then bless `public-api.txt`.
  - Last, rename the lookalike test helpers and edit the remaining prose (Engine `lib.md` and `README.md`, `types/AGENTS.md`, `crates/harness/src/lib.md`, the Cicerone plan line).
- Tests: no new test, because a deletion has no behavior to pin and a retired-name guard is excluded by decision; the existing suites are the check. Besides the touched-crate tests, formatter check and lint, run these as written:
  - `cargo test --locked --doc -p promptforge -p promptforge-engine -p harness --all-features`
  - `$env:RUSTDOCFLAGS="-D warnings"; cargo doc --locked --no-deps -p promptforge -p promptforge-engine; $env:RUSTDOCFLAGS=""`
  - `cargo +nightly-2026-09-05 xtask api --bless`, then `git diff crates/promptforge/public-api.txt` must remove only the ten `TaskEvents` lines, then `cargo +nightly-2026-09-05 xtask api --check`
  - `cargo test --locked -p build-xtask`
  - `node --test test/docs-claims.mjs` from `crates/workshop/ui`
  - `rg -n -i -e TaskEvents -e task_events -e "tasks\.events" -e task_history -e LogTaskEvents -e event_sequence crates/promptforge crates/promptforge-internal crates/harness crates/harness-internal/runner tools` finds nothing
  - the facade scan `rg -n -i -e "five kinds" -e "five effects" -e "any of the five" -e "history read" -e "task history" crates/promptforge/src crates/harness/src` finds nothing relevant

</step-1>
<step-2>

### Step 2: Remove the log's per-task read path [completed]

- Component: run-log-read-path
- Artifacts: `crates/harness-internal/log` (package `harness-log`): `src/read.rs`, `src/record.rs`, `src/schema.rs`, `README.md`, and the tests `tests/it/read.rs`, `tests/it/append.rs`, `tests/it/fidelity.rs`; plus every `task: None` literal in a `RecordFilter { ... }` in the runner and models tests, found with `rg -n "task:" crates/harness-internal`. All edits are listed under Technical Design, "File and public API changes" (Harness log).
- Work: remove `RunLog::events_for_task`, `RecordFilter::task` with its docs, `SELECT_TASK_RECORDS`, the `records_by_task` index and the by-task doc lines, and the `match filter.task` branch in `RunLog::records` (keep the plain query; the `kind` and `last` filters stay). Fix the `README.md` line. Fix the tests: the header and three `events_for_task` tests in `tests/it/read.rs`, the `events_for_task` assertion in `the_event_readers_refuse_an_unknown_run`, helpers that become unused, and the `task: None` literals. Keep the `task_id` and `task_seq` columns and `Record` fields.
- Tests: no new test (a deletion, and a retired-name guard is excluded by decision). Run `cargo nextest run --locked -p harness-log -p harness-runner -p harness-models -p harness-sessions --all-features` as written, then confirm `rg -n -e events_for_task -e records_by_task -e SELECT_TASK_RECORDS crates` finds nothing.

</step-2>
<step-3>

### Step 3: Remove the Task Events chapter [completed]

- Component: guide-chapter
- Artifacts: `guide/src/language/16-task-events.md` (deleted), `17-limits-and-errors.md` and `18-quick-reference.md` (renamed to `16-limits-and-errors.md` and `17-quick-reference.md`), the chapters `01`, `04`, `05`, `10`, `11`, `12` and `15-tasks.md`, `guide/src/introduction.md`, `crates/harness/src/vfs.md`, and the regenerated `guide/promptforge-language-guide.md`. All edits are listed under Technical Design, "File and public API changes" (Guide).
- Work, in this order because later edits depend on the renames: `git rm` chapter 16 and `git mv` 17 and 18; rewrite the two chapter names in every link under `guide/src` and in `crates/harness/src/vfs.md` with a short Python script; rename the `15-tasks.md` heading, delete its `### History reads` subsection and rewrite the old anchor in every link; apply the per-site edits chapter by chapter; run the leftover scan from Technical Design and judge each hit; regenerate the exports once at the end.
- Tests: no new test (prose only). Run these as written, one at a time:
  - `cargo run --locked -q -p build-user-guide`, then `git diff --stat -- guide` lists only the language chapters and `guide/promptforge-language-guide.md`, with the gateway and workshop exports unchanged
  - `cargo xtask site --books-only`
  - the residual searches from the Testing Plan: the removed-name search, the case-sensitive `Task Events` search, the renumbering-straggler search, the "five" and "fifth" search, and `rg -c "task_events" guide/promptforge-language-guide.md` reporting 0
  - The last step also gets the full-scope run from the project survey; judge any residual hit and keep the unrelated senses of "history".

</step-3>

</execution-plan>
