---
name: Debt removal plan
overview: "Remove the debt the Debt Collector kept from promptforge's 19 target commits (upstream/master to HEAD): drop the transcript \"Search with Google\" row entirely (D1-2), stop a late post-Stop reasoning chunk from rendering as streaming (D1-9), and guard the runtime service key with a server test plus a recorded revisit trigger (D1-1, option A)."
todos:
  - id: d1-2-remove-search
    content: "D1-2: update agent-session-view.mjs expectations first (confirm failure), then delete the Search with Google menu item and searchWithGoogle from transcript-view.ts; grep confirms no remaining references under crates"
    status: pending
  - id: d1-9-stopped-flag
    content: "D1-9: add the failing late-reasoning-after-Stop case to agent-session-service.mjs, then add the private stopped flag (set in cancelTurn; cleared in respond, onInputRequired, acknowledge) and open the late thought already ended in foldDelta; check whether a late text delta reproduces the same stuck row"
    status: pending
  - id: d1-1-runtime-key-guard
    content: "D1-1 option A: add the server test installing a valid remote MCP entry (closed loopback URL) through host_context and asserting the refusal names the handshake failure, not promptforge/tokio-runtime; run the fault-injection check on the server's RUNTIME key and revert it"
    status: pending
isProject: false
---

# Debt Removal: MCP Client Plugin and Copy Cursor Everywhere

<product-contract>

## Product Requirements

- Scope and target work
  - Repository: `promptforge` (`c:\Users\Vinnie\cursor\promptforge`), branch `master`, clean worktree.
  - Baseline: `fb8f85cc178d3b15631950a4ff85ec8ed418dc94` (`upstream/master`). Endpoint: `7f4582548cece6a00d0b5d9499c70f9a8166366f` (`HEAD`). 19 commits, 287 files, +29,260 / -3,775, no merges.
  - Plan `vibe/2026-10-08-1-mcp-client-plugin.md`: commits `20aace8ee`, `e37308797`, `1004599c6`, `c74899f70`, `c9ed649bf`, `020292701`, plus the unplanned follow-up `a1201da73` (drops an unused `serde` dependency from `plugin-mcp`).
  - Plan `vibe/2026-10-08-2-copy-cursor-everywhere.md`: commits `65da9448e`, `ed187216c`, `c1b9bc97e`, `57c67ca18`, `87a6b52c8`, `24777abe9`, `a948b6392`, `d8ce41c51`, `59411a4c7`, `faeb3a98a`, `7f4582548`, plus the unplanned `475b96214` (replaces the workshop-ui rules with a copy-Cursor rule).
- Cleanup goals and non-goals
  - Goals: remove the "Search with Google" row from the transcript context menu everywhere (D1-2); stop a late reasoning chunk after Stop from leaving a thought streaming (D1-9); guard the runtime service key the Plugin contract names as a bare string, and record when to revisit it (D1-1).
  - Non-goals: no change to the Plugin contract's public interface, no typed or contract-owned runtime key, no new `HostServices` method, no ratchet or source-shape check, no edit to the two closed plan records, no work on any rejected candidate.
- Success criteria
  - No Workshop UI source or test names "Search with Google", `searchWithGoogle`, or builds a `google.com/search` URL. The transcript context menu offers only Copy or Copy Message (when there is something to copy) and Select All.
  - After a Stop, a first reasoning chunk for a round the UI had not seen renders as an ended thought, not a streaming one, and the next turn's first reasoning chunk streams normally.
  - A server test installs a valid remote MCP entry through the production `host_context` path and proves the runtime key the server provides is the one `plugin-mcp` reads.
  - Every touched test passes, and each behavior fix has a test that failed before the change.

## Functional Specification

### Debt Inventory

- Debt added (introduced and worsened)
  - D1-1, introduced, prospective debt on a public interface. Commits `20aace8ee` (names the service in the Plugin contract) and `1004599c6` (the MCP Plugin declares its own key). Baseline: one typed `pub const TOKIO_RUNTIME: ServiceKey<Handle>` in `plugin-web`, imported by the Workshop server. Endpoint: the contract crate owns only `pub const TOKIO_RUNTIME: &str` (`crates/promptforge-plugin/src/service.rs` line 130), and three production crates each build their own `ServiceKey<Handle>` from it: `crates/plugin-web/src/web.rs` line 40, `crates/plugin-mcp/src/lib.rs` line 91, `crates/workshop/server/src/agents.rs` lines 74-75. Four test files hold further copies. The contract's own `ServiceKey` doc (service.rs lines 74-79) says a service's key is declared once as a const, and this one service breaks that rule by design. Impact: a drifted handle type in any crate makes every tokio-based Plugin read as unavailable with "this host provides none" (inference, no failure exists). Reversal cost: removing the public constant is a public-interface change on the Plugin contract, which cannot name `tokio` (Engine manifest guard, `crates/promptforge-plugin/src/lib.rs` lines 20-26), and a shared typed key crate is blocked by `crates/build-xtask/src/product.rs` (Plugin crates may name only `promptforge-plugin`, `shared-*`, and outside libraries). Cost grows with each new tokio-based Plugin. Target state: debt stays by operator decision, guarded by one server test and a revisit trigger. Severity low.
- Cheap fixes, listed apart from debt added
  - D1-2: the transcript context menu's "Search with Google" row does nothing outside a message row, and sends a whole message to a third party as one URL otherwise. New in `c1b9bc97e`. Target state: the row and its function are deleted.
  - D1-9: a first reasoning chunk that arrives after Stop opens a pending thought with no end, so `thoughtStep` reports `streaming: true` under an idle composer. Cause: `cancelTurn` ends only already-open thinking and `foldDelta` has no post-cancel guard. The late-chunk window is documented in `crates/workshop/server/src/agents/wire.rs` lines 12-17. The clock fields come from `ed187216c`. Target state: the late thought opens already ended.
- Exposed pre-existing debt: none in scope. C-5 (the server's runtime service is left out when no runtime is current, already true at the baseline) is unrelated pre-existing and is not planned.
- Rejected candidates: 20 in total.
  - 10 residual-but-acceptable: D1-3 (every run waits for every built Plugin to be ready; operator-chosen, bounded by each Plugin), D1-4 (a ready server never leaves ready and a failed one is never retried; stated in the plan, restarts deferred with a trigger), D1-5 (a bad `mcp.json` is only logged; status UI out of scope), D1-6 (a stored Render Whitespace `false` reads as "selection"; the value is ambiguous), D1-7 (two hand-synchronized token sheets; deliberate fork, pinned per sheet), D1-10 (About build date reserved by a `Deferred:` trailer), D1-11 (shared-ui additions the gateway does not read; reserved by the plan), D1-12 (Hugging Face key saves on blur; stated in the plan), D1-16 (Select All Occurrences shows a chord the chat pane wins; pinned by tests), D1-18 (`prepare` is a pass-through used by seven integration test files).
  - 2 weak/speculative: D1-8 (several CSS readers in test code, no wrong pass or fail shown), C-1 (`getZoomLevel` read only by tests).
  - 7 false: D1-13, D1-14, D1-15, D1-17, C-2, C-3, C-4.
  - 1 unrelated pre-existing: C-5.

</product-contract>
<implementation-contract>

## Technical Design

- D1-2 (UI only, no interface change). In [crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts](promptforge/crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts), `openContextMenu` (around lines 234-270) keeps the Copy item guarded by `subject !== ""` and Select All, and drops the "Search with Google" item. Delete the module-level `searchWithGoogle` function (around lines 273-288) with its doc comment. Nothing else in the file uses it. The view no longer builds an anchor or hands a navigation to the desktop shell.
- D1-9 (UI service only). In [crates/workshop/ui/src/services/agent-session.ts](promptforge/crates/workshop/ui/src/services/agent-session.ts) add a private boolean `stopped`, initially false.
  - Set it true in `cancelTurn` after `wire.cancelTurn()` succeeds.
  - Clear it in `respond` after a successful send (the turn's work restarts), in the `wire.onInputRequired` handler (the relaunched agent is waiting, so the grace window is over), and in `acknowledge` (a session acknowledgment, including reattach).
  - In `foldDelta`, when no pending item matches and the delta kind is `reasoning`, push the item with `startedAt: null` and `endedAt: this.stopped ? this.now() : null` (and `startedAt: this.now()` as before when not stopped). `thoughtStep` in `transcript-model.ts` (line 387, `streaming: item.pending && item.endedAt === null`) then reads false, and `durationMs` is null because `startedAt` is null. The durable `agent_thought` handler already reads `pending?.startedAt` and `pending?.endedAt`, so it inherits the ended item.
  - Unchanged: `transcript-model.ts`, the wire, the server, the `settled` cut-off, and deltas that append to an existing item.
- D1-1 (test only, no production change). In [crates/workshop/server/src/agents/tests.rs](promptforge/crates/workshop/server/src/agents/tests.rs) add one `#[tokio::test]` that builds a host with `with_plugins(services(&Registry::new()), vec![("docs".to_owned(), json!({"url": <closed loopback URL>}))])`, runs `run_prompt(&Arc::new(host), requiring("docs"))`, and asserts on the refusal. The test file already has `with_plugins`, `services`, `run_prompt`, `requiring`, `RunOutcome`, `json!`, and the `TOKIO_RUNTIME` import.
  - The URL comes from binding `std::net::TcpListener::bind(("127.0.0.1", 0))`, reading the port, and dropping the listener, so the connection is refused at once. `entry::parse` only requires an `http://` or `https://` prefix (`crates/plugin-mcp/src/entry.rs` lines 101-106).
  - The `docs` server constructs (so it reads the runtime through `plugin-mcp`'s own key, `crates/plugin-mcp/src/lib.rs` lines 90-109), its connection task fails fast, and `Server::ready` reports the failure (`connect.rs` and `server.rs` lines 200-212). The run is then refused naming the handshake failure.
  - Assert the refusal message contains "the MCP handshake failed" (proof the Plugin got past the runtime read) and does not contain `promptforge/tokio-runtime`. Confirm the exact refusal wording against `crates/harness-internal/runner/tests/it/prepare-ready.rs` before fixing the assertion text. `run_prompt` already bounds the run at 10 seconds.
  - Record the revisit trigger under Deferred and Out of Scope in this plan.

</implementation-contract>
<verification-contract>

## Testing Plan

- D1-2, regression. In [crates/workshop/ui/test/agent-session-view.mjs](promptforge/crates/workshop/ui/test/agent-session-view.mjs) the menu label lists pin the old behavior, so changing them first makes the file fail before the source change.
  - Line 13 header comment: reword to "Copy Message / Select All".
  - Lines 773-780: remove `searches`, `onClick`, and the capture-phase click listener. Line 824: remove the matching `removeEventListener`.
  - Lines 785-786: expect `["Copy Message", "Select All"]` and retitle the check.
  - Lines 805-813: remove the re-selection lines and the "Search with Google opens a search for the selection" check, including `items()[2]?.click()`.
  - Lines 845 and 859: expect `["Select All"]`.
  - Line 868: expect `["Copy Message", "Select All"]`.
  - Run `node test/agent-session-view.mjs` from `crates/workshop/ui`. It fails against the unchanged `transcript-view.ts`, then passes after the removal.
- D1-9, regression. In [crates/workshop/ui/test/agent-session-service.mjs](promptforge/crates/workshop/ui/test/agent-session-service.mjs), after the `cancelTurn` block (around lines 578-612), add a case using `makeWire()` and an injected clock.
  - Setup as the neighboring case does: `wire.fire.session("s1")`, `wire.fire.inputRequired("tok1")`, `wire.respondResult = true`, `wire.cancelResult = true`, `service.respond("go")`, `service.cancelTurn()`.
  - Then `clock.now = 40; wire.fire.delta("reasoning", "late", 0)`. Assert the last item is a reasoning item with `pending === true`, `startedAt === null`, and `endedAt === 40`. Before the change `endedAt` is null, so this fails.
  - Then `wire.fire.inputRequired("tok2"); service.respond("again"); wire.fire.delta("reasoning", "fresh", 1)`. Assert the new item has `endedAt === null` and `startedAt` equal to the clock, which proves the flag cleared.
  - Also assert a reattach acknowledgment clears the flag: after a cancel, `wire.fire.session("s1")` again, then a first reasoning delta streams normally.
  - Run `node test/agent-session-service.mjs`.
- D1-1, guard plus fault injection. Run the new server test and the existing `agents` tests in `crates/workshop/server`. Then, without committing, inject a fault: change the server's `RUNTIME` key and the matching `services.provide` call in `agents.rs` to a different handle type (for example `ServiceKey<Arc<tokio::runtime::Handle>>` providing `Arc::new(Arc::new(runtime))`). Confirm the new test fails with a refusal naming `promptforge/tokio-runtime`, then revert. This shows the test catches Host-side drift between the server and `plugin-mcp`, the one pair no test bound before.
- Exit checks
  - `git grep -i "search with google"` and `git grep searchWithGoogle` under `crates` return nothing.
  - Run the full Workshop UI test set and the Workshop server tests.
  - Run the repository's standard format and lint gates for the touched TypeScript and Rust files.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - D1-2: delete the row and function entirely. This is the operator's instruction ("I don't want Search with Google anywhere"). It supersedes the analysis's narrower remedy of hiding the row when there is nothing to search. Consequence: the transcript menu has no web search, the secrets-to-a-third-party concern is gone with it, and the menu no longer matches Cursor's. Restoring it later is a small re-add. Confidence: high - the instruction is explicit and the change is one function and the tests that pin it.
  - D1-9: a private `stopped` flag that opens the late thought already ended. Rejected: having `thoughtStep` read `generating`, because `generating` only turns true on `respond` (line 341) and on an answered model call (line 449), so a reattach to a mid-turn session would render a live thought as finished. Rejected: dropping late deltas, because it can lose the cancelled round's visible text. Tradeoff: the late chunk's text is kept but shows no duration. Confidence: medium - the race is documented in `wire.rs` but cannot be reproduced without running it.
- User-resolved architecture choices
  - D1-1 option A: add the server test and record a revisit trigger, leave the public contract alone. Rejected: option B (a name-only presence check on `HostServices`, because `HostServices::provides` returns false for both an absent provider and a wrong-type one, so a new public method would be needed and it cannot be taken back once Plugins use it); option C (a contract-owned runtime handle abstraction, which breaks the public constant and touches `promptforge-plugin`, `plugin-web`, `plugin-mcp`, and `workshop-server` together); option D (drop D1-1).
- Rejected alternatives, assumptions, and risks
  - Assumption: the analysis read the MCP plugin, the readiness wait, the contract's `service.rs`, and most Workshop UI service and model code, but only sampled the large UI files (`rows.ts` after line 230, `zones.ts`, `workshop-panel.ts`, `find-widget.ts`, `editor-surface.ts`, the quick input, the run panel, most Gateway config pages) and about 70 UI test files. Debt may remain there.
  - Assumption: both plan records were read by keyword and range, and nothing was executed, so D1-9's race is inferred from code and comments.
  - Risk: commits `475b96214` and `a1201da73` have no `Plan:` trailer, so no design record explains them.
  - Risk: the D1-1 test depends on a closed loopback port refusing promptly. If a platform stalls, replace the URL fixture, never raise the 10 second bound.

### Deferred and Out of Scope

- A typed or contract-owned runtime key for the Plugin contract (option C) - revisit when a third tokio-based Plugin is added after `web` and `mcp`.
- A name-only presence check on `HostServices` so a missing-service reason can tell "none" from "wrong type" (option B) - revisit together with option C, or when a misread diagnostic is reported.
- The closed plan `vibe/2026-10-08-2-copy-cursor-everywhere.md` still lists Search with Google at lines 558 and 1051 - left as history, revisit if the operator asks for it to be amended.
- A late text (reply) delta after Stop opening a pending reply row that renders as streaming, the same shape as D1-9 - revisit if the D1-9 test sequence, extended with a `text` delta, reproduces it.
- C-5, the server leaving the runtime out when no runtime is current (the public synchronous `AppState::new` was not traced) - revisit if a caller outside a runtime is found.
- The unreviewed or sampled UI areas listed above - revisit with a deeper pass if the operator wants one.
- The `vibe2` and `vibe3` worktrees and branches - not touched.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the only default member; it needs no CUDA or Tauri packages). The desktop app is `cargo build -p workshop` (alias `cargo workshop` runs `build-workshop`). Both need the UI dependencies first: `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`. The workshop UI bundle alone is `npm run build --workspace ui` from `crates/workshop`; the config UI is `npm run build` from `crates/gateway/config-ui/ui`. The toolchain is `stable` (`rust-toolchain.toml`), edition 2024.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>` (for example `-p harness-runner a_stop_`); add `--all-features` for non-workshop crates, and `--test it` or `--test suite` to pick the integration binary with `cargo test`. UI: `node --test <file>.mjs` from `crates/workshop/ui` or `crates/gateway/config-ui/ui`. Structural checks: `cargo test -p build-xtask <name>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for a non-workshop crate; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the Workshop trio; `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless` for their separate partitions; `cargo test -p build-xtask` for boundary and structural checks; `npm test --workspaces --if-present` from `crates/workshop` and `npm test` from `crates/gateway/config-ui/ui` for the UIs.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, plus `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the Workshop crates, plus `cargo check -p gateway --no-default-features` for the headless build shape. The nightly-only `build-xtask` fixtures run with `cargo nextest run --locked -p build-xtask --run-ignored only` on the pinned nightly.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with env `CARGO_BUILD_WARNINGS=deny`. Never run a standalone `cargo check --workspace` beside clippy. UI typecheck: `npm run typecheck --workspaces --if-present` from `crates/workshop` and `npm run typecheck` from `crates/gateway/config-ui/ui`. Facade surface: `cargo +<pinned nightly> xtask api --check` (nightly named in `crates/build-xtask/src/api/toolchain.rs`, currently `nightly-2026-09-05`).
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` at the root; the pre-commit hook runs exactly this). No formatter check was found for the TypeScript or CSS.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with env `RUSTDOCFLAGS=-D warnings`; facade docs are `cargo doc -p promptforge --no-deps` (no `--all-features`) and `cargo doc -p harness --no-deps` with the same env, and Engine private items are `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`.
- Test placement and naming conventions: a module's unit tests sit in a sibling `<module>-tests.rs` file wired at the bottom of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (a `tests.rs` or inline `mod tests {}` also appears); shared test helpers live in `test_support.rs` or `tests/<binary>/support.rs`. Integration tests are one binary per crate, `tests/it/main.rs` or `tests/suite/main.rs`, declaring one module per file, with the file opening `#![expect(clippy::expect_used, clippy::unwrap_used, reason = ...)]` where helpers panic. Test names are snake_case sentences that state the behavior, such as `a_stop_leaves_a_question_to_the_operator_open`, with `#[tokio::test]` for async. Structural checks live in `crates/build-xtask/src/<check>.rs` with a `<check>-tests.rs` beside each. UI tests are `*.test.mjs` under `src/` or `*.mjs` under `test/`, run by `node --test`. Benches sit in `benches/` and examples in `examples/`. Behavior changes ship with tests in the same change. Slow-test limits and the `heavy` group for speech-engine tests are in `.config/nextest.toml`.
- Directory map: `crates/` holds every workspace crate. Public and shared crates sit at its top level (`promptforge`, `promptforge-plugin`, `harness`, `harness-gateway-client`, `plugin-mcp`, `plugin-user-input`, `plugin-web`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `workspace-hack`, `shared-ui`, and the `build-*` tooling crates). The manifestless containers hold private families: `crates/promptforge-internal/` (Engine: `types`, `vfs`, `model-client`, `lua`, `parser`, `engine`), `crates/harness-internal/` (`runner`), `crates/gateway/` (`app`, `config`, `config-ui`, `cloud-providers`, `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, `stt/{api,engine,backend-whisper,whisper-ffi}`), and `crates/workshop/` (`desktop`, `server`, `server-api`, `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, `workspace`, plus the TypeScript packages `ui`, `look`, `platform`). `guide/` holds the user guide sources (`books`, `chrome`, `landing`). `prompts/` holds example prompt files. `tools/` holds Node scripts that stage the gateway sidecar and run a live TTS check. `vibe/` holds dated plan and run notes. `local/` holds the operator's untracked config and fixtures. `.github/workflows/` holds CI, release, and site workflows. `.githooks/` holds the pre-commit (fmt) and pre-push (headless check, clippy, cargo deny) hooks. `.config/` holds `nextest.toml` and `hakari.toml`. `.cursor/rules/` holds the Workshop architecture and SPA rules. `.cargo/config.toml` defines the `xtask` and `workshop` aliases. Root files: `Cargo.toml`, `Cargo.lock`, `AGENTS.md`, `clippy.toml`, `deny.toml`, `rustfmt.toml`, `dist-workspace.toml`, `rust-toolchain.toml`. `target/` and `target-msrv/` are ignored build output.
- Component boundaries: the Engine is `promptforge` (the public facade, the only crate allowed to depend on `promptforge-internal`), over `promptforge-engine`, which uses `promptforge-lua`, `promptforge-parser`, `promptforge-model-client`, `promptforge-types`, and `promptforge-vfs`; `promptforge-types` and `promptforge-vfs` depend on no other workspace crate. The Plugin contract `promptforge-plugin` depends on `promptforge-types` and `promptforge-vfs`, and `plugin-mcp`, `plugin-user-input`, and `plugin-web` depend only on it. The Harness is `harness`, a facade over `harness-runner`, which depends on `promptforge` and `promptforge-plugin`; `harness-gateway-client` depends on `harness`, `plugin-web`, and `promptforge`. The Gateway depends on none of the Engine or Harness crates: `gateway` fans out to `gateway-config`, `-protocol`, `-routing`, `-local`, `-stt`, `-web-search`, `-progress`, `-logging`, `-config-ui`, `-api-types`, and `-api-discovery`, and its speech stack runs `gateway-stt` over `gateway-stt-engine`, `gateway-stt-backend-whisper`, and `gateway-whisper-ffi`. Workshop depends one way, server over features over services over vocabulary: `workshop-server` composes the feature crates (`agents`, `gateway`, `menu`, `run-log`, `status`, `user-state`, `workspace`); most of them depend on `workshop-registry`, `workshop-support`, and `workshop-protocol`, and `workshop-agents` and `workshop-run-log` also depend on `harness`; same-tier crates never depend on each other except `workshop-registry` on `workshop-protocol`. The desktop app `workshop` depends on `workshop-server-api` (the facade over `workshop-server`), never on `workshop-server`. Each `workshop-*` crate's `lib.rs` carries a `## Invariants` doc section listing what it may and may not depend on. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, and lint inheritance; crates inherit `[workspace.lints]` with `[lints] workspace = true`.
- Conventions summary: kebab-case directories and files in the UI, with each `.css` beside its `.ts` and `.ws-` class prefix, and no raw color, size, or spacing values in component CSS. Rust source directories are flat unless a subdirectory has at least three files; smaller groups are `foo-bar.rs` siblings wired with `#[path = "foo-bar.rs"] mod bar;`. `lib.rs` is a facade of docs, attributes, `mod`, and `pub use`. Clippy `all` and `pedantic` are denied, `unwrap_used`, `expect_used`, and `allow_attributes` are denied, and `#[expect(..., reason = ...)]` is the only form of suppression; unsafe code is denied outside its allow-listed boundary and `missing_docs` warns. Capitalized terms Engine, Harness, Host, and Plugin each have one meaning in code, docs, and plans, and `crates/workshop/ui/test/docs-claims.mjs` enforces them. Engine crates call their caller "the caller" and never name the Host. Comments explain non-obvious constraints and cite an upstream issue URL for any workaround. JSON that reaches a recorder round-trips exactly (`float_roundtrip`, sorted keys). Error messages are written for a model to read. Nearly every crate has a `build.rs` that depends on `build-ceiling`. Dependency pins carry explanatory comments in the root `Cargo.toml`. Languages seen: Rust, TypeScript, JavaScript (`.mjs`), Lua (the Engine's prelude), CSS, and Markdown prompts.

</project-survey>
<execution-plan>

## Execution Instructions

Plan shape: Full path, 3 components, 3 steps, 3 commits. The objective is to remove the three pieces of debt the Debt Collector kept, with no change to the Plugin contract's public interface, no new `HostServices` method, and no work on any rejected candidate.

Components, in dependency order. No component depends on another in code, so the order follows risk and what each verification runs:

1. Transcript menu (D1-2). First. It is the smallest and highest-confidence change, and it edits `agent-session-view.mjs`, which Step 2 also runs, so that test is already in its final state when Step 2 needs it.
2. Stopped-turn reasoning (D1-9). Second. It touches the UI service and its test, not the view, but its verification runs the view test, so it follows component 1.
3. Runtime key guard (D1-1). Last. It touches only the server's test file, and its fault-injection check temporarily edits `agents.rs`, so it goes after the two UI commits and ends with a clean worktree once the fault is reverted.

Pieces: each component is a single piece, so no sequential or joint choice arises inside a component. D1-2 is one deletion plus the tests that pin it. D1-9 is one flag whose set, clear, and use cannot be split without a commit holding dead code. D1-1 is one test plus its fault check.

Every step ends with its own test, typecheck, format, and lint gates, so each commit is green when it is made. The steps share no files, so any order works. The order above is a preference, not a requirement.

<step-1>

### Step 1: Remove "Search with Google" from the transcript context menu [completed]

- Component: Transcript menu
- Debt: D1-2. The operator's instruction is that "Search with Google" does not appear anywhere. The row does nothing outside a message row, and otherwise sends a whole message to a third party as one URL.
- Artifacts: `crates/workshop/ui/test/agent-session-view.mjs` (test edits), `crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts` (`openContextMenu` and the module-level `searchWithGoogle` function).
- Test first, in `agent-session-view.mjs` (line numbers are from the unchanged file):
  - Line 13 header comment: reword to "Copy Message / Select All".
  - Lines 773-780: remove `searches`, `onClick`, and the capture-phase click listener. Line 824: remove the matching `removeEventListener`.
  - Lines 785-786: expect `["Copy Message", "Select All"]` and retitle the check.
  - Lines 805-813: remove the re-selection lines and the "Search with Google opens a search for the selection" check, including `items()[2]?.click()`.
  - Lines 845 and 859: expect `["Select All"]`.
  - Line 868: expect `["Copy Message", "Select All"]`.
  - Run `node test/agent-session-view.mjs` from `crates/workshop/ui` and confirm it fails against the unchanged `transcript-view.ts`.
- Source change in `transcript-view.ts`:
  - In `openContextMenu` (around lines 234-270) keep the Copy item guarded by `subject !== ""` and keep Select All. Drop the "Search with Google" item.
  - Delete `searchWithGoogle` (around lines 273-288) with its doc comment. Nothing else in the file uses it, so the view no longer builds an anchor or hands a navigation to the desktop shell.
- Verification:
  - `node test/agent-session-view.mjs` passes. The menu offers only Copy or Copy Message (when there is something to copy) and Select All.
  - `git grep -i "search with google" crates` and `git grep searchWithGoogle crates` return nothing.
  - From `crates/workshop`, `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present` pass.
- Commit: one commit with the test edits and the source deletion. Leave the closed plan record `vibe/2026-10-08-2-copy-cursor-everywhere.md` alone.
- Confidence: high - explicit instruction, small and self-contained change.
- Dependencies: none.

</step-1>
<step-2>

### Step 2: Open a late reasoning chunk after Stop as an ended thought

- Component: Stopped-turn reasoning
- Debt: D1-9. A first reasoning chunk that arrives after Stop opens a pending thought with no end, so `thoughtStep` reports `streaming: true` under an idle composer. `cancelTurn` ends only already-open thinking, and `foldDelta` has no post-cancel guard.
- Artifacts: `crates/workshop/ui/test/agent-session-service.mjs` (new case), `crates/workshop/ui/src/services/agent-session.ts` (new private `stopped` flag, `cancelTurn`, `respond`, `acknowledge`, the `wire.onInputRequired` handler, `foldDelta`).
- Test first, in `agent-session-service.mjs`, after the `cancelTurn` block (around lines 578-612), add one case using `makeWire()` and an injected clock:
  - Setup as the neighboring case does: `wire.fire.session("s1")`, `wire.fire.inputRequired("tok1")`, `wire.respondResult = true`, `wire.cancelResult = true`, `service.respond("go")`, `service.cancelTurn()`.
  - Then `clock.now = 40; wire.fire.delta("reasoning", "late", 0)`. Assert the last item is a reasoning item with `pending === true`, `startedAt === null`, and `endedAt === 40`.
  - Then `wire.fire.inputRequired("tok2"); service.respond("again"); wire.fire.delta("reasoning", "fresh", 1)`. Assert the new item has `endedAt === null` and `startedAt` equal to the clock, which proves the flag cleared.
  - Also assert a reattach acknowledgment clears the flag: after a cancel, `wire.fire.session("s1")` again, then a first reasoning delta streams normally.
  - Run `node test/agent-session-service.mjs` from `crates/workshop/ui` and confirm the new case fails (`endedAt` is null) while the existing cases pass.
- Source change in `agent-session.ts`:
  - Add a private boolean `stopped`, initially false.
  - Set it true in `cancelTurn` after `wire.cancelTurn()` succeeds.
  - Clear it in `respond` after a successful send (the turn's work restarts), in the `wire.onInputRequired` handler (the relaunched agent is waiting, so the grace window is over), and in `acknowledge` (a session acknowledgment, including reattach).
  - In `foldDelta`, when no pending item matches and the delta kind is `reasoning`: if `stopped`, push the item with `startedAt: null` and `endedAt: this.now()`; otherwise push it as before (`startedAt: this.now()`, `endedAt: null`). `thoughtStep` in `transcript-model.ts` (line 387) then reads `streaming: false`, and `durationMs` is null because `startedAt` is null. The durable `agent_thought` handler already reads `pending?.startedAt` and `pending?.endedAt`, so it inherits the ended item.
  - Leave `transcript-model.ts`, the wire, the server, the `settled` cut-off, and deltas that append to an existing item unchanged.
- Late reply check (no code to commit): once the case passes, run a throwaway copy of the same sequence with a `text` delta after Stop and see whether it leaves a pending reply row that renders as streaming. Do not commit the probe and do not widen this change. If it reproduces, report it to the operator in the hand-off. This plan's Deferred section already records the item.
- Verification:
  - `node test/agent-session-service.mjs` passes, including the existing `cancelTurn` case.
  - `node test/agent-session-view.mjs` passes.
  - From `crates/workshop`, `npm test --workspaces --if-present` and `npm run typecheck --workspaces --if-present` pass.
- Commit: one commit with the new test case and the `agent-session.ts` change.
- Confidence: medium - the race is documented in `crates/workshop/server/src/agents/wire.rs` lines 12-17 but was inferred from code, not reproduced by running.
- Dependencies: none in code. It follows Step 1 only so `agent-session-view.mjs` is already in its final state when this step runs it.

</step-2>
<step-3>

### Step 3: Guard the runtime service key with a server test

- Component: Runtime key guard
- Debt: D1-1, option A. The contract owns only the bare string `TOKIO_RUNTIME`, and three production crates (`plugin-web`, `plugin-mcp`, `workshop-server`) each build their own `ServiceKey<Handle>` from it. No test bound the server's key to the one `plugin-mcp` reads. Add that guard and leave the public contract alone.
- Artifacts: `crates/workshop/server/src/agents/tests.rs` (one new `#[tokio::test]`, the only committed change). Read for reference: `crates/harness-internal/runner/tests/it/prepare-ready.rs`, `crates/plugin-mcp/src/lib.rs` lines 90-109, `crates/plugin-mcp/src/entry.rs` lines 101-106. Edited temporarily and never committed: `crates/workshop/server/src/agents.rs` lines 74-75.
- Write the test:
  - Confirm the exact refusal wording against `prepare-ready.rs` before fixing the assertion text.
  - Name it as a sentence that states the behavior, for example `a_remote_mcp_entry_reads_the_runtime_the_server_provides`.
  - Build a host with `with_plugins(services(&Registry::new()), vec![("docs".to_owned(), json!({"url": <closed loopback URL>}))])`, run `run_prompt(&Arc::new(host), requiring("docs"))`, and assert on the `RunOutcome` refusal. The test file already has `with_plugins`, `services`, `run_prompt`, `requiring`, `RunOutcome`, `json!`, and the `TOKIO_RUNTIME` import.
  - Get the URL by binding `std::net::TcpListener::bind(("127.0.0.1", 0))`, reading the port, and dropping the listener, so the connection is refused at once. `entry::parse` only requires an `http://` or `https://` prefix.
  - The `docs` server constructs by reading the runtime through `plugin-mcp`'s own key, its connection task fails fast, and `Server::ready` reports the failure. Assert the refusal contains "the MCP handshake failed" (proof the Plugin got past the runtime read) and does not contain `promptforge/tokio-runtime`.
  - `run_prompt` already bounds the run at 10 seconds. If a platform stalls on the closed port, replace the URL fixture and never raise that bound.
- Fault injection (do not commit): in `agents.rs`, change the server's `RUNTIME` key and the matching `services.provide` call to a different handle type, for example `ServiceKey<Arc<tokio::runtime::Handle>>` providing `Arc::new(Arc::new(runtime))`. Run the new test and confirm it fails with a refusal naming `promptforge/tokio-runtime`. Revert the fault, confirm `git diff --stat` lists only `tests.rs`, and rerun the test to confirm it passes again.
- Production code: none. No change to the Plugin contract, no new `HostServices` method, no typed key. The revisit trigger (a third tokio-based Plugin added after `web` and `mcp`) stays recorded in this plan's Deferred and Out of Scope section, which this step leaves as is.
- Verification:
  - The new test passes on the real code and fails under the injected fault with a refusal naming `promptforge/tokio-runtime`.
  - `cargo nextest run --locked -p workshop-server agents` passes.
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo nextest run --locked -p workshop-server --features headless` pass.
  - `cargo fmt --all --check` passes, and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets` passes with env `CARGO_BUILD_WARNINGS=deny`.
- Commit: one commit containing only the new test in `tests.rs`.
- Confidence: medium - the guard is cheap and reversible, but the fixture's refusal timing and wording must be confirmed against `prepare-ready.rs`.
- Dependencies: none.

</step-3>

</execution-plan>