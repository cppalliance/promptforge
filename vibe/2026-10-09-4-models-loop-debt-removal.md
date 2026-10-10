---
name: Models loop debt removal
overview: "Remove the two debts the Debt Collector kept from the models-loop-in-rust work (b5ea5d9b4..2a8e9aedd): capture `error` in the coroutine shim so a rebound author global can no longer make the loop trampoline spin forever (C-1), and drop the two `ChatResult` fields nothing reads (D1-1). Fold in four cheap fixes the owner chose from the rejected candidates (D1-2, C-3, D1-15, D1-20)."
todos:
  - id: c1-capture-error
    content: "C-1 and D1-20: write the rebound-error regression test (fails as Interrupted), then capture error chunk-wide in __impl_coro.lua; delete the unread infer return key, the stale H1 comment, and the dead models and tools guards"
    status: pending
  - id: d1-1-chatresult-fields
    content: "D1-1, C-2, C-3, D1-15: remove ChatResult.model and .metrics, their four initializers in scheduler/chat.rs, and test literal lines; fix the stale loop-shim wording and the empty-reply doc line in the touched docs; keep one shared copy of the call() and round() test fixtures"
    status: pending
  - id: d1-2-messages-doc
    content: "D1-2: correct the messages.rs module doc that says the round reads only author-built records with no second validation"
    status: pending
  - id: exit-checks
    content: Run the AGENTS.md Verification list; facade surface check shows no diff
    status: pending
isProject: false
---

# Models loop debt removal

<product-contract>

## Product Requirements

- Scope and target work
  - Repository `c:\Users\Vinnie\cursor\promptforge`. The target is every commit whose `Plan:` trailer names `vibe/2026-10-09-3-models-loop-in-rust.md`: `9ca61463b`, `5a676d1cc`, `7da8ff2ee`, `c0a72484f`, `04266ff27`, `e49618385`, and `2a8e9aedd`, over the baseline `b5ea5d9b4`. The endpoint and disposition ref are both `2a8e9aedd`. The worktree was clean and excluded.
  - Evidence came from the target diffs, the endpoint tree, and the design record `vibe/2026-10-09-3-models-loop-in-rust.md`. An independent analysis and challenge pass produced it, and the main context reconciled the result.
  - Analysis limit: the guide commit `44ff253` lives in `c:\Users\Vinnie\cursor\promptforge-docs`, outside this repository. Only its marker commit `5a676d1cc` was analyzed.
- Cleanup goals and non-goals
  - Goal: the loop trampoline always terminates on a loop raise, even when author code has rebound the `error` global (C-1).
  - Goal: `ChatResult` carries only the fields the loop reads (D1-1).
  - Goal: four cheap fixes the owner chose from the rejected candidates, each in or beside a file the kept fixes already touch: two stale doc lines (D1-2, C-3), one duplicated pair of test fixtures (D1-15), and one unread return key with its stale comment (D1-20).
  - Non-goal: no change to the explicitly deferred work: the `tools.call_as_model` hook, the second copy of the local-tool handshake, or keeping bound-tool text out of Lua.
  - Non-goal: no new structural ratchet, no public API change, and no edit to architecture records or to the executed plan in `vibe/`.
- Success criteria
  - A block that rebinds `error` to a function that returns, then calls `pcall(models.loop, 42)`, gets the list error back promptly. Today the run spins until cancelled.
  - `ChatResult` no longer has `model` or `metrics` fields, and no code builds or reads them.
  - The `messages.rs` module doc and the `ChatResult` reply doc state what the code does; `call()` and `round()` each have one definition; the shim's return table no longer exports `infer`, and its install comment names no H1 base install.
  - Every check in the `## Verification` list of `AGENTS.md` passes, and the facade surface check shows no diff.

## Functional Specification

### Debt Inventory

- Debt added:
  - C-1, worsened, from `c0a72484f`:
    - Evidence: `drive` in `crates/promptforge-internal/lua/src/__impl_coro.lua` (lines 224 to 241 at `2a8e9aedd`) is a `while true do` loop whose raise branch is `error(a, 0)`, read as a global at call time.
    - The chunk captures `pcall`, `xpcall` (line 35), and `math.type` (line 39) at load, but not `error`. Authors can rebind `error`: the `_G` guard in `__impl_globals.lua` lets an assignment to an existing raw key go through as a plain table write.
    - When a rebound `error` returns instead of raising, `action` stays `"raise"` and the loop spins without yielding. The run stalls until its cancel flag trips the instruction hook.
    - Relationship to target work: the baseline Lua loop also read `error` as a global, but it was a `for` loop bounded by `max_tool_iterations`, so it always ended, at worst with a wrong nil. The target turned a bounded wrong result into non-termination. That contradicts the design record's parity claim and its Functional Specification line "Every raise happens in a Lua frame as `error(value, 0)`".
    - Impact: low likelihood, high impact. Any loop raise hangs the run: an argument error, `empty_model_reply`, `tool_loop_exhausted`, a failed tool answer, or `compactors.fail` exhaustion.
    - Reversal cost: none.
    - Target state: `error` is captured once at chunk load, and every shim raise uses the captured base function.
- Cheap fixes:
  - D1-1, from `e49618385`, with context from `c0a72484f`:
    - Evidence: `ChatResult` in `crates/promptforge-internal/lua/src/protocol/answer.rs` keeps `pub model: String` (line 130) and `pub metrics: Option<CallMetrics>` (line 132).
    - `e49618385` deleted `chat_result_table` in `protocol/render.rs`, the only code that read them. `Machine::judge` in `models_loop/machine.rs` reads only `overflow`, `overflow_reason`, `tool_calls`, `turn`, `reply`, `finish_reason`, and `empty_detail`.
    - A `git grep` at `2a8e9aedd` finds no reader of either field. No plan text or `Deferred:` trailer reserves them.
    - Every served round in `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs` still clones `served.model` and `served.metrics` into them, and about seven test literals must set them.
    - Impact: dead data, test upkeep, and a type shape that suggests the loop consumes serving metadata. No incorrect behavior.
    - Reversal cost: low. `ChatResult` is internal to the unpublished `promptforge-lua` crate and absent from `crates/promptforge/public-api.txt`.
    - Target state: both fields are gone. The round's events still carry the served model and metrics through `report_model_turn` and `assistant_tool_calls`, and the precheck anchor still reads `served.metrics` directly.
- Owner-added cheap fixes, chosen from the rejected candidates below after review:
  - D1-2, doc wording, from `04266ff27`: the module doc of `crates/promptforge-internal/lua/src/messages.rs` (line 15 at `2a8e9aedd`) says the round "reads the records the author built with no second validation". The list now also holds records the loop's state machine and the chat dispatch's notice push add through `MessageList::push`, which never pass `parse_record`; they are valid by construction. Target state: the doc says which records each edit validates and that Engine-built records are trusted by construction.
  - C-3, doc wording, pre-existing: the `reply` field doc of `ChatResult` in `protocol/answer.rs` says an empty reply is `reply` absent, never an empty string. A host-built completion can deliver `Some("")`, and `Machine::judge` reads an empty string as no reply, as it reads `None`. Target state: the doc says so.
  - D1-15, test upkeep, from `c0a72484f`: `call()` and `round()` are identical in `crates/promptforge-internal/lua/src/models_loop/machine-tests.rs` (lines 61 to 83) and `models_loop/tests.rs` (lines 23 to 45). D1-1 edits both copies. Target state: one definition of each.
  - D1-20, dead code, pre-existing: the shim chunk's return table in `__impl_coro.lua` exports `infer = infer` (line 348), which no Rust code reads, and the comment at lines 331 to 334 describes a "live H1 base install" that passes nil and "takes `infer` from the return", which no longer exists. The `if models then` and `if tools then` guards below that comment are dead: `install_shim_prelude` reads both globals as typed tables (`coro.rs` lines 181 and 182, `let models: Table = globals.raw_get("models")`), so a nil fails the install before the chunk runs. Target state: the key, the stale comment, and both guards are gone, and the guarded bodies run unconditionally.
- Exposed pre-existing debt: none.
- Rejected candidates: 22, four of which the owner chose to fix anyway (D1-2, C-3, D1-15, D1-20, above).
  - Residual-but-acceptable (14):
    - Explicitly deferred: D1-12, D1-13, D1-14.
    - Documented and tested couplings or design choices: D1-2, D1-5, D1-7, D1-8, D1-17, D1-18, D1-19.
    - Structural leads with no demonstrated consequence, where existing tests pin the contract: D1-3, D1-4, D1-15, D1-21.
  - Weak or speculative (3): D1-6, which is unreachable in production; D1-16, which repeats a sleep-then-cancel test pattern already in the repository; C-2, which is wording drift only.
  - False (3): D1-9, D1-10, D1-11. Coverage, model-client checks, or every producer already rule each out.
  - Unrelated pre-existing (2): D1-20 and C-3. Both are byte-identical at `b5ea5d9b4`.

</product-contract>
<implementation-contract>

## Technical Design

- C-1, in `crates/promptforge-internal/lua/src/__impl_coro.lua`:
  - Add one chunk-level capture of the base `error` function beside `local raw_pcall, raw_xpcall = pcall, xpcall` (line 35). Use the form the chunk's `math_type` capture and `__impl_globals.lua` line 17 already use, for example `local error = error`.
  - Give it a one-line comment in the style of the `math_type` capture: the shim's raises must not move when author code rebinds `error`.
  - The capture is chunk-wide, so `raise`, `fail`, `drive`, `run_local_tool`, and the other shim raises all use the base function. `drive` itself stays as it is.
- D1-1, in `crates/promptforge-internal/lua/src/protocol/answer.rs`:
  - Remove `model` and `metrics` from `ChatResult`, with their doc comments.
  - Remove any import, such as `CallMetrics`, that the removal leaves unused.
  - Leave every other field and the `impl mlua::UserData for ChatResult {}` as they are.
- D1-1, in `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`:
  - Drop the `model` and `metrics` initializers from the four `ChatResult` builders: `overflow_result`, the `EmptyReply` arm of `ArrivedRound::failed`, `ArrivedRound::text_reply`, and `ArrivedRound::tool_calls`.
  - `Served` keeps its `model` and `metrics`, because `assistant_tool_calls` and the precheck anchor in `accept_chat` read them.
- D1-1, test literals:
  - Drop the two field lines from every `ChatResult` literal the compiler names. These include `crates/promptforge-internal/lua/src/models_loop/machine-tests.rs` and `models_loop/tests.rs`: both `round()` fixtures, plus the `served` value in `a_chat_answer_resumes_as_a_chat_result_userdata_the_step_takes_whole`. They also include `crates/promptforge-internal/engine/src/lua/tests/quota.rs`, `crates/promptforge-internal/engine/src/lua/tests/shims.rs`, and `crates/promptforge-internal/engine/src/lua/tests/models_loop_contract.rs`, which is a different file from the C-1 test's `engine/src/execute/tests/models_loop_contract.rs`.
  - Struct-literal exhaustiveness flags every construction site, and `..round()` spreads follow the fixtures.
- C-2 wording, a reversible touch inside the D1-1 edits:
  - The D1-1 change already edits the `ChatResult` field docs in `answer.rs` and the `ArrivedRound::failed` doc in `scheduler/chat.rs`.
  - In those docs, replace wording that says the loop shim invokes the compactor, applies the exit rules, or alone knows the earlier rounds with the loop's state machine (`models_loop`). Make no other doc edits.
- C-3, in the same `answer.rs` edit: rewrite the `reply` field doc of `ChatResult` to say that an empty reply usually arrives as `None`, that a host-built completion can deliver an empty string, and that the loop reads both as no reply. No code change.
- D1-15, in `crates/promptforge-internal/lua/src/models_loop/`: keep one definition each of the `call()` and `round()` test fixtures, under `#[cfg(test)]`, in whichever of `machine-tests.rs` or `tests.rs` the other can reach with the narrowest visibility, and import it from the other. The directory already holds three files, so no layout rule changes. Make the move in the same edit that drops D1-1's fixture lines.
- D1-20, in `crates/promptforge-internal/lua/src/__impl_coro.lua`:
  - Delete the `infer = infer,` entry from the chunk's return table (line 348 at `2a8e9aedd`). Before deleting, confirm that `install_shim_prelude` and `stash_shims` in `coro.rs` read only named keys of the return table and never `infer`.
  - Delete the `if models then` and `if tools then` guards and run their bodies unconditionally. They are dead, because `install_shim_prelude` reads `models` and `tools` as typed tables (`coro.rs` lines 181 and 182), so a nil fails the install before the chunk runs.
  - Replace the stale comment above them (lines 331 to 334) with at most one line saying the install passes the VM's `models` and `tools` tables, or delete it if the code reads plainly without it.
- D1-2, in `crates/promptforge-internal/lua/src/messages.rs`: rewrite the module doc sentence at line 15, and line 5 if it reads as covering every record, so it says that author edits validate the records they add, and that records the Engine adds through `push` (the loop's state machine and the chat dispatch's notices) are valid by construction and not validated again. No code change.
- Lifecycle, failure, and security:
  - No protocol, persisted record, effect, event, or public facade item changes.
  - After C-1, an author who rebinds `error` gets the same raised values as one who does not, which is the parity the design record intended.

</implementation-contract>
<verification-contract>

## Testing Plan

- C-1 regression test, written first:
  - Location: a new case in `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`. If the file would pass the 500-line ceiling, use a flat peer registered beside it in `execute/tests.rs`.
  - Block: `error = function() end`, then `local ok, err = pcall(models.loop, 42)`, then `return tostring(err)`.
  - Run it with the sleep-then-cancel pattern of `a_local_tool_cancelled_mid_handler_reports_no_failed_tool_call` in `models_loop_contract-rounds.rs`, so the spin before the fix ends `Interrupted` instead of hanging the test.
  - Assert that the block returns the list error text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace`, and that the run is not interrupted.
  - It must fail before the change and pass after it. A pass before the change proves C-1 false: keep the test and skip the change.
- C-1 existing coverage: the contract raise cases and `act_names_each_call_return_and_raise_with_its_values` already drive the normal raise path through `drive`, so they catch a broken capture. The quota test's per-round instruction count does not change, because an upvalue read costs the same one instruction as a global read.
- D1-1 checks:
  - The build is the check: no `ChatResult` literal or reader names `model` or `metrics`.
  - These tests pass unchanged apart from the dropped lines: the machine tests, the adapter tests including `a_chat_answer_resumes_as_a_chat_result_userdata_the_step_takes_whole` (its `assert_eq!(*taken, served)` still compares every remaining field, including a call's `tool`), quota, shims, the contract tests, and the scheduler's event tests that report the served model and metrics.
- Owner-added fixes:
  - D1-15: a search of `models_loop/` finds exactly one `fn call(` and one `fn round(` fixture, and the machine and adapter tests pass.
  - D1-20: a search of `crates/` finds no reader of an `infer` key on the shim's return table; the lua crate's test VMs that install the shims still pass with the guards gone; the `infer` and handle-method tests in `engine/src/lua/tests/shims.rs` and `errors.rs` still pass.
  - D1-2 and C-3 are doc-only: `cargo doc` with `-D warnings` passes, and a reread confirms each sentence matches the code it describes.
- Focused runs: `cargo nextest run --locked -p promptforge-lua --lib models_loop protocol messages` and `cargo nextest run --locked -p promptforge-engine --lib models_loop lua::tests`.
- Exit checks, from `AGENTS.md` `## Verification`:
  - both nextest suites;
  - both clippy runs with `CARGO_BUILD_WARNINGS=deny`, and `cargo check -p gateway --no-default-features`;
  - `cargo fmt --all --check`;
  - both docs builds with `RUSTDOCFLAGS="-D warnings"`;
  - `cargo +nightly-2026-09-05 xtask api --check` with no diff;
  - `cargo test -p build-xtask`.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - C-1: capture `error` chunk-wide rather than only for `drive`.
    - Rejected alternative: a separate local used only by `drive`. It keeps the old behavior for the other shims, where a rebound `error` makes raises fall through, which is also wrong.
    - Tradeoff: author rebinding of `error` no longer affects any shim raise. That is the intended fix, and it matches `__impl_globals.lua`.
    - Verification: the regression test.
  - D1-1: remove the fields.
    - Rejected alternative: keep them and document a future consumer, because no plan names one.
    - Tradeoff: a later feature that wants the served model in the loop re-adds one field.
    - Verification: the build plus the existing tests.
  - C-2: correct the stale "loop shim" wording only in docs the D1-1 change already edits. Rejected alternative: a broader doc sweep, which this debt does not justify.
  - The C-1 test reuses the repository's sleep-then-cancel pattern rather than adding a timeout mechanism.
  - D1-20: delete the `if models then` and `if tools then` guards as dead, as the findings said. An earlier draft kept them on the belief that the lua crate's test VMs install the shims without a `models` global; Step 1's coder showed that false during the run (`install_shim_prelude` reads both as typed tables, and every caller injects both first), and the operator's run applied the correction.
  - D1-15: the shared fixtures live in whichever `models_loop/` test module needs the narrowest visibility. Rejected alternative: a fourth test-support file, which adds a module for about 20 lines.
- User-resolved choices:
  - No remediation changes a public interface, persisted or wire format, component ownership, dependency direction, or trust boundary, so no architecture choice needed the owner.
  - The owner chose, on 2026-10-09, to fold four rejected candidates into this plan as cheap fixes: D1-2, C-3, D1-15, and D1-20. The other rejected candidates stay rejected for the reasons in the Debt Inventory, including D1-3's loud default in `drive`, D1-6, D1-17's `raw_get`, and the deferred hook items.
- Rejected alternatives, assumptions, and risks:
  - D1-3's optional tag hardening in `drive` (match `"return"` and raise on unknown tags) is not needed, because `act_names_each_call_return_and_raise_with_its_values` and `act_yields_each_request_as_the_protocol_parses_it` pin every tag on both sides.
  - Assumption: the scripted first round finishes inside the 100 ms window, as the existing cancel tests assume.
  - Risk: none to persisted data, because `ChatResult` is never serialized or logged; it lives only between `into_envelope` and `take`.

### Deferred and Out of Scope

- D1-12, D1-13, D1-14, the Lua copies kept for the test hook: revisit when the `tools.call_as_model` hook is retired or rebuilt over the step, or if the two handshake copies drift.
- D1-6 and D1-17, call-time reads of author-mutable `tostring` and a `compactors` `__index`: revisit if a production path can reach either. Today the first is unreachable and the second only changes an error's shape.
- D1-16, the sleep-then-cancel timing pattern: revisit if any of these tests flakes.
- promptforge-docs commit `44ff253`, not inspected: revisit if the guide is audited against the code.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <crate>`, for example `cargo build --locked -p promptforge-engine`. Plain `cargo build` builds only the default member, `crates/gateway/app`. Every crate's `build.rs` runs `build_ceiling::check()`, so any build fails when a Rust file under `src`, `tests`, `benches`, or `examples` passes 500 lines. `AGENTS.md` forbids a standalone `cargo check --workspace` beside the clippy runs.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --lib <filter> [<filter> ...]`, where each filter matches a test path substring such as a module name. Examples: `cargo nextest run --locked -p promptforge-lua --lib models_loop protocol messages` and `cargo nextest run --locked -p promptforge-engine --lib models_loop lua::tests`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, for example `-p promptforge-lua` or `-p promptforge-engine`. The Workshop crates (`workshop`, `workshop-server`, `workshop-server-api`) run without `--all-features`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The other test gates are `cargo test -p build-xtask` (boundary and structural checks) and `cargo +nightly-2026-09-05 xtask api --check` (facade surface; the nightly is pinned in `crates/build-xtask/src/api/toolchain.rs` and is installed on this machine, as are `cargo-nextest`, `cargo-deny`, and `cargo-hakari`).
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, then the headless feature gate `cargo check -p gateway --no-default-features`. The shell is PowerShell, so set the variable first with `$env:CARGO_BUILD_WARNINGS='deny'`. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used`, `expect_used`, `allow_attributes`, and `allow_attributes_without_reason`.
- Formatter check command: `cargo fmt --all --check`, which is also the pre-commit hook.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc -p promptforge --no-deps`, both with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`). CI also runs `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`.
- Test placement and naming conventions:
  - Unit tests live in a sibling `<stem>-tests.rs`, wired at the bottom of `<stem>.rs` with `#[cfg(test)]`, `#[path = "<stem>-tests.rs"]`, and `mod tests;`. Examples: `coro.rs` with `coro-tests.rs`, `messages.rs` with `messages-tests.rs`, and `models_loop/machine.rs` with `models_loop/machine-tests.rs`. Extra files for one stem take a dash label, such as `messages-tests-list.rs` and `tools/tests-offering.rs`.
  - A module laid out as a directory keeps its tests at `<dir>/tests.rs`: `models_loop.rs` declares `#[cfg(test)] mod tests;`, which resolves to `models_loop/tests.rs`.
  - Larger suites use a `tests/` subdirectory of topic files declared from a `tests.rs` hub. `engine/src/execute/tests.rs` declares `mod models_loop_contract;` and its peers. `engine/src/lua/tests.rs` declares `coroutine`, `errors`, `globals`, `models_loop_contract`, `quota`, and `shims`. The lua crate has `src/tests/` and `src/protocol/tests/`. A topic's overflow moves to a dash-labeled peer wired with `#[path]`, as `models_loop_contract.rs` does with `#[path = "models_loop_contract-rounds.rs"] mod rounds;`.
  - Shared test support lives in `crates/promptforge-internal/engine/src/test_support/` (scripted chat, tokio driver, recording) and `execute/tests/fixtures.rs`. The lua and parser crates expose a `test-support` feature that the engine turns on in its dev-dependencies.
  - Test names are full snake_case sentences that state the behavior, such as `act_names_each_call_return_and_raise_with_its_values`. Synchronous tests use `#[test]`; Engine run tests are async functions under `#[tokio::test(flavor = "current_thread")]`.
  - Prompt fixtures for parsing and execution sit under `crates/promptforge-internal/engine/tests/prompts/` in `valid/`, `invalid/`, and `execution/`.
  - Test files obey the same 500-line ceiling as source files.
- Directory map:
  - `crates/`: every Rust crate plus the TypeScript packages.
    - Flat top-level crates: the facade `promptforge`, `promptforge-plugin`, `harness`, `harness-gateway-client`, `plugin-web`, `plugin-mcp`, `plugin-user-input`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `workspace-hack` (cargo-hakari), and the tool crates `build-xtask`, `build-ceiling`, `build-ui`, `build-workshop`, `build-user-guide`, and `build-llama-cuda`.
    - Manifestless containers hold each family's private crates: `promptforge-internal/` (`engine`, `lua`, `parser`, `types`, `vfs`, `model-client`), `harness-internal/runner`, `gateway/` (`app` plus its subsystems, with speech-to-text nested under `gateway/stt/`), and `workshop/` (the desktop app, server, UI, and support crates). The root `Cargo.toml` lists these members explicitly.
    - `crates/shared-ui` is a TypeScript and CSS package, not a Rust crate.
  - `crates/promptforge-internal/lua/src/`: the Lua layer. It holds the embedded Lua shim chunks `__impl_*.lua` (including `__impl_coro.lua` and `__impl_globals.lua`), the Lua-to-Engine protocol in `protocol/` (with `answer.rs`, `parse.rs`, `render.rs`, `request.rs`), the `models.loop` adapter `models_loop.rs` and its state machine in `models_loop/`, message lists in `messages.rs`, and the shim installer in `coro.rs`.
  - `crates/promptforge-internal/engine/src/`: the Engine's run stepper in `execute/` (with `scheduler/` and `run/`), the Engine-level Lua tests in `lua/tests/`, and `test_support/`.
  - npm workspaces: `crates/workshop/` (packages `ui`, `look`, `platform`) and `crates/gateway/config-ui/ui`.
  - `guide/`: user guide site sources (`books`, `chrome`, `landing`). `prompts/`: sample prompts. `images/`: README art. `tools/`: Node scripts for gateway sidecar staging and live TTS checks, each with a `.test.mjs`.
  - `vibe/`: dated design records and executed plans named `YYYY-MM-DD-N-slug.md`, with older months in `YYYY-MM/`. `local/` (gitignored, untracked) holds machine-local gateway config, profiles, and fixtures. `cabinet/` is local staging.
  - Config: `.cargo/config.toml` (rust-lld linker and static CRT on Windows MSVC; `cargo xtask` and `cargo workshop` aliases), `.config/nextest.toml` (60-second slow timeout, a capped `heavy` group for the speech-to-text suites), `.config/hakari.toml`, `.githooks/` (pre-commit runs fmt; pre-push runs the headless gateway check, the main clippy run, and `cargo deny`), `.github/workflows/ci.yml` (the gates), `clippy.toml`, `deny.toml`, `rustfmt.toml`, and `rust-toolchain.toml` (stable).
- Component boundaries:
  - Engine: `promptforge` is the public facade over `promptforge-engine`, `promptforge-lua`, `promptforge-parser`, `promptforge-types`, `promptforge-vfs`, and `promptforge-model-client`. Inside the family, `engine` depends on `lua`, `parser`, `model-client`, `types`, and `vfs`; `parser` depends on `lua` and `types`; `lua` depends on `model-client`, `types`, and `vfs`; `model-client` depends on `types`; `types` and `vfs` depend on no family crate. No Engine crate depends on a Harness crate.
  - Plugin API: `promptforge-plugin` depends only on `promptforge-types` and `promptforge-vfs`, and each `plugin-*` crate depends on `promptforge-plugin`.
  - Harness: `harness` depends on `harness-runner`, `promptforge`, and `promptforge-plugin`; `harness-runner` depends on `promptforge` and `promptforge-plugin`; `harness-gateway-client` depends on `harness`, `promptforge`, and `plugin-web`.
  - Host: `workshop-server` depends on the Harness crates, the facade, the Plugins, and the `workshop-*` crates.
  - Gateway: `gateway` (`crates/gateway/app`) depends on its own `gateway-*` family plus `gateway-api-types`, `gateway-api-discovery`, and `shared-loopback`, and on no Engine or Harness crate.
  - Enforcement: `cargo test -p build-xtask` checks the product and container boundaries and the Workshop tier graph, and `xtask api --check` pins the facade surface to `crates/promptforge/public-api.txt`.
  - For this plan: `ChatResult` is defined in `promptforge-lua` (`protocol/answer.rs`), built in `promptforge-engine` (`execute/scheduler/chat.rs`), and read by `models_loop/machine.rs`. It does not appear in `public-api.txt`.
- Conventions summary:
  - Four capitalized terms (Engine, Harness, Host, Plugin) each carry one meaning. Engine crates call whatever steps a run "the caller" and never mention the Host. `crates/workshop/ui/test/docs-claims.mjs` enforces this in docs, `AGENTS.md` files, and rules.
  - Source directories are flat: a subdirectory needs at least three files, otherwise the files are `foo-bar.rs` siblings wired with `#[path = "foo-bar.rs"] mod bar;`. A group on the wrong side of the line converts when touched.
  - No Rust file passes 500 lines. A split goes into private child modules that each open with a `//!` line naming their one concern, and never widens anything to `pub(crate)` or `pub` to compile.
  - Lints: `missing_docs`, `unreachable_pub`, and `missing_debug_implementations` warn, and the gates deny warnings. `unsafe_code` is denied outside its owned boundary. Exceptions use `#[expect(..., reason = "...")]`, never `#[allow]`.
  - Comments explain only a non-obvious constraint, ordering requirement, or workaround, and every workaround cites its upstream issue URL.
  - Error and status messages are written for model consumption: concise, self-contained, and naming required versus actual.
  - Behavior changes ship with tests in the same change, and refactors keep the existing behavior tests.
  - JSON that reaches a recorder or replay round-trips exactly (`serde_json` with `float_roundtrip`, sorted keys).
  - The Lua shim chunks capture base functions as chunk-level locals at load, such as `local raw_pcall, raw_xpcall = pcall, xpcall` (line 35 of `__impl_coro.lua`) and `local math_type = math.type` (line 39), so author code cannot change shim behavior by rebinding a global.
  - Commits end with `Design:` lines and a `Plan:` trailer naming the `vibe/` record they execute.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Capture error in the coroutine shim and drop its unread infer key [completed]

- Component: none
- Covers: C-1 and D1-20, which carry the plan's only behavior change and its regression test. Frontmatter todo: `c1-capture-error`.
- Repository: `c:\Users\Vinnie\cursor\promptforge`, at `2a8e9aedd` with a clean worktree. Every line number below is at `2a8e9aedd`.
- Artifacts:
  - `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`: one new `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` case with a full-sentence snake_case name, the same runtime as its pattern, so the timed cancel can fire while the unfixed shim spins on the other worker; built on the sleep-then-cancel pattern of `a_local_tool_cancelled_mid_handler_reports_no_failed_tool_call` in the peer `models_loop_contract-rounds.rs`. The file is 357 lines, so the case fits under the 500-line ceiling; the flat-peer fallback below applies only if it would not.
  - `crates/promptforge-internal/lua/src/__impl_coro.lua`:
    - A chunk-level capture of the base `error` function beside `local raw_pcall, raw_xpcall = pcall, xpcall` (line 35), in the form of `local math_type = math.type` (line 39) and `__impl_globals.lua` line 17, for example `local error = error`. Give it a one-line comment in the style of the `math_type` capture: the shim's raises must not move when author code rebinds `error`.
    - The capture is chunk-wide, so `raise`, `fail`, `drive`, `run_local_tool`, and the other shim raises all use the base function. Leave `drive` (lines 224 to 241) unchanged, and do not add D1-3's tag hardening to it.
    - Delete `infer = infer,` from the chunk's return table (line 348). Keep `loop`, `call`, `helpers`, and `handle_methods`, including the handle's own `infer` entry.
    - Delete the `if models then` and `if tools then` guards (lines 335 onward) and run their bodies unconditionally. They are dead: `install_shim_prelude` reads `models` and `tools` as typed tables (`coro.rs` lines 181 and 182, `let models: Table = globals.raw_get("models")`), so a nil fails the install before the chunk runs. Replace the stale comment above them (lines 331 to 334) with at most one line saying the install passes the VM's `models` and `tools` tables, or delete it if the code reads plainly without it.
  - `crates/promptforge-internal/lua/src/coro.rs`, read only: before deleting the key, confirm that `install_shim_prelude` and `stash_shims` read only named keys of the chunk's return table and never `infer`.
- Order:
  1. Write the regression test and run it against the unchanged shim. It must end `Interrupted`.
  2. If it passes instead, C-1 is false: keep the test, skip the `error` capture, make only the D1-20 edits, and report it.
  3. Otherwise make all the `__impl_coro.lua` edits, then run the checks below.
- Testing Plan checks this step owns, restated verbatim:
  - C-1 regression test, written first:
    - Location: a new case in `crates/promptforge-internal/engine/src/execute/tests/models_loop_contract.rs`. If the file would pass the 500-line ceiling, use a flat peer registered beside it in `execute/tests.rs`.
    - Block: `error = function() end`, then `local ok, err = pcall(models.loop, 42)`, then `return tostring(err)`.
    - Run it with the sleep-then-cancel pattern of `a_local_tool_cancelled_mid_handler_reports_no_failed_tool_call` in `models_loop_contract-rounds.rs`, so the spin before the fix ends `Interrupted` instead of hanging the test.
    - Assert that the block returns the list error text, `models.loop needs a messages.new() list; build one with messages.new() and :user, :append, or :replace`, and that the run is not interrupted.
    - It must fail before the change and pass after it. A pass before the change proves C-1 false: keep the test and skip the change.
  - C-1 existing coverage: the contract raise cases and `act_names_each_call_return_and_raise_with_its_values` already drive the normal raise path through `drive`, so they catch a broken capture. The quota test's per-round instruction count does not change, because an upvalue read costs the same one instruction as a global read.
  - D1-20: a search of `crates/` finds no reader of an `infer` key on the shim's return table; the lua crate's test VMs that install the shims still pass with the guards gone; the `infer` and handle-method tests in `engine/src/lua/tests/shims.rs` and `errors.rs` still pass.
  - Focused runs: `cargo nextest run --locked -p promptforge-lua --lib models_loop protocol messages` and `cargo nextest run --locked -p promptforge-engine --lib models_loop lua::tests`.
- Out of bounds: the `tools.call_as_model` hook, the second copy of the local-tool handshake, bound-tool text in Lua, any new structural ratchet, any public API change, architecture records, and `vibe/2026-10-09-3-models-loop-in-rust.md`.
- Commit: one commit holding the test and the shim edits, with `Design:` lines and a `Plan:` trailer per the Project Survey conventions. The pre-commit hook runs `cargo fmt --all --check`.

</step-1>

<step-2>

### Step 2: Remove the unread ChatResult fields, fix stale docs, and run exit checks

- Component: none
- Covers: D1-1 with C-2, C-3, D1-15, and D1-2, then the plan's exit checks. Changes no behavior and touches no file Step 1 edits. Frontmatter todos: `d1-1-chatresult-fields`, `d1-2-messages-doc`, and `exit-checks`.
- Repository: `c:\Users\Vinnie\cursor\promptforge`, with Step 1's commit on top of `2a8e9aedd`. Line numbers below are at `2a8e9aedd`.
- Artifacts:
  - `crates/promptforge-internal/lua/src/protocol/answer.rs`:
    - Remove `ChatResult::model` (line 130) and `ChatResult::metrics` (line 132) with their doc comments, and any import the removal leaves unused, such as `CallMetrics`. Leave every other field and `impl mlua::UserData for ChatResult {}` as they are.
    - C-3: rewrite the `reply` field doc to say an empty reply usually arrives as `None`, a host-built completion can deliver an empty string, and the loop reads both as no reply. No code change.
    - C-2: in the `ChatResult` field docs this edit touches, replace wording that says the loop shim invokes the compactor, applies the exit rules, or alone knows the earlier rounds with the loop's state machine (`models_loop`). No other doc edits.
  - `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`:
    - Drop the `model` and `metrics` initializers from the four `ChatResult` builders: `overflow_result`, the `EmptyReply` arm of `ArrivedRound::failed`, `ArrivedRound::text_reply`, and `ArrivedRound::tool_calls`.
    - `Served` keeps its `model` and `metrics`, because `assistant_tool_calls` and the precheck anchor in `accept_chat` read them, and the round's events still carry them through `report_model_turn`.
    - C-2: apply the same loop-shim wording fix to the `ArrivedRound::failed` doc.
  - `crates/promptforge-internal/lua/src/models_loop/tests.rs` and `models_loop/machine-tests.rs` (D1-1 with D1-15):
    - Drop the two field lines from both `round()` fixtures and from the `served` value in `a_chat_answer_resumes_as_a_chat_result_userdata_the_step_takes_whole` in `tests.rs`.
    - Keep one `call()` and one `round()`, under `#[cfg(test)]`. `tests.rs` is `models_loop::tests`, which `machine-tests.rs` (`models_loop::machine::tests`) can already reach as a descendant of `models_loop`. So keep the fixtures in `tests.rs` as `pub(super)`, delete the copies at `machine-tests.rs` lines 61 to 83, and import them there. The reverse placement would also need `mod tests` in `machine.rs` widened. Add no fourth test-support file.
    - Remove imports the edits leave unused, such as `ToolCallEvent` in `machine-tests.rs` or `CallMetrics` and `ClientTiming` in `tests.rs`, if nothing else in the file uses them.
  - `crates/promptforge-internal/engine/src/lua/tests/quota.rs`, `engine/src/lua/tests/shims.rs`, and `engine/src/lua/tests/models_loop_contract.rs`, which is a different file from Step 1's `engine/src/execute/tests/models_loop_contract.rs`: drop the two field lines from every `ChatResult` literal the compiler names. Struct-literal exhaustiveness flags every site, and `..round()` spreads follow the fixtures.
  - `crates/promptforge-internal/lua/src/messages.rs` (D1-2): rewrite the module doc sentence at line 15, and line 5 if it reads as covering every record. It says author edits validate the records they add, and records the Engine adds through `MessageList::push` (the loop's state machine and the chat dispatch's notices) are valid by construction and not validated again. No code change.
- Order:
  1. Make the edits and get both focused runs passing.
  2. Run the exit checks on the tree with Step 1 committed and these edits in place, so any fix lands in this commit and the plan stays at two commits.
  3. Commit.
- Testing Plan checks this step owns, restated verbatim:
  - D1-1 checks:
    - The build is the check: no `ChatResult` literal or reader names `model` or `metrics`.
    - These tests pass unchanged apart from the dropped lines: the machine tests, the adapter tests including `a_chat_answer_resumes_as_a_chat_result_userdata_the_step_takes_whole` (its `assert_eq!(*taken, served)` still compares every remaining field, including a call's `tool`), quota, shims, the contract tests, and the scheduler's event tests that report the served model and metrics.
  - D1-15: a search of `models_loop/` finds exactly one `fn call(` and one `fn round(` fixture, and the machine and adapter tests pass.
  - D1-2 and C-3 are doc-only: `cargo doc` with `-D warnings` passes, and a reread confirms each sentence matches the code it describes.
  - Focused runs: `cargo nextest run --locked -p promptforge-lua --lib models_loop protocol messages` and `cargo nextest run --locked -p promptforge-engine --lib models_loop lua::tests`.
  - Exit checks, from `AGENTS.md` `## Verification`:
    - both nextest suites;
    - both clippy runs with `CARGO_BUILD_WARNINGS=deny`, and `cargo check -p gateway --no-default-features`;
    - `cargo fmt --all --check`;
    - both docs builds with `RUSTDOCFLAGS="-D warnings"`;
    - `cargo +nightly-2026-09-05 xtask api --check` with no diff;
    - `cargo test -p build-xtask`.
- D1-2 reread: check the new sentence against `MessageList::push` and the list's builders.
- Out of bounds: the `tools.call_as_model` hook, the second copy of the local-tool handshake, bound-tool text in Lua, any new structural ratchet, any public API change (`ChatResult` is absent from `crates/promptforge/public-api.txt`), architecture records, and `vibe/2026-10-09-3-models-loop-in-rust.md`.
- Commit: one commit holding every edit above, with `Design:` lines and a `Plan:` trailer per the Project Survey conventions.

</step-2>

</execution-plan>