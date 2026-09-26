---
name: Batch turn debt removal
overview: "Remove three local-tool handler debts. DEBT-LTC-C1: later tool results in a model batch report a shifted turn after a handler runs model rounds; fixed by carrying the requesting round's turn with every model-issued call. DEBT-LTC-X1: a handler can jump through a saved reference; fixed by making the real jump function refuse while a handler runs. DEBT-LTC-X2: the event contract promises a ToolResult that failed local calls never emit; fixed by correcting the contract doc."
todos: []
isProject: false
---

# Remove the local tool handler debts

All paths are relative to the `promptforge` repository root.

<product-contract>

## Product Requirements

- Scope and target work:
  - The target is `0f43e1b3..c7742237`: commits `c71828c2` (run local tool handlers inside the block coroutine), `efdea974` (guide paragraph), and `c7742237` (plan close).
  - The baseline `0f43e1b3` is the merge base of `origin/master` and `master`.
  - The disposition state is the clean worktree at `c7742237`.
  - Design records read: `vibe/archdoc.md` and `vibe/2026-09-26-2-local-tools-coroutine.md`. There is no `vibe/archdoc-next.md`.
  - The operator expanded the scope to include the two exposed pre-existing debts, DEBT-LTC-X1 and DEBT-LTC-X2. They were not added by the target, but they sit in the mechanism it rebuilt.
  - Analysis limits:
    - No repository code was executed; every consequence is traced from reading code.
    - The 1,662-entry commit log was searched by keyword rather than read end to end.
    - Of the hosts in this repository, only the workshop host was checked.
- Cleanup goals:
  - Remove DEBT-LTC-C1, the one debt the target introduced.
  - Remove DEBT-LTC-X1 and DEBT-LTC-X2.
- Non-goals:
  - Every rejected candidate.
  - Any change to the facade API. Only the wording of `crates/promptforge/src/event.md` changes.
- Success criteria:
  - DEBT-LTC-C1: take a model batch where an earlier local handler runs model rounds. The `Event::ToolResult` of every later model-issued call in that batch, whether bound, local, or a task built-in, carries the same turn as the batch's `Event::AssistantToolCalls`.
  - DEBT-LTC-X1:
    - Inside a handler, `jump` raises a clear refusal error and records no jump. That holds whether the handler calls the global or a reference saved before it ran, and whether or not the caller catches the error.
    - Outside any handler, `jump` works exactly as before.
  - DEBT-LTC-X2: the event contract describes what a failed model-issued local call reports, and a test pins that behavior.
  - All existing tests pass, apart from the three `jump` tests this plan deliberately updates.

## Functional Specification

### Debt Inventory

- DEBT-LTC-C1 (accepted, introduced by `c71828c2`, present at `c7742237`):
  - Mechanism, in order:
    - A chat round advances the chain's turn counter to N and reports the whole batch as `AssistantToolCalls` under N.
    - `models_loop` then dispatches the batch one call at a time (`crates/promptforge-internal/lua/src/__impl_coro.lua` around lines 264-268).
    - Each dispatch reads the live counter into its `ScriptReport` (`crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 139-141). The task built-ins read the live counter again when they answer (`crates/promptforge-internal/engine/src/execute/scheduler/builtins.rs` lines 245-247).
    - A local handler that calls `models.infer`, runs its own `models.loop`, or uses `call` into a section that runs model rounds advances that same counter in the middle of the batch. A `call` child shares its caller's counter, while task chains get their own (`crates/promptforge-internal/engine/src/execute/context.rs` around lines 329-345).
    - So every call dispatched after that handler reports N+k. The handler's own result stays correct at N, because it is recorded on the frame's `LocalCall`.
  - Why it counts as introduced: at baseline a handler could not suspend, so nothing could advance the counter between two dispatches of one batch.
  - Contract contradicted:
    - `crates/promptforge/src/event.md` line 204 and line 380 say each dispatched call is followed by a `ToolResult` "with the same turn and the call's id".
    - Line 381 says `AssistantToolCalls::turn` is "the model-turn counter of the round that requested the batch".
    - Line 399 says to scope ids by turn, because providers recycle ids.
    - The target plan's own rationale for the frame stack was to keep the reported turn correct when a handler runs model rounds.
  - Impact:
    - A host that pairs results with requests by turn and id, as the event contract instructs, cannot find later results under turn N. It either misses them or attaches them to the wrong request when a nested loop reused an id.
    - The workshop server forwards the wrong turn (`crates/workshop/server/src/agents/wire.rs` lines 268-276). The workshop UI appends result rows in arrival order and never pairs by turn (`crates/workshop/ui/src/services/agent-session.ts` lines 321-327), so it shows a wrong turn stamp but nothing visibly breaks.
    - The model-facing message list is unaffected.
  - Reversal cost: small and internal. The protocol types live in the unpublished `promptforge-lua` crate, they do not appear in `crates/promptforge/public-api.txt`, and nothing about them is persisted.
  - Target state: every model-issued `ToolResult` carries the turn of the round that requested it.
- DEBT-LTC-X1 (exposed pre-existing, in scope at the operator's request): a handler can still jump by saving a reference to `jump` first.
  - Mechanism:
    - `run_local_tool` withholds `jump` only by swapping the global name through `swap_jump` (`crates/promptforge-internal/lua/src/__impl_coro.lua` lines 119-135; `crates/promptforge-internal/lua/src/coro.rs` lines 267 onward).
    - The real `jump` is one Rust host function that resolves the target, writes the VM's `jump_slot`, and raises the transfer marker, with no check for a running handler (`crates/promptforge-internal/lua/src/vm.rs` lines 595-609).
    - `step_block_coro` honors a recorded slot whenever the block finishes, and `start_block_coro` clears it only when a block starts.
  - Consequence:
    - After `local j = jump` at block level, `j('## Other')` inside a handler records a jump. The event log reports it as `TOOL_CALL_FAILED`.
    - If the error goes uncaught, the loop ends and the block jumps. If the caller wraps the call in `pcall`, the block carries on and then jumps anyway when it ends.
    - Either way this contradicts the guide's "it still cannot call `jump`" (`guide/src/language/07-tools.md` line 66).
  - The behavior was identical at baseline, so this is not debt added.
  - Reversal cost: small and internal.
  - Target state: the one real `jump` function refuses while any handler is running on its VM, so a saved reference gets the same refusal as the global.
- DEBT-LTC-X2 (exposed pre-existing, in scope at the operator's request): a failed model-issued local call reports no `ToolResult`.
  - Mechanism: `dispatch_local_tool_done` reports `TOOL_CALL_FAILED` and no `ToolResult` for both `BadReturn` and `Raised` (`tool_call.rs` around lines 231-238).
  - Contract contradicted: `crates/promptforge/src/event.md` lines 204, 380, 396, and 397 promise a `ToolResult` for every model-issued call, whether it succeeds or fails.
  - The behavior matches a deliberate decision recorded in `vibe/2026-09/2026-09-18-4-sans-io-engine-harness.md`: a local handler's failure is the call's error for both kinds of call. So the contract doc is the part that's out of step.
  - The behavior was identical at baseline, so this is not debt added.
  - Reversal cost: a documentation change only.
  - Target state: `event.md` describes the local-tool exception, and a test pins it.
- Rejected candidates: 20.
  - 7 residual-but-acceptable:
    - the new tag-dispatch arms;
    - protocol rendering depending on the handler registry;
    - the two-yield handshake;
    - unbounded handler recursion, which the plan already defers, bounded by the Lua stack and the VM memory cap;
    - handler frames lost from tracebacks, an accepted tradeoff;
    - file growth;
    - "a handler error fails the run" wording.
  - 8 weak or speculative:
    - pairing `LocalCall` entries by stack order;
    - split storage of local tool metadata and handlers;
    - the `install_add_local` parameter cluster;
    - guide-promised calls with no test (the `models.infer` and `call` parts are filed under DEBT-LTC-C1);
    - argument conversion failing only under memory exhaustion;
    - the ignored `Plain("")` answer;
    - `error(nil)` passing through;
    - a nested `models.loop` draining the outer loop's task notices.
  - 5 false:
    - trusted handler output;
    - the handler's own result turn;
    - handlers unable to suspend (repaired by the target);
    - another chain seeing the withheld `jump`;
    - a nested round re-gating the outer batch.
  - 0 unrelated pre-existing.

</product-contract>
<implementation-contract>

## Technical Design

- DEBT-LTC-C1: carry the requesting round's turn with each model-issued call, the same way `call_id` already travels from the chat result through the shim and back to the scheduler. The value stays in the shim's local variables, so nested loops and `call` children need no state to restore.
  - The chat result carries the turn:
    - `ChatResult` in `crates/promptforge-internal/lua/src/protocol/answer.rs` gains `turn: u32`.
    - The chat arm (`crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`, around the `advance_turn` call near line 204 and `assistant_tool_calls` near lines 338-339) sets it to the turn it reported `AssistantToolCalls` under.
    - `chat_result_table` in `crates/promptforge-internal/lua/src/protocol/render.rs` renders it as `turn`.
  - The shim passes it back:
    - In `crates/promptforge-internal/lua/src/__impl_coro.lua`, `models_loop` calls `tools_call_as_model(call.id, call.name, call.arguments, round.turn)`.
    - `tools_call_as_model` adds `turn` to its `tool_call` yield.
  - The request carries it:
    - `Request::ToolCall` in `crates/promptforge-internal/lua/src/protocol/request.rs` gains `turn: Option<u32>`.
    - `parse_tool_call` in `crates/promptforge-internal/lua/src/protocol/parse.rs` reads it as a shim-produced field. Absent or nil becomes `None`. An integer within `u32` range becomes `Some`. Anything else is a malformed yield.
  - The scheduler records it:
    - In `prepare_tool_call` (`tool_call.rs` lines 139-141), the report becomes `ScriptReport { turn: turn.unwrap_or(live counter) }`.
    - That one value already reaches the local path through `LocalCall.report`, and the bound path through `ToolCallContinuation.report` and then `ModelReport` (`crates/promptforge-internal/engine/src/execute/scheduler/apply.rs` around lines 155-158).
  - The task built-ins use it:
    - `report_builtin_answer` (`builtins.rs` lines 227-254) takes the dispatch turn as a parameter and stops reading `chain.ctx.turns()`.
    - `answer_task_builtin` passes it through.
    - The parked `await_tasks` record (`AwaitTasks` in `crates/promptforge-internal/engine/src/execute/scheduler/await_tasks.rs`) and the `task_events` continuation store it next to the call id, so their later answers report it.
  - Contract wording: change `crates/promptforge/src/event.md` line 398 from "the round that dispatched the call" to the round that requested the call. For a script-issued call it is the counter's value when the script dispatched it.
  - What does not change:
    - Script calls (`call_id: None`) keep the live counter, since they have no requesting round.
    - The test-only `tools.call_as_model` hook may leave out `turn` and falls back to the live counter.
    - Failure behavior, trust marking, counting, and the facade API.
- DEBT-LTC-X1: guard the real `jump` function instead of swapping the global name.
  - `SectionVm` gains a `local_handler_depth` counter shared with its closures (an `Arc<AtomicU32>`), next to `jump_slot` in `crates/promptforge-internal/lua/src/vm.rs` (field near line 88).
  - `install_jump_global` (`vm.rs` lines 595-609) checks the counter first. While it is above zero, the function returns a refusal error without resolving the target or writing `jump_slot`. The message is written for a model to read, for example: "jump is unavailable inside a local tool handler: return a value from the handler and call jump from the block after the tool call returns".
  - Two new prelude captures replace the `swap_jump` capture (`crates/promptforge-internal/lua/src/coro.rs` lines 148, 182, 196, 267 onward): `enter_local_handler` and `leave_local_handler`. `SectionVm::install_coro_shims` builds them over the same counter and passes them into `install_shim_prelude`.
  - `run_local_tool` in `__impl_coro.lua` calls `enter_local_handler()`, then `raw_pcall(handler, args)`, then `leave_local_handler()`. It never touches the global `jump`.
  - Why this is safe:
    - `raw_pcall` catches every failure, so the leave always runs.
    - Nested handlers count up and down.
    - Each section frame owns its VM, so the counter is per chain. A chain that ends while parked inside a handler drops its VM along with the counter.
    - A `call` child runs in its own VM, so it can still jump inside its own chain.
  - Update the prelude header comments that describe `swap_jump`.
- DEBT-LTC-X2: correct the contract doc and leave the code alone.
  - In `crates/promptforge/src/event.md`, amend lines 204, 380, 396, and 397 to state the exception for a model-issued call to a Lua-local tool:
    - When its handler raises or returns an unsupported value, the call reports `ToolCallFailed` and no `ToolResult`.
    - The failure propagates to the caller of `models.loop` and ends the loop unless the author catches it.
  - Every other rule in those lines stays as written.
- Guide:
  - Extend the handler sentence in `guide/src/language/07-tools.md` line 66 to say that calling `jump` from a handler raises an error, even through a saved reference.
  - Regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`, which rebuilds it from the chapter files. It is never edited by hand.

</implementation-contract>
<verification-contract>

## Testing Plan

- DEBT-LTC-C1 regression, the main engine test:
  - Round 1 returns two calls: `c1`, a local `grab` whose handler calls `models.infer`, and `c2`, a bound `echo` from `echo_tools()`. The handler's inference gets its own scripted text reply, and round 2 returns final text.
  - Observe the events through a recording observer that captures the turn on `Event::AssistantToolCalls` and `Event::ToolResult`.
  - Assert the batch turn, `c1`'s result turn, and `c2`'s result turn are equal. Before the fix, `c2` reports the next turn.
- DEBT-LTC-C1 variants:
  - The handler runs its own `models.loop` with one inner tool call. The inner result reports the inner round's turn, and `c2` still reports the outer batch's turn.
  - The handler uses `call` into a section that runs `models.infer`. `c2` reports the outer turn.
  - `c2` is a task built-in such as `task_status`. It reports the outer turn.
- DEBT-LTC-C1 unit tests:
  - Parse tests for the `turn` field: absent gives `None`, a valid integer gives `Some`, and a negative number, float, or string is a malformed yield.
  - A render test that the chat result carries `turn`.
  - A shim walk showing that the loop's `tool_call` yield carries the same `turn` as the chat result that requested it.
- DEBT-LTC-X1, new engine tests:
  - A saved reference: the block runs `local j = jump`, then registers a handler that calls `j('## Other')`. Through a script `tools.call` and through `models.loop`, the run fails with the refusal message and does not jump.
  - A caught refusal: the caller wraps `tools.call` in `pcall` around that handler, and the block then returns `'no jump'`. The run returns `'no jump'`, proving that no jump was recorded.
  - Nesting: an outer handler calls an inner local tool. `jump` is still refused in the outer handler after the inner one returns, and works in the block once the outer one returns.
- DEBT-LTC-X1, existing tests this plan deliberately updates:
  - `a_handler_that_calls_jump_fails_the_run` in `crates/promptforge-internal/engine/src/execute/tests/local_tools.rs` (lines 175-199): assert the refusal message instead of "nil value".
  - The shim walk in `crates/promptforge-internal/engine/src/lua/tests/shims.rs` (lines 186-197): replace the `jump == nil` check with a `pcall(jump, ...)` inside the handler that fails with the refusal.
  - `a_script_caller_catches_the_handlers_own_error_and_jump_is_restored` in `crates/promptforge-internal/engine/src/execute/tests/tool_call_arm.rs` (lines 337-358): its `type(jump)` clause proves nothing once the global is never swapped. Replace it with a check that the block's `jump` transfers after the caught failure.
  - `jump_works_in_the_same_block_after_the_loop_returns` in `local_tools.rs` stays as written.
- DEBT-LTC-X2: a new engine test in `local_tools.rs`. A handler raises inside `models.loop`, and the recorder shows `TOOL_CALL_FAILED` with no `tool_result` for that call id. The run fails with the handler's error.
- Regression: every other existing test passes without edits, except for mechanical additions where a test builds `ChatResult` or `Request::ToolCall` literals. That includes:
  - `crates/promptforge-internal/lua/src/dispatch-tests.rs`
  - the engine's `tool_call_arm.rs`, `local_tools.rs`, and `models_loop.rs`
  - the task built-in, `await_tasks`, and `task_events` tests
- Exit checks:
  - the workspace test suite and doctests;
  - both clippy runs with warnings denied;
  - the formatter check;
  - rustdoc with warnings denied, including the facade docs with default features;
  - the facade surface check, because `event.md` is facade documentation;
  - the user guide book build, after regenerating the combined guide.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions:
  - DEBT-LTC-C1: carry the round's turn as a field the shim adds to the `tool_call` yield.
    - Consequence: one more shim-produced field, in the same style as `call_id`. Nested loops and `call` children report correctly with no restore step.
    - Verification: the main regression test and its variants.
  - DEBT-LTC-C1: script calls keep reading the live counter, because no round requested them. A missing `turn` also falls back to the live counter, so the test-only `tools.call_as_model` hook keeps working.
  - DEBT-LTC-X1: the refusal is a plain host error, not a structured error kind. The caller sees it the same way as any other handler failure: `TOOL_CALL_FAILED`, and the value raised again at the call site.
- User-resolved architecture choices:
  - Bring DEBT-LTC-X1 and DEBT-LTC-X2 into scope. User's words: "I think you should add them both in. unless you have a good reason not to?"
  - DEBT-LTC-X1: make `jump` throw while a handler runs. User's words: "jump should be handled by temporarily replacing it with a version that throws an error."
    - Adopted as the behavior, with one change: the throw lives inside the real `jump` host function, gated by `local_handler_depth`, instead of in a stand-in placed on the global.
    - Reason: a stand-in on the global misses any reference saved before the handler runs, and a saved reference is exactly the bypass. Guarding the one real function covers the global and every saved reference, and lets `swap_jump` be deleted.
  - DEBT-LTC-X2: fix the contract doc rather than the behavior. This was the recommended option, and the operator put X2 in scope without choosing a different one.
    - It keeps the decision recorded on 2026-09-18 and changes nothing hosts observe.
    - Switch to the behavior change listed below if hosts need a result for every call.
- Rejected alternatives:
  - DEBT-LTC-C1: a frame-level "batch turn" slot that the chat arm sets and each closing local call restores from `LocalCall.report`.
    - Rejected because it adds mutable frame state whose correctness depends on the order of restores, and it would break silently if a future suspending call ran rounds without restoring.
    - Revisit if the shim protocol ever has to stop carrying per-call metadata.
  - DEBT-LTC-C1: amending `event.md` so a result's turn means the latest round at dispatch time.
    - Rejected because it contradicts lines 204, 380, and 399, breaks pairing by turn and id, and contradicts the target plan's stated intent.
  - DEBT-LTC-X1: swapping the global for a throwing stand-in.
    - Rejected because a reference saved before the handler runs still reaches the real function.
  - DEBT-LTC-X1: allowing handlers to jump.
    - Rejected because it reverses the documented rule, and `models.loop` would need rules for reporting a batch abandoned mid-way.
    - Revisit if prompts need control transfer from a tool handler.
  - DEBT-LTC-X1: a documentation-only fix.
    - Rejected because it leaves a rule with a known bypass.
  - DEBT-LTC-X2: emitting a `ToolResult` with failure text for failed model-issued local calls.
    - Rejected because it reverses the 2026-09-18 decision and changes the event stream every host sees.
    - Revisit if a host needs a result for every call.
- Assumptions and risks:
  - Every consequence was traced from code, never executed. The new regression tests are the first things that run them.
  - Hosts outside this repository were not inspected. Their exposure is inferred from the facade event contract.
  - The facade surface check needs the pinned nightly toolchain named in `crates/build-xtask/src/api/toolchain.rs`.

### Deferred and Out of Scope

- A nested `models.loop` inside a handler draining the outer loop's task notices. Revisit when a prompt's handler runs `models.loop` while the outer model owns live tasks.
- A recursion depth cap for handlers. Revisit if a real prompt hits runaway recursion.
- `user_input` is missing from the other pages of the language guide. Revisit in the next language guide documentation pass.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace default member, so plain `cargo build` builds the same thing). The desktop app builds with `cargo workshop` (alias for `run -p build-workshop --`), which builds the gateway, stages the sidecar, builds the app, and removes the staged copy; `cargo build --locked -p workshop` is the low-level form. There is no workspace-wide build step: the clippy runs below are the compile gate, and AGENTS.md forbids a standalone `cargo check --workspace` beside them.
  - Package names differ from directory names: `crates/gateway/app` is `gateway`, `crates/workshop/desktop` is `workshop`, `crates/gateway/stt/api` is `gateway-stt`, and other family crates take the family prefix (`crates/harness/runner` is `harness-runner`, `crates/promptforge-internal/engine` is `promptforge-engine`, `crates/gateway/stt/whisper-ffi` is `gateway-whisper-ffi`).
  - Anything that compiles the `workshop` crate (its clippy, tests, and doctests) runs a build script that copies the newest `target/<profile>/promptforge-gateway` into `crates/workshop/desktop/binaries/` for tauri-build, so build the gateway in the same profile first (CI uses `cargo build --locked -p gateway --no-default-features`). A Windows sidecar is already staged in this clone.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test_name_substring>`. For `workshop`, `workshop-server`, and `workshop-server-api`, drop `--all-features`. Add `--test it` to run only a crate's integration binary (`--test suite` for the `promptforge` facade). One doctest: `cargo test -p <package> --all-features --doc <name_substring>`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test -p <package> --all-features --doc` for a crate with a library target, because nextest skips doctests.
  - Workshop trio: `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `cargo nextest run --locked -p workshop-server --features headless`, then `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
  - A UI package (`crates/workshop/ui` or `crates/gateway/config-ui/ui`): `npm run typecheck`, `npm run build`, then `npm test`, in that order, because the jsdom tests read the `dist/` bundle the build writes.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the three workshop trio commands above.
  - The workspace run includes `build-xtask`, the boundary and structural harness (alone: `cargo test -p build-xtask`), which checks the tier graph, the `## Invariants` marker, lint inheritance, the 500-line ceiling, and the product-boundary matrix.
  - UI partition, when TypeScript or CSS changes: the three npm commands in both UI directories.
  - `tools/*.test.mjs` use `node:test` and no workflow runs them; run one with `node --test tools/<script>.test.mjs` when that script changes. `tools/gateway-tts-live.mjs` itself is a dev-only live probe that builds the gateway and needs `TOGETHER_API_KEY`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop trio: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - Headless feature gate, the one allowed standalone check: `cargo check -p gateway --no-default-features`.
  - Facade surface, when the `promptforge` facade changes: `cargo +nightly-2026-09-05 xtask api --check`, then `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`. The nightly is whatever `crates/build-xtask/src/api/toolchain.rs` pins (currently `nightly-2026-09-05`, installed here); the check fails at once on any other toolchain.
  - Supply chain, when dependencies change: `cargo deny check`, `cargo audit`, and `cargo tree --locked -p gateway -e normal -i ring --depth 0`, which must print no `ring v` line. `workspace-hack` is regenerated with `cargo hakari generate` and `cargo hakari manage-deps` per `.config/hakari.toml`; that file says CI runs `cargo hakari verify`, but no workflow does, so run it by hand.
  - STT crates, when they change: with `RUSTFLAGS` set to `-D warnings`, run `cargo rustc --locked -p <crate> --lib -- -F unsafe-code` for `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-stt`, then `cargo check --locked -p gateway-whisper-ffi --lib`.
  - TypeScript has no linter; `npm run typecheck` (`tsc --noEmit`) is its only static check.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`). No TypeScript or CSS formatter is configured.
- Docs command: with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS = "-D warnings"`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then the facade with default features `cargo doc -p promptforge --no-deps`, then `cargo doc --locked --no-deps -p workshop-server --document-private-items`. User guide: `cargo xtask site --books-only` (mdBook 0.4.44 is installed; staged books land in `target/site-books/`). Clippy does not cover rustdoc lints, so the docs gate is never skipped.
- Test placement and naming conventions:
  - Unit tests live in the crate under `#[cfg(test)]`, in one of three shapes that follow the flat-directory rule: an inline `mod tests { ... }`; a kebab sibling `foo-tests.rs` wired as `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;`; or, for three or more files, a `foo/tests/` directory of snake_case topic files wired by `#[cfg(test)] mod tests;` (for example `engine/src/execute/tests/`, `lua/src/protocol/tests/`, `workshop/workspace/src/workspace/tests/`).
  - Test helpers sit in a `test_support` module (engine `src/test_support.rs` plus `src/test_support/`), a `tests-*.rs` sibling (lua `tests-recording.rs`), or a `fixtures.rs` among the topic files. Hook functions are named `*_for_test`: crate-private ones under `#[cfg(test)]`, cross-crate ones exposed through a test feature. Cross-crate test seams use a `test-fixtures` feature (gateway and workshop families), `test-support` (promptforge-internal crates, `harness-runner`), or `test-helpers` (`gateway-routing`); `gateway` and `workshop-server-api` dev-depend on themselves with that feature so gate commands need no `--features` flag.
  - Integration tests are one binary per crate at `tests/it/main.rs` with snake_case topic modules (a large topic becomes `topic.rs` plus `topic/`); the `promptforge` facade uses `tests/suite/main.rs`. A few crates keep loose single-file binaries instead (`gateway-stt-engine`, `gateway-stt-backend-whisper`, `gateway-cloud-providers`, `build-workshop`). Shared helpers go in `support.rs` (`common/mod.rs` in workshop-server), JSON fixtures in `tests/fixtures/`, and prompt-program fixtures are `.md` files under `tests/prompts/{valid,invalid,execution}/`. Benches are criterion files under `benches/` (engine `models_loop.rs`, lua `surface.rs`).
  - Test functions are snake_case sentences stating the behavior (`a_process_lifetime_lease_recovers_after_its_owner_is_terminated`). `unwrap` and `expect` are allowed in tests only (root `clippy.toml`, restated in each harness crate's own `clippy.toml`).
  - JavaScript: Workshop UI tests are `crates/workshop/ui/test/<topic>.mjs`, config UI tests sit beside their sources as `src/**/<name>.test.mjs`, and tool tests are `tools/<script>.test.mjs`. Both UI packages run tests with `node --test` and use jsdom for the DOM.
  - Behavior changes ship with tests in the same change; product and behavior tests are preserved through refactors.
- Directory map:
  - `crates/`: every Rust crate plus the UI packages. The root holds the public layer (`promptforge`, `harness-api`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`), `build-*` tooling (`build-xtask`, `build-ui`, `build-workshop`, `build-user-guide`, `build-llama-cuda`), `workspace-hack` (cargo-hakari), `shared-ui` (a TypeScript and CSS package, not a crate), and `README.md` describing each root crate.
  - `crates/promptforge-internal/`: private engine family (`engine`, `lua`, `parser`, `store`, `vfs`, `model-client`, `types`).
  - `crates/gateway/`: private gateway family (`app`, `cloud-providers`, `config`, `config-ui` with its `ui/` SPA, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem of `api`, `engine`, `backend-whisper`, and `whisper-ffi`).
  - `crates/harness/`: private harness family (`runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, `web-search`).
  - `crates/workshop/`: private workshop family (`desktop` is the Tauri app; `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, `workspace`; `ui/` is the npm and esbuild SPA).
  - `guide/`: user guide sources (`src/{language,gateway,workshop}` plus `introduction.md`, `books/`, `landing/`, `chrome/`), `CONTRIBUTING.md` for guide authors, and the three per-book `promptforge-*-guide.md` exports.
  - `prompts/`: sample prompt programs. `tools/`: Node scripts (gateway sidecar staging, live TTS probe) with their tests, plus `dokuman-promptforge.md`. `images/`: README banners.
  - `vibe/`: planning workspace (`archdoc.md`, dated plan files, monthly archive folders, and a gitignored `scratch/`).
  - `.github/`: `workflows/ci.yml` is the merge gate (one `ci-green` job over fmt, clippy, test, docs, workshop on Windows and Linux, UI, supply chain, and API surface); the other workflows cover release, nightly, installer smoke, docs site, Miri, and native libraries. `fixtures/workshop-package-smoke/` holds the package smoke test's gateway config.
  - `.githooks/`: pre-commit runs fmt; pre-push runs the headless gateway check, clippy, and `cargo deny`. They are opt-in and not active in this clone (`core.hooksPath` is unset).
  - `.cursor/rules/`: two Workshop rule files (`workshop-architecture.mdc`, `workshop-spa.mdc`).
  - `.config/`: `nextest.toml` (60-second slow timeout, a `heavy` test group for the STT crates) and `hakari.toml`. `.cargo/config.toml`: `rust-lld` and static CRT on Windows, plus the `xtask` and `workshop` aliases.
  - Root files: `Cargo.toml` (members, `[workspace.dependencies]`, lints), `AGENTS.md` (repository policy and verification gates), `deny.toml`, `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml` (cargo-dist, gateway Linux installers only), `gateway.local.example.toml`, `LICENSE` (BSL-1.0).
  - Local only and gitignored: `local/` (operator profiles, prompts, stores, gateway config, STT fixtures), `target/`, `target-msrv/`.
- Component boundaries:
  - Runtime shape: the executor (`promptforge-engine`) is a sans-I/O state machine with no I/O and no clock, driven through `Run::new`, `step`, `resume`, and `cancel`. The harness is its only production host and owns the tokio runtime, the effect performers, sessions, and the Turso run log. The gateway is a separate server process that owns model routing and vendor credentials. The Workshop drives sessions through `harness-api` and attaches over the gateway protocol. `vibe/archdoc.md` also lists a CLI component, but no CLI crate or binary exists in the workspace.
  - PromptForge: `promptforge` is the one public crate, a facade over `crates/promptforge-internal/` whose surface is committed in `crates/promptforge/public-api.txt`. Inside, the engine depends on lua, parser, store, model-client, types, and vfs; parser on lua and types; lua on store, model-client, and types; store on vfs; model-client on types; types and vfs on nothing. The family depends on no gateway, workshop, or harness crate.
  - Harness: `harness-api` is the only public surface over `crates/harness/`. Harness crates may depend on `promptforge`, the gateway public pair, and shared-* crates, never on workshop or private gateway crates, and spawn tasks only through `harness-runner`'s instrumented wrapper.
  - Gateway: the public pair is `gateway-api-types` (types only) and `gateway-api-discovery` (discovery file, launch lock, health probe); everything else is private under `crates/gateway/`, and the STT subsystem exposes only `gateway-stt` to the rest of the family. Gateway crates depend on no promptforge, workshop, or harness crate.
  - Workshop: private under `crates/workshop/`; may name `promptforge`, the gateway public pair, and `harness-api` only. The desktop app depends on `workshop-server-api`, never on `workshop-server`. Internal tiers flow one way: server, then features, then services, then vocabulary.
  - Shared: shared-* crates depend on no product crate.
  - Composed rule: a crate in a family container may depend only on crates at the `crates/` root and its own siblings; `build-*` crates are exempt. The rules bind normal, dev, build, and target-specific dependencies (one exception: promptforge-internal crates may dev-depend on `promptforge` for doc examples only), and `cargo test -p build-xtask` enforces them.
- Conventions summary:
  - Rust edition 2024 on the stable toolchain, resolver 3. Every dependency is declared once in `[workspace.dependencies]` with a comment justifying any pin or feature choice, and every member inherits `workspace-hack`.
  - Workspace lints: `unsafe_code` forbidden; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn; clippy `all` and `pedantic` deny; `unwrap_used` and `expect_used` deny outside tests; broken or private intra-doc links deny. Crates that own an unsafe boundary (such as the `gateway-api-discovery` `src/sys/` shims and the desktop app's WebView2 bridge) mirror the set with `unsafe_code` lowered to deny, and every unsafe block documents its safety invariants immediately before it.
  - Source directories are flat: a subdirectory needs at least three files; otherwise use kebab siblings `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`. Convert in either direction when touching a group on the wrong side of the line. Top-level `tests/` and `benches/` are exempt.
  - Every workshop-* and harness-* `lib.rs` opens with a `//!` doc containing a `## Invariants` marker listing allowed and forbidden dependencies; no file in a marker crate exceeds 500 lines (split first, then edit). The desktop app is exempt.
  - Error and status messages are written for model consumption: concise, factual, self-contained, naming required versus actual.
  - Comments explain only non-obvious constraints, ordering, or workarounds; platform or external-bug workarounds cite an upstream issue URL.
  - Run-log and replay JSON round-trips exactly: canonical sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Cargo features gate real constraints (toolchains, heavy native builds), never product shape. Library and serve paths return failures instead of exiting or installing process-global state, and runtime paths never compile native code or invoke build tools.
  - No build step writes into the repository: the UI bundles build into `OUT_DIR`, and CI fails on a dirty tree after building.
  - Prefer types and compiler checks, then behavior tests. New structural checks (parsers, allowlists, counts, topology checks) need explicit user approval, and plans cannot introduce them otherwise.
  - Design order: reuse an existing facility, then make the smallest improvement to one, then add a new facility only with material benefit.
  - SPA: CSS sits beside its TypeScript, component CSS uses `--ws-*` tokens only, lazy panels never import entry-bundle modules, and there is no `localStorage` (persisted UI state goes through the server to `ui-state.json` or the `.pfwork` workspace file).

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Turn fields in the shim protocol [completed]

- Component: batch turn (DEBT-LTC-C1)
- Component order: first. It removes the only debt the target introduced, which is the primary cleanup goal. It is independent of the other two components, and settling its `__impl_coro.lua` and `event.md` edits first lets the later components edit those files against their final shape.
- Piece: protocol vocabulary. Built before the propagation piece, because the shim, scheduler, and built-in changes all read these two fields.
- Artifacts:
  - `ChatResult` in `crates/promptforge-internal/lua/src/protocol/answer.rs` gains `turn: u32`.
  - The chat arm in `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs` sets `turn` to the value it reports `AssistantToolCalls` under (around `advance_turn` near line 204 and `assistant_tool_calls` near lines 338-339).
  - `chat_result_table` in `crates/promptforge-internal/lua/src/protocol/render.rs` renders it as `turn`.
  - `Request::ToolCall` in `crates/promptforge-internal/lua/src/protocol/request.rs` gains `turn: Option<u32>`.
  - `parse_tool_call` in `crates/promptforge-internal/lua/src/protocol/parse.rs` reads `turn` as a shim-produced field: absent or nil gives `None`, an integer within `u32` range gives `Some`, and anything else is a malformed yield.
  - `prepare_tool_call` accepts the new field but does not use it yet, so reported turns are unchanged by this step.
  - Mechanical literal additions wherever tests build `ChatResult` or `Request::ToolCall`, including `crates/promptforge-internal/lua/src/dispatch-tests.rs` and the engine's `tool_call_arm.rs`, `local_tools.rs`, and `models_loop.rs`.
- Tests:
  - Parse tests: absent `turn` gives `None`, a valid integer gives `Some`, and a negative number, a float, and a string are each a malformed yield.
  - A render test showing the chat result table carries `turn`.
- Verification: `cargo nextest run --locked -p promptforge-lua --all-features` and `cargo nextest run --locked -p promptforge-engine --all-features`, then both crates' doctests with `cargo test -p <package> --all-features --doc`.
- Commit: one commit with the fields, parser, renderer, chat arm assignment, and their tests.

</step-1>

<step-2>

### Step 2: Report the requesting round's turn on every model-issued result [completed]

- Component: batch turn (DEBT-LTC-C1)
- Piece: turn propagation. Built after the protocol vocabulary piece, which it depends on. The shim, scheduler, and built-in changes form one behavior, "every model-issued `ToolResult` carries the batch turn", and one regression test set covers all of it, so they ship together.
- Artifacts:
  - In `crates/promptforge-internal/lua/src/__impl_coro.lua`, `models_loop` calls `tools_call_as_model(call.id, call.name, call.arguments, round.turn)`, and `tools_call_as_model` adds `turn` to its `tool_call` yield. The test-only `tools.call_as_model` hook may leave `turn` out.
  - In `prepare_tool_call` (`crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 139-141), the report becomes `ScriptReport { turn: turn.unwrap_or(live counter) }`. Script calls (`call_id: None`) keep the live counter. The value reaches the local path through `LocalCall.report` and the bound path through `ToolCallContinuation.report` and `ModelReport` (`apply.rs` around lines 155-158) with no further change.
  - `report_builtin_answer` in `crates/promptforge-internal/engine/src/execute/scheduler/builtins.rs` (lines 227-254) takes the dispatch turn as a parameter and stops reading `chain.ctx.turns()`. `answer_task_builtin` passes it through.
  - `AwaitTasks` in `crates/promptforge-internal/engine/src/execute/scheduler/await_tasks.rs` and the `task_events` continuation store the dispatch turn next to the call id, so their later answers report it.
  - `crates/promptforge/src/event.md` line 398: change "the round that dispatched the call" to the round that requested the call, and state that a script-issued call reports the counter's value when the script dispatched it.
  - Mechanical literal additions in the task built-in, `await_tasks`, and `task_events` tests.
- Tests:
  - Main engine regression: round 1 returns `c1`, a local `grab` whose handler calls `models.infer` (with its own scripted text reply), and `c2`, a bound `echo` from `echo_tools()`; round 2 returns final text. A recording observer captures the turn on `Event::AssistantToolCalls` and `Event::ToolResult`, and the batch turn, `c1`'s result turn, and `c2`'s result turn are equal.
  - Variant: the handler runs its own `models.loop` with one inner tool call. The inner result reports the inner round's turn, and `c2` reports the outer batch's turn.
  - Variant: the handler uses `call` into a section that runs `models.infer`. `c2` reports the outer turn.
  - Variant: `c2` is a task built-in. Cover `task_status`, and add a case where `c2` is a parked `await_tasks` answer so the stored turn is exercised.
  - A shim walk showing that the loop's `tool_call` yield carries the same `turn` as the chat result that requested it.
- Verification:
  - `cargo nextest run --locked -p promptforge-lua --all-features` and `cargo nextest run --locked -p promptforge-engine --all-features`, then both crates' doctests.
  - Because `event.md` is facade documentation: with `RUSTDOCFLAGS` set to `-D warnings`, `cargo doc -p promptforge --no-deps`; then `cargo +nightly-2026-09-05 xtask api --check` and `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`).
- Commit: one commit with the shim, scheduler, built-in, and contract changes and their tests.

</step-2>

<step-3>

### Step 3: Refuse `jump` inside a local tool handler

- Component: handler jump guard (DEBT-LTC-X1)
- Component order: second. It is independent of the batch turn component, but it edits `__impl_coro.lua` and `coro.rs` in different functions, so it lands after Step 2 to keep those edits sequential. It precedes the contract component because that component's new test sits in `local_tools.rs`, which this step also updates.
- Piece: one piece. The counter, the guard, the new captures, and the removal of `swap_jump` cannot compile or behave correctly apart, so they are built jointly.
- Artifacts:
  - `SectionVm` in `crates/promptforge-internal/lua/src/vm.rs` gains `local_handler_depth: Arc<AtomicU32>`, next to `jump_slot` (field near line 88), shared with its closures.
  - `install_jump_global` (`vm.rs` lines 595-609) checks the counter first. While it is above zero, the function returns a plain host error without resolving the target or writing `jump_slot`. The message is written for a model to read: "jump is unavailable inside a local tool handler: return a value from the handler and call jump from the block after the tool call returns".
  - In `crates/promptforge-internal/lua/src/coro.rs` (lines 148, 182, 196, 267 onward), the `swap_jump` capture is replaced by `enter_local_handler` and `leave_local_handler`. `SectionVm::install_coro_shims` builds them over the same counter and passes them into `install_shim_prelude`.
  - `run_local_tool` in `__impl_coro.lua` calls `enter_local_handler()`, then `raw_pcall(handler, args)`, then `leave_local_handler()`, and never touches the global `jump`. Update the prelude header comments that describe `swap_jump`.
  - Guide: extend the handler sentence in `guide/src/language/07-tools.md` line 66 to say that calling `jump` from a handler raises an error, even through a saved reference. Regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`, never by hand.
- Tests:
  - New, saved reference: the block runs `local j = jump`, then registers a handler that calls `j('## Other')`. Through a script `tools.call` and through `models.loop`, the run fails with the refusal message and does not jump.
  - New, caught refusal: the caller wraps `tools.call` in `pcall` around that handler, and the block then returns `'no jump'`. The run returns `'no jump'`.
  - New, nesting: an outer handler calls an inner local tool. `jump` is still refused in the outer handler after the inner one returns, and works in the block once the outer one returns.
  - Updated: `a_handler_that_calls_jump_fails_the_run` in `crates/promptforge-internal/engine/src/execute/tests/local_tools.rs` (lines 175-199) asserts the refusal message instead of "nil value".
  - Updated: the shim walk in `crates/promptforge-internal/engine/src/lua/tests/shims.rs` (lines 186-197) replaces the `jump == nil` check with a `pcall(jump, ...)` inside the handler that fails with the refusal.
  - Updated: `a_script_caller_catches_the_handlers_own_error_and_jump_is_restored` in `crates/promptforge-internal/engine/src/execute/tests/tool_call_arm.rs` (lines 337-358) replaces its `type(jump)` clause with a check that the block's `jump` transfers after the caught failure.
  - Unchanged and passing: `jump_works_in_the_same_block_after_the_loop_returns` in `local_tools.rs`.
- Verification: `cargo nextest run --locked -p promptforge-lua --all-features` and `cargo nextest run --locked -p promptforge-engine --all-features`, then both crates' doctests, then `cargo xtask site --books-only` after the guide regeneration.
- Commit: one commit with the guard, the captures, the shim change, the guide edit and regenerated guide, and the new and updated tests.

</step-3>

<step-4>

### Step 4: State the local tool failure exception in the event contract

- Component: event contract correction (DEBT-LTC-X2)
- Component order: last. It is a documentation change plus one pinning test, depends on nothing else, and lands after Step 2's line-398 rewording so the finished contract paragraph can be read as a whole. Its test goes into `local_tools.rs` after Step 3's edits to that file. Because it is last, it also carries the plan's exit checks.
- Piece: one piece, built as one unit.
- Artifacts:
  - In `crates/promptforge/src/event.md`, amend lines 204, 380, 396, and 397 to state the exception for a model-issued call to a Lua-local tool: when its handler raises or returns an unsupported value, the call reports `ToolCallFailed` and no `ToolResult`, and the failure propagates to the caller of `models.loop` and ends the loop unless the author catches it. Every other rule in those lines stays as written.
  - No code change. `dispatch_local_tool_done` in `tool_call.rs` keeps its current behavior.
- Tests:
  - A new engine test in `local_tools.rs`: a handler raises inside `models.loop`, the recorder shows `TOOL_CALL_FAILED` with no `tool_result` for that call id, and the run fails with the handler's error.
- Verification, the exit list from the Testing Plan:
  - Build the gateway in the same profile first (`cargo build --locked -p gateway --no-default-features`), because the workshop crate's build script stages it.
  - Tests: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, `cargo nextest run --locked -p workshop-server --features headless`, and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
  - Clippy: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`.
  - Formatter: `cargo fmt --all --check`.
  - Rustdoc with `RUSTDOCFLAGS` set to `-D warnings`: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p promptforge --no-deps`, then `cargo doc --locked --no-deps -p workshop-server --document-private-items`.
  - Facade surface: `cargo +nightly-2026-09-05 xtask api --check`, then `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
  - User guide book build: `cargo xtask site --books-only`.
- Commit: one commit with the contract amendment and the new test.

</step-4>

</execution-plan>
