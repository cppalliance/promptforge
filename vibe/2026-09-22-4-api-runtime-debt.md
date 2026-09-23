---
name: api-runtime debt cleanup
overview: "Reduce technical debt in promptforge-api-runtime without changing behavior: make the crate root the one public facade, shrink the feature-gated test_support surface, move the public-API-only tests into tests/suite, fix stale docs, and remove duplicated scheduler and test-only plumbing. No new crate and no new structural checks."
todos:
  - id: docs
    content: "Docs accuracy: README badges/version, lib.md, stale lua/untrusted docs, execute.rs module map, test_support voice, legacy comments, crate AGENTS.md, Run step/resume contract docs"
    status: pending
  - id: facade
    content: "Root facade: make execute private, root-only re-exports, rewrite dependent imports and doctests, resolve unreachable_pub and private_intra_doc_links"
    status: pending
  - id: modelseams
    content: "Model seams: drop Applied/SseScanner/StreamAccumulator hidden re-exports (keep ToolSchemaError and the transport seams)"
    status: pending
  - id: demotions
    content: "Demotions: apply each demotion candidate that meets its condition, with the accessor it is paired with, and leave the rest public"
    status: pending
  - id: testsupport
    content: "Trim test_support pub surface to the listed names (recording keeps Observer and Observation), delete RecordingObserver, keep TokioDriver/mock/hooks cfg(test)"
    status: pending
  - id: movetests
    content: "Move args_surface, lazy_prose, offline exec_flow cases to tests/suite and declare them in tests/suite/main.rs"
    status: pending
  - id: splittests
    content: "Split tests/scheduler.rs and tests.rs along identified seams (tests/scheduler/ in standard layout; plain modules under tests/)"
    status: pending
  - id: answerinline
    content: "Hoist answer_inline into core Scheduler and replace the 16 hand-written sites across 11 modules"
    status: pending
  - id: testhost
    content: "Remove cfg(test) test_host from RunContext/RunState and the RunContext test builders; build RunHost in tests and pass it to TokioDriver"
    status: pending
  - id: substtests
    content: "Move subst.rs test block to subst-tests.rs"
    status: pending
  - id: verify
    content: "Record baseline test count with --all-features; run the crate inner loop after each work item; before finishing run clippy, nextest + doctests, cargo doc -D warnings, fmt, build-xtask and confirm the count is not lower"
    status: pending
isProject: false
---

# promptforge-api-runtime technical debt cleanup

<product-contract>

## Product Requirements

The `promptforge-api-runtime` crate has picked up duplicate public paths, a wider test-support surface than its callers use, stale documentation, and repeated scheduler plumbing. This plan removes that debt without changing run behavior. It answers the owner's five questions: whether the public API can be simplified, whether tests can be made private, whether tests can move to a new crate, whether documentation can be improved, and how complexity can be reduced.

- Problem and users:
  - Users are the maintainers of the runtime and of the in-workspace crates that depend on it: `crates/harness/runner`, `crates/harness/sessions`, `crates/harness/capabilities`, `crates/harness/models`, and `crates/workshop/workspace` (each lists `promptforge-api-runtime` in its `Cargo.toml`).
  - The crate is about 38k lines, of which about 21k are tests (line counts across `crates/promptforge-api-runtime/src`, `tests`, and `benches`).
- Goals:
  - One public path per host type.
  - A feature-gated `test_support` surface limited to what its callers use.
  - Tests that need only the public API live in the integration suite.
  - Docs that match the crate's actual visibility, version, and publish status.
  - Less repeated scheduler code, and no test-only fields in production structs.
- Non-goals:
  - No change to run behavior, effect semantics, event output, or log schema.
  - No new crate.
  - No new structural enforcement such as source parsers, allowlists, counts, ceilings, or topology checks.
- Success criteria: every exit criterion in Testing Plan passes, the workspace test count is not lower than the baseline, and every work item in Execution Instructions is done.
- Constraints:
  - Among promptforge crates, outside crates may depend only on `promptforge-api-runtime` and `promptforge-api-types`. The crates under `crates/promptforge/` are private to the family, and `cargo test -p build-xtask` enforces this (stated in `AGENTS.md`, Structure section).
  - Behavior and product tests are preserved during refactors. Tests may move but must not be dropped.
  - Source directories are flat by default. A group of three or more files is a subdirectory in standard layout; one or two files belong beside the parent module as `parent-label.rs` siblings wired with `#[path = "parent-label.rs"] mod label;` (stated in `AGENTS.md`, Structural Rules section).
  - The crate is `publish = false` (`crates/promptforge-api-runtime/Cargo.toml` line 7). Its version comes from the workspace at 0.3.0 (`Cargo.toml` line 20; line 28 is the crate's entry in the workspace dependency table, which repeats the version).
  - Comments state only non-obvious constraints, ordering requirements, or workarounds.
- Open questions: none. Commit granularity is settled by the execution section: one commit per step, thirteen steps, with the first and last steps recording their measured counts in this plan's Project Survey.

## Functional Specification

The only externally visible changes are import paths and the set of public items. Hosts drive a run exactly as today: prepare with `Environment`, loop on `Run::step`, and answer effects with `Run::resume`. Test-support callers keep every driver they use.

- Actors and workflows:
  - Host crates import runtime host types from the crate root and vocabulary from the `model`, `parser`, `input`, and `types` modules.
  - Test callers are the runtime's unit tests, `crates/promptforge-api-runtime/tests/suite`, `crates/promptforge-api-runtime/benches/models_loop.rs`, and `crates/harness/capabilities/tests/it/support.rs`, which imports `Performers` and `drive_tokio` at line 15.
- Inputs and outputs: unchanged. `Effect::record` and `EffectAnswer::record` keep returning the public record types the harness runner serializes to its log.
- States and validation: unchanged.
- Errors and recovery: unchanged. `RunError`, `RunErrorKind`, and the internal `Error` keep their current classification.
- Security and privacy behavior: unchanged. Store access handles are still minted only inside the engine, and `perform_store_op` uses the handle it is given without deriving, widening, or retaining scope (`crates/promptforge-api-runtime/src/execute.rs` lines 122-140).
- Acceptance criteria:
  - No crate names `promptforge_api_runtime::execute`.
  - No `pub` item is left unreachable once `execute` is private: the crate is clean under `unreachable_pub`, which the workspace enables as a warning and the exit criterion's `-D warnings` turns into an error.
  - Every intra-doc link still resolves to a publicly reachable item, so the docs gate passes with `private_intra_doc_links` denied.
  - Every item under File and public API changes is in its stated end state.
  - The doctests in `crates/promptforge-api-runtime/src/lib.md` and `src/test_support.rs` compile and pass.

</product-contract>
<implementation-contract>

## Technical Design

The crate root becomes the single public facade for host types, and `execute` becomes a private module. `model`, `parser`, `input`, and `types` stay public as vocabulary modules. `test_support` stays in-crate behind the `test-support` feature with a trimmed public surface. Two internal refactors change structure across modules: a shared inline-answer helper on the scheduler, and removal of the test-only host field from the production context structs. All paths below are under `crates/promptforge-api-runtime/` unless they start with `crates/`.

- Architecture:
  - Root facade: `src/lib.rs` changes `pub mod execute` (line 5) to a private `mod execute`. The root must re-export every item that stays public inside `execute`; the compiler-derived set is authoritative and the list here is representative, not exhaustive. From `src/execute.rs`: `ModelBindings` and `ToolBindings` (line 107, same line); `CapabilityConflict`, `RequirementCheck`, `Requirements`, `UnmetRequirement` (line 111); `AnswerRecord`, `ChatAnswerRecord`, `Effect`, `EffectAnswer`, `EffectId`, `EffectRecord`, `InputAnswerRecord`, `Run`, `Step`, `StoreAnswerRecord`, `ToolAnswerRecord` (lines 112-115); `StoreOp`, `StoreOutcome` (line 119); `StoreError` (line 120); and the `perform_store_op` function (line 135, a function, not a re-export). Names already reachable at the crate root need no new re-export - confirm each against the compiler (`unreachable_pub` reports every miss) rather than against this list. `ModelBindings` is re-exported here only while its accessor stays public; Step 4 removes it only together with the accessor.
  - The facade change lands whole or not at all. `Cargo.toml` sets `[workspace.lints.rust] unreachable_pub = "warn"` and `[workspace.lints.rustdoc] private_intra_doc_links = "deny"`, and the exit criterion passes `-D warnings`, so with `execute` private the crate is not clean until every `pub` item inside it is either re-exported at the root or demoted to `pub(crate)`. A partial facade is a failing build, not a warning to clean up later.
  - It also reaches the crate's landing docs: `src/lib.md` line 3 links [`execute`] (the module-split prose itself is `src/execute.rs` lines 64-87), and a link to the now-private module fails rustdoc under `private_intra_doc_links = "deny"`.
  - Dependents rewrite their `promptforge_api_runtime::execute::...` imports to root paths. The sites are: `crates/harness/runner` (`src/effect_loop.rs` line 35, `src/effect_loop-answering.rs` line 9, `src/performers-host.rs` line 14, `src/performers.rs` line 29, `src/prepare.rs` line 30, `tests/it/prepare.rs` line 20, `tests/it/support.rs` line 16); `crates/harness/capabilities` (`src/activation.rs` line 20, `tests/it/activation.rs` line 8, `tests/it/assembly.rs` line 12, `tests/it/support.rs` line 13); `tests/suite/{execution,fanout,prepare,support,vfs}.rs`; the doctests in `src/execute/config.rs` line 43, `src/execute/config-limits.rs` lines 37 and 60, and `src/execute/run.rs` line 92; and `benches/models_loop.rs`. `crates/harness/sessions` and `crates/workshop/workspace` depend on this crate but already import through root or `input`/`parser` paths, so neither needs a rewrite. The compiler finds the full set; the acceptance check is a workspace-wide search for the `execute::` path.
- Modules and interfaces:
  - Inline answer: `answer_inline` (`src/execute/scheduler/waits.rs` lines 56-61) moves to the core `impl Scheduler` in `src/execute/scheduler.rs`. It replaces the hand-written `incoming = Some(..)` plus `ready.push_back(..)` pairs in the scheduler's `chat`, `timer`, `dispatch`, `tasks`, `await_tasks`, `task_events`, `apply`, `step`, `tool_call`, `notices`, and `chain` modules: sixteen sites across those eleven files. (`incoming = Some` matches seventeen times across twelve files in `src/execute/scheduler`; the twelfth file is `waits.rs`, where the match is the helper's own body.)
  - Ready-queue order must not change. Where a site also pushes a spawned child (`src/execute/scheduler/tasks.rs` line 140, `src/execute/scheduler/tool_call.rs` line 144), the helper runs first and the child push stays after it. `src/execute/scheduler/step.rs` lines 229-232 set `incoming` through a borrowed chain alongside `chain.coroutine`; convert that site only if the borrow allows it without reordering.
  - Test host removal: remove `RunContext.test_host` (`src/execute/config.rs` lines 121 and 155) together with the `#[cfg(test)] impl RunContext` block that seeds it (`src/execute/config.rs` lines 351-397: `observer`, `debug`, `client`, `input_broker`, and `on_delta`) and the `test_host` field on that type's `Debug` impl (lines 402-403); remove `RunState.test_host` with `test_host()` and `set_test_host()` (`src/execute/context.rs` lines 77, 167, and 186-203). `TokioDriver` takes the `RunHost` as an explicit parameter instead of reading it off the state (`src/test_support/tokio_driver.rs` line 162), as `TokioDriver::new(state, host, client)`. `RunHost`'s own public builders (`src/test_support/host.rs`) are what the tests then use to assemble it. The callers to migrate are the fixtures that today configure a context: `src/execute/tests.rs` line 433 and `to_context` at line 287, `src/execute/run-tests.rs` lines 270, 316, and 344, `src/execute/tests/effects.rs` lines 71-80 and 175-185, `src/execute/tests/input.rs` lines 164-166, 198-199, 254-256, 270-280, 290-300, 306-316, and 333-343, and `src/execute/tests/model_tasks.rs` lines 63-65. `RunContext::debug`'s `report_debug = DebugMode::On` side effect moves wherever the debug capture is now installed. The `tap` and `raw_shims` test seams stay.
- File and public API changes:
  - Removed from the public surface: the `execute` module path; `test_support::RecordingObserver`, which has no references outside `src/test_support.rs` line 52 and `src/test_support/recording.rs`; and the `#[doc(hidden)]` re-exports `Applied`, `SseScanner`, and `StreamAccumulator` in `src/model.rs` lines 30-33, which no crate outside the promptforge family names.
  - Kept in `src/model.rs`: `ToolSchemaError`, because the public `ToolSchema::new` returns it (`crates/promptforge/model-client/src/client/wire.rs` lines 219-265). Also kept are the hidden seams that `crates/harness/models/src/transport.rs` imports at lines 14-18: `ChunkSource`, `ClientError`, `ClientTimeout`, `build_request_body`, `escape_controls`, `read_body_capped`, and `read_completion_stream`.
  - Demotion candidates: `ModelBindings`, `SourceLocation`, and `input::INPUT_UNAVAILABLE_FALLBACK`. Each becomes `pub(crate)` only if no crate outside the runtime names it after the facade change and no remaining public signature exposes it. For the first two the paired accessor decides it: `ModelBindings` cannot be demoted while `RunContext::model_bindings` is public, and `SourceLocation` cannot while `RunError::location` is public. Demoting an accessor is a removal from the host API, so it is a decision to state, not a side effect to discover; a candidate whose accessor stays public stays public. The facade item re-exports `ModelBindings` at the root, because the public `RunContext::model_bindings` returns it; the demotion item deletes that re-export only in the same change that demotes the accessor.
  - The record types (`AnswerRecord`, `EffectRecord`, and the four `*AnswerRecord` types) stay public.
  - No `#[non_exhaustive]` is added to `Effect`, `EffectAnswer`, `Step`, or `RunResult`.
  - The public `test_support` surface after the trim is `drive`, `drive_tokio`, `Performers`, `Performer`, `BoxFuture`, `RunHost`, `run_host`, `run_with_host`, `ChatClient`, `DeltaHook`, `TestTool`, `TestToolTable`, `TestBroker`, and `forward`, plus `recording::Observer` and `recording::Observation`, the only two `recording` items the suite names (`tests/suite/support.rs` line 12, `tests/suite/execution.rs` line 10). The rest of `recording` (`RecordingObserver`, `DebugCapture`, `NullObserver`, `detail`, and `null_emitter`) drops out of the public surface and stays available to the in-crate suites. `TokioDriver`, `MockGatewayClient`, and `src/execute/scheduler/test_hooks.rs` stay `cfg(test)`. The module-doc rewrite at `src/test_support.rs` lines 10-28 belongs to this trim, not to the docs item, because it describes the surface this trim narrows. The `#[cfg(test)] pub(crate) use promptforge_parser::test_support::synthetic_section` re-export (lines 29-30) also leaves, because a re-export that exists only to feed test modules is test plumbing in a production namespace: its two consumers in `src/execute/tests/exec_flow.rs` import `promptforge_parser::test_support::synthetic_section` directly, which the dev-dependency already enables.
  - The bench imports `BoxFuture`, `ChatClient`, `DeltaHook`, `RunHost`, and `run_with_host` (`benches/models_loop.rs` lines 34-36). It also includes `src/test_support/mock-gateway-client.rs` through a `#[path]` attribute (lines 43-44), so that file keeps its current path.
  - Docs:
    - `README.md`: remove the crates.io, docs.rs, and license badges (lines 3-5), and change the dependency snippet (lines 11-14) to `promptforge-api-runtime.workspace = true`.
    - `src/lib.md` line 3: retarget the [`execute`] link and the module description at the root items, since the module is now private and the link is denied by lint.
    - `src/lua.rs` (lines 11-12) and `src/untrusted.rs` (lines 3-5): rewrite the module docs as a crate-internal import surface, matching `src/store.rs` and `src/tools.rs`.
    - `src/execute.rs`: replace the module-layout paragraph (lines 64-87) with a bullet map, one line per child module.
    - Remove the comments that compare against old "legacy" paths in `src/execute/scheduler.rs` and in the scheduler's `apply`, `dispatch`, `step`, and `walk` modules.
    - `Run::step`, `Run::resume`, and `Run::cancel` in `src/execute/run.rs`: document the step/resume contract, including that each answer must match its effect's kind and that the methods are infallible by design.
    - `crates/promptforge-api-runtime/AGENTS.md`: reread against the new facade and record whether its historical-path rule and its module description still hold, and whether the root facade belongs in the single-public-crate paragraph.
  - Test file moves and splits are described under Execution Instructions and in Decision Record notes.
- Data, persistence, failure, security, and privacy constraints: events, effect records, and answer records serialize the same way before and after the change, so the run log format does not change.

</implementation-contract>
<verification-contract>

## Testing Plan

This is a behavior-neutral refactor, so the existing suites are the main check, backed by the full workspace gates. Moved tests keep their assertions unchanged apart from helper and import swaps, and the total test count must not drop.

- Unit: all in-crate unit tests pass, including every test in the split files.
- Integration and end-to-end: `tests/suite` passes with the moved tests, and each moved module is declared in `tests/suite/main.rs`. The dependents' suites (harness runner, sessions, capabilities, and models, plus workshop workspace) pass after the import rewrite.
- Regression, security, and performance: the `models_loop` bench builds under `--all-targets --all-features`. There is no performance target.
- Exit criteria:
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` passes. This is the gate that catches an unreachable `pub` left behind by the facade change.
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` passes, followed by `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`.
  - `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"` passes. This is the gate that catches an intra-doc link left pointing at the private `execute` module.
  - `cargo fmt --all --check` and `cargo test -p build-xtask` pass.
  - The workspace test count from `cargo nextest list` is not lower than the baseline recorded before the first change. The list must use the same flags as the nextest run, `--all-features` included: without it, the `suite` and `models_loop` targets are excluded by their `required-features = ["test-support"]` (`crates/promptforge-api-runtime/Cargo.toml` lines 61-68), and the ratchet would not cover the tests this plan moves. The baseline count is appended to this plan's Project Survey before the first edit and the acceptance count after the last, because the run ledger is scratch and never committed; the two recorded lines are what the ratchet compares. The count is a case count: if a split re-parametrizes a test, re-measure and confirm coverage rather than assuming the total is unchanged.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - The crate root is the only public path for host types. Dependents mix root and `execute::` paths for the same types, so one path halves what a reader has to learn. User's words: "Root-only facade: move all host types to the crate root, make `execute` private; keep `model`, `parser`, `input` as vocabulary modules".
  - No new crate. `test_support` is trimmed in place and the public-API-only tests move into `tests/suite`. User's words: "No new crate: shrink test_support to a minimal feature-gated surface, keep the rest cfg(test), and move the ~2-3k lines of public-API-only tests into tests/suite".
  - The record types stay public, because `Effect::record` and `EffectAnswer::record` are public and return them, and the harness runner logs their output.
  - `ToolSchemaError` stays public, because the public `ToolSchema::new` returns it (`crates/promptforge/model-client/src/client/wire.rs` lines 219-265).
  - No `#[non_exhaustive]` on `Effect`, `EffectAnswer`, `Step`, or `RunResult`. The harness runner is the only production host and matches these values (`crates/harness/runner/src/effect_loop.rs` line 37 imports them). With `#[non_exhaustive]` it would need wildcard arms and would stop getting a compile error when a variant is added.
  - The `lua`, `model`, `fanout`, and `tools` tests stay in this crate. `src/lua/tests.rs` lines 9-12 state that they exercise the executor's `section_vm` path and yield protocol, and the fanout tests cover `resolve_sibling` in `src/fanout.rs`. The private crates never depend on the executor (`crates/promptforge-api-runtime/AGENTS.md`), so these tests cannot move there.
  - The internal `Error` and the public `RunError` stay separate. `RunError` is the stable public classifier with 15 kinds over an internal error with about 36 variants (`src/execute/error.rs` lines 81-194, `src/error.rs`).
  - A test's `RunHost` becomes an explicit `TokioDriver` argument rather than a field on the production context structs. The goal is no test-only fields in `RunContext`/`RunState`; the cost is that the suites assemble the host themselves, which is what `src/test_support/host.rs` already exists for.
  - The gap between a demotion candidate and its accessor is the deciding factor, not the type name: a type stays public while a public signature returns it.
  - Run measurements are recorded in this plan's Project Survey: the baseline test count before the first edit and the acceptance count at the end. The run ledger is scratch and never committed, so a count recorded only there cannot carry the ratchet across a resume, and the plan copy is a tracked file whose diff carries the number into the commit that measured it.
  - No step may end with an empty commit. A step whose likely outcome is no change - a measurement, a demotion whose candidates all fail their condition, a reread that finds nothing stale - records its outcome and the evidence for it in the plan copy, so its commit has content and the decision leaves a trace.
  - The rulebook's declined rules stay declined. The repository audited itself against the same rulebook (`vibe/2026-09/2026-09-11-1-rulebook-debt-tiers.md`) and recorded oversized-file splits, documentation-example convergence, import regrouping, test-layout consolidation, and the toolchain-pin policy as non-goals or deliberate counter-conventions. This plan applies only the two rulebook rules that survive that filter - a `#[cfg(test)]` re-export that exists only to feed test modules, and the field-destructuring fix for a conflicting borrow - and does not re-open the rest.
- Rejected alternatives:
  - Module paths only (keep `execute::`, drop root duplicates): not chosen by the user. Revisit if the root re-export list grows too long to scan.
  - Leaving the dual paths: rejected because it keeps the inconsistency. Revisit if crates outside this workspace start depending on the runtime and path stability outweighs simplicity.
  - A new public test-kit crate: it would need an amendment to the single-public-crate rule and to `build-xtask`, and most unit tests could not move anyway. About 85% of the test lines reach private items such as `RunState` (`src/execute/context.rs`), `TokioDriver` (`src/test_support/tokio_driver.rs`), and the scheduler hooks (`src/execute/scheduler/test_hooks.rs`). Revisit if crates outside the runtime need more than the current drivers.
  - Moving the `lua`, `model`, and `fanout` tests to the private crates: rejected because it would invert the dependency direction. There is no revisit condition.
  - Keeping the `RunHost` on the context behind `cfg(test)` and merely narrowing it: rejected because the field, its accessors, its five builders, and its `Debug` arm are the test-only production surface the item exists to delete; narrowing it would leave all of that.
- Assumptions, risks, and notes:
  - Paths are relative to the `promptforge` repository root. Line numbers and line counts were read on 2026-09-22 and may drift; every count below should be re-measured before it is relied on for a split or a ratchet.
  - The unit tests are already private (`#[cfg(test)] mod tests`). The only public test surface is `test_support`, compiled under `cfg(any(test, feature = "test-support"))` (`src/lib.rs` lines 13-14).
  - Tests that can move to the suite: `src/execute/tests/args_surface.rs` (324 lines), `src/execute/tests/lazy_prose.rs` (224 lines), and the offline cases in `src/execute/tests/exec_flow.rs` (about 1.5-2.2k lines). They depend only on `run_offline` from `src/execute/tests.rs`, which wraps the public `RunHost` and `run_with_host`. In the suite they use `tests/suite/support.rs` helpers instead.
  - Parts of `exec_flow.rs` that stay in-crate: the `engine::` unit tests near lines 1087-1110, the `advance_turn` test near lines 2253-2273, and the few tests that use the HTTP mock gateway.
  - Split seams for `src/execute/tests/scheduler.rs` (about 3978 lines; it still has content past line 3745, so the end of each range below is approximate, and the 2917-3080 range overlaps the previous range's end at 2920), by line range:
    - 1-1340: walk, call, jump, cancel, and depth.
    - 1343-2055: live H1.
    - 2060-2920: fanout mechanics.
    - 2917-3080: store-gate helpers.
    - 3089-end: fanout failure, cancel, and tool-arm cases.
    Five files is the subdirectory form, so the result is `tests/scheduler.rs` beside `tests/scheduler/{walk,live-h1,fanout,store-gate,failures}.rs` in standard layout, with plain `mod` declarations and no path attributes.
  - Split seams for `src/execute/tests.rs` (about 1.6k lines; the module list alone ends at line 1612, so re-measure), by line range: 1-570 context helpers, 578-800 fixture tools, and 810-1230 the scripted gateway. The extractions become plain module files under `src/execute/tests/`, which is already the directory those 37 module declarations from `tests.rs` live in; they are not `parent-label.rs` siblings, since that form is for one or two files.
  - This crate has no 500-line file ceiling: `build-xtask`'s marker check reads `src/lib.rs` (`crates/build-xtask/src/tidy.rs` lines 231-273), which here is only `#![doc = include_str!("lib.md")]`, and only `workshop-*` and `harness-*` crates are required to carry the marker. The splits are for readability only, so the results may still be over a thousand lines each; go finer only where a seam reads as two.
  - There are 17 matches for `incoming = Some` across 12 files in `src/execute/scheduler`, of which one is `answer_inline`'s own body in `waits.rs`. That leaves 16 hand-written sites across the other 11 files.
  - The `#[cfg(test)]` block at the end of `src/subst.rs` starts near line 408 and moves to `src/subst-tests.rs`, wired with `#[path]` as the crate's other `parent-tests.rs` files are.
  - Risk (medium): removing `test_host` changes how unit tests hand fixture tools to the tokio driver, and it reaches further than the two context structs: the `#[cfg(test)]` `RunContext` builders (`src/execute/config.rs` lines 351-397), the `Debug` arm that prints the field, and roughly fifteen fixture call sites across `src/execute/tests.rs`, `run-tests.rs`, and `tests/{effects,input,model_tasks}.rs`. The change is contained to test code and the two context structs.
  - Risk (low): the inline-answer hoist could change ready-queue order if a child push is moved. The ordering rule under Technical Design guards against it.
  - Risk (low): the docs gate, not clippy, is what proves the facade's intra-doc links; a link to the private `execute` module in `src/lib.md` is a `deny`-level rustdoc failure, so run the docs gate as soon as the facade item lands rather than at the end.

### Step 4 demotion notes

- Search: `git grep -n -E 'ModelBindings|SourceLocation|INPUT_UNAVAILABLE_FALLBACK'` over the workspace, read with `crates/promptforge-api-runtime/src` separated from every other compiled target (workspace crates plus this crate's own `tests/` and `benches/` targets, which compile as separate crates and see only the public API).
- `ModelBindings`: stays public. The type name appears outside the library only in dated `vibe/` notes, but its paired accessor `RunContext::model_bindings` is named by `crates/promptforge-api-runtime/tests/suite/prepare.rs` line 210 - an integration target that can only use the public API. Demoting the accessor would break that target, so the accessor stays; a type stays public while a public signature returns it, so the type and its root re-export stay too.
- `SourceLocation`: stays public with `RunError::location`. The only names are `crates/promptforge-api-runtime/src/execute/error.rs` (definition and accessor) and the `src/error.rs` unit tests, but a public signature exposes the type, and a trial demotion to `pub(crate)` fails `cargo clippy -D warnings` with `dead_code` for both the struct and `location` - the production build never calls them, so the accessor is host-facing rather than internal. Removing it would be a host-API removal with no in-crate consumer to justify it, and gating a documented accessor behind `cfg(test)` is a larger change than the condition licenses, so the pair stays public.
- `input::INPUT_UNAVAILABLE_FALLBACK`: demoted to `pub(crate)`. The only names are `src/execute/scheduler/apply.rs` and `src/execute/tests/input.rs`, both inside the library crate, and no public signature returns or accepts it. Its public doc links in `src/input.rs` and `src/test_support/host.rs` become plain code spans, because a public doc linking a private const fails the docs gate's `private_intra_doc_links`.

### Deferred and Out of Scope

- Deferred: collapse the twin transport error variants in `src/error.rs` (for example `Backend` and `BackendBodyRead`) and share one classification map between `RunErrorKind` and the Lua error kind. Revisit when `src/error.rs` next changes for a behavior reason.
- Deferred: unify `Chain.waiting_on` and `Chain.awaiting` in the scheduler. Revisit when wait semantics change.
- Deferred: rename `src/execute/engine.rs`, which holds only visible-set and jump-target resolution, to a name that fits its role. Revisit during the next walk refactor.
- Deferred: replace most scripted-gateway unit tests (axum plus reqwest) with Chat answers scripted through `Performers`. Revisit after `src/execute/tests.rs` is split.
- Deferred: split the long functions `prepare_chat` (scheduler `chat`), `deliver` (scheduler `waits`), `handle_coro_result` (scheduler `step`), and `pop_position` (scheduler `walk`). Revisit when each function next changes.
- Out of scope: merging `Error` and `RunError`.
- Out of scope: removing the thin crate-internal re-export modules `cancel`, `untrusted`, `store`, `tools`, and `lua`.
- Out of scope: adding a crate.
- Out of scope: adding structural checks.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (workspace `default-members = ["crates/gateway/app"]`, so a plain `cargo build` compiles only the gateway on a fresh clone); the desktop app is `cargo build -p workshop` (CI: `cargo build --locked -p gateway`, `cargo build --locked -p gateway --no-default-features`); in an automated or agent loop, `cargo check --message-format=json` emits the same diagnostics as a parseable stream.
- Focused test command pattern: one crate and one test, e.g. `cargo test --locked -p gateway-api-discovery a_process_lifetime_lease_recovers_after_its_owner_is_terminated` (nextest: `cargo nextest run -p <crate> -E 'test(<name>)'`).
- Component test command pattern: `cargo nextest run --locked -p <crate>` per component, e.g. `cargo nextest run --locked -p workshop-server --features headless`; add `--all-features` for `promptforge-api-runtime`, whose `suite` and `models_loop` targets are gated by `required-features = ["test-support"]`, so the bare pattern runs none of its integration tests.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then doctests `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`; the workshop partition runs separately as `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` plus `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`. Never run a standalone `cargo check --workspace` beside clippy (shared-artifact rule); the one exception is `cargo check -p gateway --no-default-features`; add `--message-format=json` when an agent consumes the output.
- Formatter check command: `cargo fmt --all --check`.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`; user guide: `mdbook build guide`.
- Test placement and naming conventions: unit tests sit inline in `#[cfg(test)]` modules under `src/`, including kebab sibling module files (`src/fanout-tests.rs`, `src/tools-tests.rs`) and `src/execute/tests.rs`. Integration tests live in each crate's top-level `tests/` tree (exempt from the flat-source rule), arranged as a `tests/suite/` harness declared in `tests/suite/main.rs` with fixtures such as `tests/prompts/`. Test names are descriptive snake_case sentences (`a_process_lifetime_lease_recovers_after_its_owner_is_terminated`).
- Directory map: `crates/` holds every Rust crate; the manifestless family containers `crates/promptforge/`, `crates/gateway/` (with nested `crates/gateway/stt/`), `crates/workshop/`, and `crates/harness/` sit beside the root public crates (`promptforge-api-runtime`, `promptforge-api-types`, `gateway-api-types`, `gateway-api-discovery`, `harness-api`, `shared-*`, `workspace-hack`), the `build-*` tooling crates, and `crates/shared-ui/` (a TypeScript+CSS package the Cargo member glob skips). Beside `crates/`: `guide/` (mdBook user guide), `prompts/` (sample prompt documents), `tools/` (Node `.mjs` scripts with `.test.mjs` siblings), `vibe/` (dated design notes and `archdoc.md`), `images/`, `local/`, `.github/` (workflows and fixtures), `.cargo/`, `.config/` (hakari and nextest), `.githooks/`, and root manifests (`Cargo.toml`, `rust-toolchain.toml`, `rustfmt.toml`, `clippy.toml`, `deny.toml`, `dist-workspace.toml`, `gateway.local.example.toml`, `AGENTS.md`, `README.md`, `LICENSE`).
- Component boundaries: one-way dependency flow `shell -> features -> services -> vocabulary`. Families: executor/`promptforge-*` (public surface `promptforge-api-runtime` + `promptforge-api-types`), `gateway-*` (public pair `gateway-api-types` + `gateway-api-discovery`), `harness-*` (public `harness-api`), `workshop-*` (the Tauri shell `workshop` depends on `workshop-server-api`, never `workshop-server`), and `shared-*` substrate (no product dependencies). Family containers are private: outside crates may name only the listed public crate, and `cargo test -p build-xtask` enforces the topology, the mandatory `## Invariants` marker, and the 500-line ceiling; the marker check reads `src/lib.rs` or `src/main.rs`, so the ceiling binds the crates that carry the marker and the `workshop-*`/`harness-*` families, not every crate.
- Conventions summary: Rust workspace, edition 2024, resolver 3, BSL-1.0, version 0.3.0, built on stable. Workspace lints forbid unsafe code, deny clippy `all` + `pedantic` and `unwrap_used`/`expect_used`, deny rustdoc `broken_intra_doc_links`/`private_intra_doc_links`, and warn `missing_docs`/`unreachable_pub`. Dependencies are centralised in `[workspace.dependencies]` with deliberate exact pins (for example turso `=0.7.2`). Source directories are flat by default: a subdirectory needs three files, or the group flattens to `foo-bar.rs` kebab siblings wired with `#[path = "..."]`. Behavior changes ship with tests in the same change; error and status messages are written for model consumption; run-log JSON must round-trip exactly. Lua is the embedded scripting language inside Markdown prompt documents, and the workshop UI is TypeScript and CSS colocated per feature using `--ws-*` tokens with no `localStorage`. Formatting lives in `rustfmt.toml`, lint tuning in `clippy.toml`, and supply-chain policy in `deny.toml`.
- Baseline test count: 3827 (cargo nextest list --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features, 2026-09-22)

</project-survey>
<execution-plan>

## Execution Instructions

Components, in dependency order:

- `baseline` - one piece, run before the first edit because the test-count ratchet compares every later step against it.
- `facade` - piece `hidden-re-exports` (independent), then piece `facade-and-demotions` (sequential: the outside-name check for a demotion only works once every import uses root paths). Placed first because it changes the public path set that later steps import and the docs describe.
- `internals` - piece `inline-answer` (independent), then piece `test-seams` (sequential: the trim settles the surface the new `RunHost` argument is drawn from, so it lands before the host removal). Placed after `facade` so its tests import root paths.
- `suite` - one piece, built sequentially: the moves need root paths, and the splits and the `subst` move follow the moves.
- `docs` - one piece, after `facade` so the retargeted links and the module map match the private module.
- `verification` - one piece, last; runs every exit criterion.

Each step is one commit holding its code and its tests.

<step-1>

### Step 1: Record the baseline test count [completed]

- Component: `baseline`
- Artifacts: this plan's Project Survey - append `- Baseline test count: <N> (<command>, <date>)`.
- Run `cargo nextest list --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` and record the count in that line. Use the exit criterion's flags so the `suite` and `models_loop` targets, gated by `required-features = ["test-support"]`, are in it. The plan copy is the record because the run ledger is scratch and is never committed, and the ratchet needs a number that survives a resume.
- Tests: none; the recorded count is the ratchet's reference.
- Verify: the appended line is present in the plan copy and names the count and the exact command.

</step-1>

<step-2>

### Step 2: Drop the hidden model re-exports [completed]

- Component: `facade`
- Artifacts: `src/model.rs` (lines 29-33) - remove the `#[doc(hidden)]` re-exports `Applied`, `SseScanner`, and `StreamAccumulator`. These three share the hidden block with `ChunkSource`, `ToolSchemaError`, `build_request_body`, `escape_controls`, `read_body_capped`, and `read_completion_stream`, so remove only those three lines and leave the rest. First confirm no crate outside `promptforge-api-runtime` names the three (a workspace-wide grep) and that `crates/harness/models/src/transport.rs` (lines 14-18) still names none of them; the plan asserts it, but a removal breaks any outside caller at compile time.
- Tests: the retained items are covered by the existing crate unit tests; no new tests.
- Verify: `cargo clippy -p promptforge-api-runtime --all-targets --all-features -- -D warnings` and `cargo nextest run -p promptforge-api-runtime --all-features`.

</step-2>

<step-3>

### Step 3: Make the crate root the one public facade [completed]

- Component: `facade`
- Artifacts: `src/lib.rs` - make `pub mod execute` a private `mod execute` and add a root re-export for every item that stays public inside `execute`; the compiler-derived set is authoritative (`unreachable_pub` reports every miss), and the representative set is `ModelBindings`, `ToolBindings`, `CapabilityConflict`, `RequirementCheck`, `Requirements`, `UnmetRequirement`, `AnswerRecord`, `ChatAnswerRecord`, `Effect`, `EffectAnswer`, `EffectId`, `EffectRecord`, `InputAnswerRecord`, `Run`, `Step`, `StoreAnswerRecord`, `ToolAnswerRecord`, `StoreOp`, `StoreOutcome`, `StoreError`, and the `perform_store_op` function (`src/execute.rs` line 135, a function, not a re-export); names already at the crate root need nothing added. Rewrite the `promptforge_api_runtime::execute::` imports in `crates/harness/runner/src/{effect_loop,effect_loop-answering,performers-host,performers,prepare}.rs`, `crates/harness/runner/tests/it/{prepare,support}.rs`, `crates/harness/capabilities/src/activation.rs`, `crates/harness/capabilities/tests/it/{activation,assembly,support}.rs`, `tests/suite/{execution,fanout,prepare,support,vfs}.rs`, and `benches/models_loop.rs`; the doctests in `src/execute/config.rs`, `src/execute/config-limits.rs`, and `src/execute/run.rs`; and the `execute` link and module description in `src/lib.md` line 3.
- Every `pub` item left inside `execute` is re-exported at the root or demoted, so the crate is clean under `unreachable_pub` and `private_intra_doc_links`. `ModelBindings` is re-exported at the root here, because the public `RunContext::model_bindings` returns it; Step 4 deletes that re-export only together with the accessor.
- Tests: the crate suite and the dependent suites pass unchanged; this is one commit because a half-converted crate fails clippy and rustdoc.
- Verify: crate clippy, `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge-api-runtime --no-deps --all-features`, and the harness runner and capabilities suites.

</step-3>

<step-4>

### Step 4: Apply the qualifying demotions [completed]

- Component: `facade`
- Artifacts: `ModelBindings` (paired accessor `RunContext::model_bindings`), `SourceLocation` (paired accessor `RunError::location`), and `input::INPUT_UNAVAILABLE_FALLBACK`. Demote each to `pub(crate)` only when no crate outside the runtime names it after the facade change and no public signature exposes it, demoting the paired accessor in the same change, and delete the root `ModelBindings` re-export if and only if the accessor goes with it; the rest stay public.
- Record each candidate's outcome and the search evidence behind it in this plan's Decision Record notes, so the commit carries the decision even when no candidate qualifies.
- Tests: a workspace-wide search for outside names, then the crate suite.
- Verify: crate clippy and the docs gate.

</step-4>

<step-5>

### Step 5: Hoist `answer_inline` [completed]

- Component: `internals`
- Artifacts: move `answer_inline` from `src/execute/scheduler/waits.rs` (lines 56-61) to the core `impl Scheduler` in `src/execute/scheduler.rs`, replacing the sixteen hand-written `incoming = Some(..)` plus `ready.push_back(..)` pairs in the `chat`, `timer`, `dispatch`, `tasks`, `await_tasks`, `task_events`, `apply`, `step`, `tool_call`, `notices`, and `chain` modules. Keep the spawned-child push after the helper in `src/execute/scheduler/tasks.rs` line 140 and `src/execute/scheduler/tool_call.rs` line 144, and convert `src/execute/scheduler/step.rs` lines 229-232 as well: where the chain borrow conflicts with `self.ready`, destructure the chain and the ready queue into separate locals first, so that site converts under the same ordering rule instead of being skipped.
- Tests: the scheduler unit tests assert the ready-queue order is unchanged.

</step-5>

<step-6>

### Step 6: Trim the public `test_support` surface [completed]

- Component: `internals`
- Artifacts: `src/test_support.rs` - limit the public surface to `drive`, `drive_tokio`, `Performers`, `Performer`, `BoxFuture`, `RunHost`, `run_host`, `run_with_host`, `ChatClient`, `DeltaHook`, `TestTool`, `TestToolTable`, `TestBroker`, `forward`, `recording::Observer`, and `recording::Observation`; delete `RecordingObserver` (`src/test_support/recording.rs` line 52) and the remaining `recording` items from the public surface (`forward_one`, `null_emitter`, `NullObserver`, `DebugCapture`, and `DebugEvent` are public today and must leave with it); narrow the module declarations, because `src/test_support.rs` currently exposes `pub mod host` (line 43), `pub mod recording` (line 47), `pub mod tokio_driver` (line 48), and `pub mod tools` (line 49), and a `pub` module keeps its items reachable by path, so each must become `pub(crate)` or `cfg(test)` unless the listed name needs it; keep `TokioDriver`, `MockGatewayClient` (which keeps its `#[path]` include), and `src/execute/scheduler/test_hooks.rs` under `cfg(test)`; rewrite the module docs (lines 10-28). Delete the `#[cfg(test)] pub(crate) use promptforge_parser::test_support::synthetic_section` re-export (lines 29-30) and switch its two consumers in `src/execute/tests/exec_flow.rs` to import `promptforge_parser::test_support::synthetic_section` directly, so no crate-internal re-export exists only to feed test modules.
- Tests: `tests/suite`, the in-crate suites, and the `benches/models_loop.rs` build.

</step-6>

<step-7>

### Step 7: Remove the test-only host from the context structs [completed]

- Component: `internals`
- Artifacts: drop `RunContext.test_host` and the `#[cfg(test)] impl RunContext` builders `observer`, `debug`, `client`, `input_broker`, and `on_delta` (`src/execute/config.rs` lines 121, 155, 351-397) with the `Debug` arm (lines 402-403); drop `RunState.test_host`, `test_host()`, and `set_test_host()` (`src/execute/context.rs` lines 77, 167, 186-203); change `TokioDriver` to `TokioDriver::new(state, host, client)` (`src/test_support/tokio_driver.rs` line 162) taking a `RunHost` assembled from `src/test_support/host.rs`; migrate the fixtures in `src/execute/tests.rs` (lines 433 and 287), `src/execute/run-tests.rs` (lines 270, 316, 344), `src/execute/tests/effects.rs` (lines 71-80, 175-185), `src/execute/tests/input.rs` (lines 164-166, 198-199, 254-256, 270-280, 290-300, 306-316, 333-343), and `src/execute/tests/model_tasks.rs` (lines 63-65); move `RunContext::debug`'s `report_debug = DebugMode::On` side effect to where debug capture is installed; keep the `tap` and `raw_shims` seams.
- Tests: the crate suite plus the harness runner and capabilities suites, since `TokioDriver` is drawn from the trimmed surface.

</step-7>

<step-8>

### Step 8: Move public-API-only tests into `tests/suite` [completed]

- Component: `suite`
- Artifacts: move `src/execute/tests/args_surface.rs`, `src/execute/tests/lazy_prose.rs`, and the offline cases of `src/execute/tests/exec_flow.rs` into `tests/suite`, swapping `run_offline` for the `tests/suite/support.rs` helpers and using root paths; this is a port, not a mechanical move - both files reach the private harness helpers `run`, `fixture`, `TestStore`, `silent`, and `run_offline` through `use super::*`, so each test's fixture setup is rebuilt on the suite helpers while its assertions stay unchanged; declare each new module in `tests/suite/main.rs`. Leave the `engine::` unit tests, the `advance_turn` test, the HTTP-mock-gateway cases, and the two tests that call `synthetic_section` (`list_from_section_ambiguous_error_is_loud` near line 1089 and `duplicate_top_level_section_names_error_loudly` near line 1104) in `src/execute/tests/exec_flow.rs`; those two drive engine walk internals, so an integration target cannot host them, and they take `synthetic_section` from `promptforge_parser::test_support` directly.
- Tests: `tests/suite` passes with the moved assertions unchanged.

</step-8>

<step-9>

### Step 9: Split the scheduler tests [completed]

- Component: `suite`
- Artifacts: split `src/execute/tests/scheduler.rs` along the five Decision Record seams into `src/execute/tests/scheduler/{walk,live-h1,fanout,store-gate,failures}.rs`, with `src/execute/tests/scheduler.rs` as the parent module of plain `mod` declarations and no `#[path]` attributes.
- Tests: the split modules carry the same tests and pass as before.

</step-9>

<step-10>

### Step 10: Split `src/execute/tests.rs` [completed]

- Component: `suite`
- Artifacts: extract `src/execute/tests.rs` along its seams into plain modules under `src/execute/tests/`: context helpers (lines 1-570), fixture tools (lines 578-800), and the scripted gateway (lines 810-1230), re-measuring the ranges first.
- Tests: the extracted modules carry the same tests and pass as before.

</step-10>

<step-11>

### Step 11: Move the `subst.rs` test block [completed]

- Component: `suite`
- Artifacts: move the `#[cfg(test)]` block at the end of `src/subst.rs` (near line 408) to `src/subst-tests.rs`, wired with `#[path = "subst-tests.rs"] mod tests;` as the crate's other `parent-tests.rs` siblings are.
- Tests: the moved tests pass unchanged.

</step-11>

<step-12>

### Step 12: Fix the documentation [completed]

- Component: `docs`
- Artifacts: `README.md` - remove the crates.io, docs.rs, and license badges (lines 3-5) and change the dependency snippet (lines 11-14) to `promptforge-api-runtime.workspace = true`. Rewrite `src/lua.rs` (lines 11-12) and `src/untrusted.rs` (lines 3-5) module docs as crate-internal import surfaces like `src/store.rs` and `src/tools.rs`. Replace the `src/execute.rs` module-layout paragraph (lines 64-87) with a one-line-per-child bullet map. Remove the legacy-path comments in `src/execute/scheduler.rs` and its `apply`, `dispatch`, `step`, and `walk` modules. Document the step/resume contract on `Run::step`, `Run::resume`, and `Run::cancel` in `src/execute/run.rs`. Update `crates/promptforge-api-runtime/AGENTS.md` where its historical-path rule, module description, or single-public-crate paragraph no longer matches the new facade.
- Tests: none; the docs gate in Step 13 covers this step.

</step-12>

<step-13>

### Step 13: Run the full gates

- Component: `verification`
- Artifacts: this plan's Project Survey - append `- Acceptance test count: <N> (<command>, <date>)` and one line per gate result. Run every exit criterion in Testing Plan - the workspace clippy command, the nextest run followed by the doctest run, `cargo doc` with `RUSTDOCFLAGS="-D warnings"`, `cargo fmt --all --check`, and `cargo test -p build-xtask` - and confirm the workspace test count from `cargo nextest list --all-features` is not lower than Step 1's recorded baseline; when it is lower, restore the missing coverage and rerun before recording.
- Tests: this step is the acceptance run, and the two recorded lines in the plan copy are its evidence.

</step-13>

</execution-plan>
