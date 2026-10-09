---
name: Finish search removal
overview: "Finish what the debt-removal run missed and close out the four follow-ups: remove the leftover \"Search with Google\" text (a split-line header comment and the closed plan record), suppress the native right-click menu inside the Gateway config panel iframe, and record the no-change decisions for the late-text and commit-trailer items."
todos:
  - id: f1-finish-removal-text
    content: "F1: reword the split-line header comment in transcript-view.ts and edit lines 558 and 1051 of the closed plan record vibe/2026-10-08-2-copy-cursor-everywhere.md; verify with a multi-line search and the workshop UI typecheck"
    status: pending
  - id: f2-config-panel-context-menu
    content: "F2: add panel and standalone contextmenu tests to config-ui mode.test.mjs (npm run build first, panel test must fail), then install a capture-phase contextmenu preventDefault on root.ownerDocument in boot()'s panel branch in main.ts; run config-ui npm test and typecheck"
    status: pending
isProject: false
---

# Follow-ups After Debt Removal: Leftover Search Text and the Config Panel Menu

<product-contract>

## Product Requirements

- Scope and target work
  - Repository: `promptforge` (`c:\Users\Vinnie\cursor\promptforge`), branch `master`, at `f877f4244`, which equals `origin/master`. The four commits of `vibe/2026-10-08-3-debt-removal-mcp-cursor.md` (`1026c0f13`, `d7e37932b`, `c762c310f`, `f877f4244`) are already pushed. This plan adds commits on top and rewrites no history.
  - The operator asked to address four follow-ups from that run: the leftover Search with Google text (F1), the Gateway config panel's native right-click menu (F2), a late text chunk after Stop (F3), and the doubtful `Design:` trailer on `d7e37932b` (F4). F1 is new: Step 1 of that run claimed "no references remain" from a single-line search that missed a phrase split across two comment lines.
- Cleanup goals and non-goals
  - Goals: no mention of the removed menu row remains in Workshop source or in the closed plan record it was copied from (F1); the Gateway config panel no longer shows the host webview's native context menu, matching the rest of the Workshop (F2); F3 and F4 are closed with their evidence and decisions recorded.
  - Non-goals: no rewrite or force-push of published commits, no change to how late text chunks are folded, no change to the standalone (system browser) config page, no render-performance work, no PR.
- Success criteria
  - A multi-line-aware search for "search with google" under `crates`, and in the closed plan record `vibe/2026-10-08-2-copy-cursor-everywhere.md`, returns nothing. The removal plan `vibe/2026-10-08-3-debt-removal-mcp-cursor.md` and this plan name the phrase by necessity and are excluded.
  - In panel mode a `contextmenu` event on the config page is default-prevented. In standalone mode it is not. The panel test fails before the change.
  - Every touched test and typecheck passes, including the full verification at the final step.

## Functional Specification

### Follow-up Inventory

- Debt added: none new from the target work. F1 and F2 are leftovers of the "no Search with Google anywhere" instruction, F1 from a verification gap in the earlier run and F2 from a surface that run never looked at.
- Cheap fixes
  - F1, stale text. [crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts](promptforge/crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts) lines 12-14 still read "menu (Copy Message, or Copy over a selection; Select All; Search with / Google)", split across a line break, so the earlier `git grep -i "search with google"` missed it. The closed plan record [vibe/2026-10-08-2-copy-cursor-everywhere.md](promptforge/vibe/2026-10-08-2-copy-cursor-everywhere.md) still lists the row at line 558 ("**Transcript context menu:** Copy Message (or Copy when text is selected), Select All, Search with Google.") and line 1051 ("The transcript context menu offers Copy Message (Copy when text is selected), Select All, and Search with Google."). No other occurrence exists in the repository. A multi-line search over the whole repository confirmed that.
  - F2, config panel menu. [crates/workshop/ui/src/parts/gateway/gateway-config-panel.ts](promptforge/crates/workshop/ui/src/parts/gateway/gateway-config-panel.ts) embeds the config SPA in a same-origin iframe at `/gateway/config/?mode=panel&bridge=...`. The Workshop's `contextmenu` suppression (`crates/workshop/ui/src/main.ts` line 76, a capture listener on its own `document`) does not cross the frame boundary, and `crates/gateway` has no `contextmenu` handling (a search found none). So a right-click inside the panel shows the host webview's native menu. What that menu contains varies by platform and I could not run it to see, so that is inference. It is the only embedded frame in the Workshop UI source.
- Closed with no work item
  - F3, late text chunk after Stop: no defect, no change (see the Decision Record).
  - F4, the `Design: extends god-object` trailer on `d7e37932b`: left as is by the operator (see the Decision Record).
- Exposed pre-existing debt: none.
- Rejected candidates: not applicable, this plan did not run a candidate pass.

</product-contract>
<implementation-contract>

## Technical Design

- F1 (comment and record text only, no behavior)
  - In `transcript-view.ts`, reword the header comment so it reads "...and the transcript context menu (Copy Message, or Copy over a selection; Select All)." and re-wrap the two lines.
  - In the closed plan record, line 558 becomes "**Transcript context menu:** Copy Message (or Copy when text is selected) and Select All." and line 1051 becomes "The transcript context menu offers Copy Message (Copy when text is selected) and Select All." Add no note that the row existed, since that would reintroduce the phrase.
- F2 (config UI only, no interface change)
  - In [crates/gateway/config-ui/ui/src/main.ts](promptforge/crates/gateway/config-ui/ui/src/main.ts), in `boot()`, the `params.get("mode") === "panel"` branch (around line 90) installs a capture-phase `contextmenu` listener on `root.ownerDocument` that calls `event.preventDefault()`, before it calls `mountPanelMode`. Installing it in the branch, not inside `mountPanelMode`, covers the bridge-pending page too, because `mountPanelMode` returns early without a usable `bridge` origin.
  - Standalone mode is untouched: there the page lives in the operator's own browser, which owns its context menu, and suppressing it would remove normal browser behavior.
  - No Workshop-side change. No change to the bridge, the iframe's `sandbox` attribute, or the Workshop server.

</implementation-contract>
<verification-contract>

## Testing Plan

- F1
  - There is no failing-test-first shape: the change is comment and closed-record text. Verification is a multi-line-aware search that returns nothing: `rg -U -i "search\s+with(\s|//|\*)+google" crates vibe/2026-10-08-2-copy-cursor-everywhere.md` from the repository root, plus `rg -i searchWithGoogle crates`.
  - Run `npm run typecheck --workspace ui` from `crates/workshop` to show the comment edit broke nothing.
- F2, regression
  - The config UI's tests import the built bundle `crates/gateway/config-ui/ui/dist/app.js`. That directory is gitignored, and `npm test` does not rebuild it. Run `npm run build` in `crates/gateway/config-ui/ui` before every test run, or the tests exercise a stale bundle.
  - In [crates/gateway/config-ui/ui/src/mode.test.mjs](promptforge/crates/gateway/config-ui/ui/src/mode.test.mjs), which pins mode detection at boot, add two tests using `bootApp`:
    - `?mode=panel`, boot with the stub as the existing panel test does, dispatch `new dom.window.MouseEvent("contextmenu", { bubbles: true, cancelable: true })` on an element inside `root`, and assert `defaultPrevented === true`. Before the change it is false, so the test fails.
    - Standalone, boot with a stored key, dispatch the same event, and assert `defaultPrevented === false`. It passes before and after, and pins that standalone is left alone.
  - Run `node --test src/mode.test.mjs`, confirm the panel test fails, implement, rebuild, and confirm both pass. Then run `npm test` and `npm run typecheck` in the same directory.
- Exit checks
  - The final step runs at full scope: build, format check, clippy, typechecks, docs, and the full test suites from the project survey.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - F2, suppress the whole native menu in panel mode, not only outside editable fields. This matches the Workshop's own page, which suppresses everywhere including inputs. Consequence: no right-click Cut, Copy, or Paste in the panel's inputs, such as the Hugging Face key field. Ctrl+X, Ctrl+C, and Ctrl+V still work, as they do in the rest of the Workshop. Rejected: suppressing only on non-editable targets, because the native menu in an input can still offer a web search for selected text, which the operator ruled out, and the Workshop offers no such menu either. Rejected: a Workshop-side listener on the iframe's `contentDocument`, because it reaches into the frame, needs load and navigation handling, and the SPA already knows its own mode. Rejected: a shell-level webview setting, because it is platform-specific and the Workshop already solves this in page script. Confidence: medium - the choice is consistent with the Workshop, but I have not seen the native menu on each platform.
  - F1, delete the text without a note. The closed plan record is history, and a note about the removed row would put the phrase back and defeat the search that proves the removal.
- User-resolved choices
  - F4: leave the `Design: extends god-object` trailer on `d7e37932b` (the operator's choice). The commits are on `origin/master`, so changing the message means rewriting published history and a force-push. The trailer is a lead, not evidence, because a debt pass reads trailers only as pointers. My doubt about it: the class's foreign-data reads are the wire frames it exists to fold, while the label's signal asks for reads of unrelated types. Revisit if a later debt pass spends effort on it.
  - F3: no code change (the operator deferred to my judgement after discussing it). Evidence, all from reading code: a late text chunk after Stop opens a reply row with `pending: true`; the row's `streaming` flag only reaches the markdown renderer, whose own doc says the hint "currently changes nothing" apart from a tail fade; no CSS keys off it; the turn footer depends on `generating`, not on pending replies. Reply ids are the run's round counter (`next_round` in `crates/promptforge-internal/engine/src/execute/scheduler.rs` lines 360-361), so a cancelled round's id is never reused and the orphan row cannot collide with later text. Stop drops the model call (`stop_round` in `crates/harness-internal/runner/src/harness-control.rs`), the text pump is a callback inside the dropped call's read loop, and what is already queued is capped by the 256-frame delta ring (`DELTA_CAPACITY` in `crates/workshop/agents/src/conversation.rs`). Partial text is never durable, so it vanishes on a reattach. Rejected: dropping late text after Stop, because on a large browser-side backlog it would discard text the model already produced and cut the transcript at a lag-dependent point. Confidence: medium - the large-backlog case is inference, not measured.
- Rejected alternatives, assumptions, and risks
  - Assumption: the native menu inside the iframe is the host webview's default menu. I could not run the app to see its items.
  - Risk: the config UI tests silently run against a stale `dist/app.js` if the build is skipped. The plan builds before every run, and the failing-first check proves the bundle is fresh, because the panel test must fail before the change.
  - Risk: Step 1 of the earlier run used a single-line search and missed a split-line mention. This plan's checks use multi-line search for that reason.
  - Assumption carried over: the earlier debt pass only sampled large UI areas (`rows.ts` after line 230, `zones.ts`, `workshop-panel.ts`, `find-widget.ts`, `editor-surface.ts`, the quick input, the run panel, most Gateway config pages) and about 70 UI test files.

### Deferred and Out of Scope

- Painting the transcript at most once per animation frame, so the UI keeps up with a long streamed reply - revisit if the UI is observed lagging behind the socket.
- Wording of the `//!` module comment in `crates/workshop/server/src/agents/wire.rs` (lines 12-17), which says a cancelled round's deltas "fall to the round that eventually does" - revisit when that file is next edited, since round ids come from a run-wide counter and are not reused (confidence: low that the comment is wrong).
- A partial reply left visible after Stop disappearing on reattach - revisit if operators report it as confusing.
- The standalone (system browser) config page's native menu - stays, because the browser owns it.
- Rewriting the `Design:` trailer on `d7e37932b` (F4) - revisit only with a force-push the operator approves.
- Editing the closed removal record `vibe/2026-10-08-3-debt-removal-mcp-cursor.md` to drop the phrase - not done, since it is the record of the removal.
- Opening a pull request for the pushed commits - not part of this plan.
- The sampled UI areas listed above - revisit with a deeper pass if wanted.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the only default member; it needs no CUDA or Tauri packages). The desktop app is `cargo build -p workshop` (alias `cargo workshop` runs `build-workshop`). Both need the UI dependencies first: `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui`. The workshop UI bundle alone is `npm run build --workspace ui` from `crates/workshop`; the config UI is `npm run build` from `crates/gateway/config-ui/ui`. The toolchain is `stable` (`rust-toolchain.toml`), edition 2024.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>` (for example `-p harness-runner a_stop_`); add `--all-features` for non-workshop crates, and `--test it` or `--test suite` to pick the integration binary with `cargo test`. UI: `node --test <file>.mjs` from `crates/workshop/ui` or `crates/gateway/config-ui/ui`. Structural checks: `cargo test -p build-xtask <name>`. The config UI's tests import the built bundle `crates/gateway/config-ui/ui/dist/app.js` (gitignored); run `npm run build` in `crates/gateway/config-ui/ui` before any config UI test run, because `npm test` does not rebuild it.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for a non-workshop crate; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the Workshop trio; `cargo nextest run --locked -p workshop-workspace --all-features` and `cargo nextest run --locked -p workshop-server --features headless` for their separate partitions; `cargo test -p build-xtask` for boundary and structural checks; `npm test --workspaces --if-present` from `crates/workshop` and `npm test` from `crates/gateway/config-ui/ui` for the UIs (build the config UI bundle first, see the focused test line).
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

Plan shape: Bounded path, 2 steps, 2 commits, no components. The steps share no files, so any order works; Step 1 goes first because it is text only.

<step-1>

### Step 1: Remove the leftover Search with Google text [completed]

- Component: none
- Follow-up: F1. Step 1 of the earlier debt-removal run left the phrase in two places because its search matched single lines only.
- Artifacts: `crates/workshop/ui/src/parts/agent/transcript/transcript-view.ts` (header comment, lines 12-14) and `vibe/2026-10-08-2-copy-cursor-everywhere.md` (lines 558 and 1051).
- Test first: there is no failing-test-first shape, because the change is comment and closed-record text only; state that deviation in the return. The check that proves removal is the multi-line search under Tests, which finds three matches before the change (the header comment as one match across two lines, plus the two record lines) and none after.
- Change:
  - In `transcript-view.ts`, reword the header comment so it reads "...and the transcript context menu (Copy Message, or Copy over a selection; Select All)." with the comment lines re-wrapped to the same width as their neighbors. Change nothing else in the file.
  - In `vibe/2026-10-08-2-copy-cursor-everywhere.md`, line 558 becomes `    - **Transcript context menu:** Copy Message (or Copy when text is selected) and Select All.` and line 1051 becomes `  - The transcript context menu offers Copy Message (Copy when text is selected) and Select All.` Keep each line's indentation. Add no note that a row was removed, since that would put the phrase back.
- Tests:
  - From the repository root, a multi-line search for the phrase returns nothing: `rg -U -i "search\s+with(\s|//|\*)+google" crates vibe/2026-10-08-2-copy-cursor-everywhere.md`, and `rg -i searchWithGoogle crates`.
  - From `crates/workshop`, `npm run typecheck --workspace ui` passes.
- Commit: one commit with both files. Leave `vibe/2026-10-08-3-debt-removal-mcp-cursor.md` and this plan alone, since they name the phrase as records of the removal.
- Confidence: high - text only, and the exact lines are known.
- Dependencies: none.

</step-1>
<step-2>

### Step 2: Suppress the native context menu in the Gateway config panel

- Component: none
- Follow-up: F2. The Workshop embeds the config SPA in an iframe, and the Workshop page's own `contextmenu` suppression (`crates/workshop/ui/src/main.ts`, a capture listener on its `document`) does not reach inside the frame.
- Artifacts: `crates/gateway/config-ui/ui/src/mode.test.mjs` (two new tests) and `crates/gateway/config-ui/ui/src/main.ts` (`boot()`, the `params.get("mode") === "panel"` branch, around line 90).
- Test first, in `mode.test.mjs` using `bootApp` and `gatewayStub` from `./test-support.mjs`:
  - Panel: boot `http://127.0.0.1:8081/config/?mode=panel` with `gatewayStub()` as the existing panel test does, dispatch `new dom.window.MouseEvent("contextmenu", { bubbles: true, cancelable: true })` on an element inside `root` (for example `root.querySelector("main")`, falling back to `root`), and assert the event's `defaultPrevented` is true. Name the test as a sentence stating the behavior.
  - Standalone: boot with a stored key (`bootApp({ key: "k", stub })`), dispatch the same event, and assert `defaultPrevented` is false.
  - These tests read the built bundle `crates/gateway/config-ui/ui/dist/app.js`, which is gitignored and not rebuilt by `npm test`. From `crates/gateway/config-ui/ui`, run `npm run build`, then `node --test src/mode.test.mjs`, and confirm the panel test fails before any source change.
- Change in `main.ts`: in the panel branch of `boot()`, before `mountPanelMode(...)`, add a capture-phase `contextmenu` listener on `root.ownerDocument` whose handler calls `event.preventDefault()`, with a short comment saying why (the panel is an iframe in the Workshop, so the Workshop page's suppression does not reach it). Do not touch standalone mode, `mountPanelMode`, or the bridge.
- Tests:
  - From `crates/gateway/config-ui/ui`: `npm run build`, then `node --test src/mode.test.mjs` passes both new tests.
  - Then `npm test` and `npm run typecheck` pass in the same directory, after the build.
- Commit: one commit with both files.
- Confidence: medium - the logic is small, but I have not seen the native menu's contents on each platform.
- Dependencies: none.

</step-2>

</execution-plan>
