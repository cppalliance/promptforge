---
name: Remove STT fix debt
overview: "Remove the two debts that the STT report-fix work added: the sparse-successor cut that can silently drop up to 3 words (D1-1), and the README claims that the same work made false (D1-2). Both fixes are private or documentation-only, so nothing needs a decision before work starts."
todos:
  - id: d1-1-successor-cut
    content: "D1-1: cut the sparse successor at the projected token with no punctuation snap, log the cut position, add the punctuated-successor regression test (fails first)"
    status: in_progress
  - id: d1-2-readme
    content: "D1-2: fix README lines 33, 43, 58 and 80 so CI scope and forced-window history match the code"
    status: pending
  - id: exit-checks
    content: Run STT crate tests, fmt, clippy with warnings denied, and the native long-speech test if fixtures are available
    status: pending
isProject: false
---

# Remove the debt added by the STT report fixes

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository `c:\Users\Vinnie\cursor\promptforge2`, a linked git worktree on branch `vibe2`. Target: the nine commits whose `Plan:` trailer names `vibe/2026-10-08-2-stt-report-fixes.md`, from baseline `a1201da73` (excluded) to endpoint `14324ac77`: `3deaa3751`, `0a8307b2f`, `25ed6d17d`, `ca4530cf5`, `27a99bf2f`, `ced8a44e8`, `784913841`, `45061a024`, `14324ac77`.
  - Disposition ref is the endpoint. The worktree was clean and is excluded.
  - Evidence came from reading code and history only. Nothing was built or run for the analysis.
  - That work fixed silent realtime session exits, forced-window word loss, the PCM double count, term hints on both routes, the `realtime-transcribe` model name on batch, and added a native long-speech test and its CI job. Its design record is `vibe/2026-10-08-2-stt-report-fixes.md`.
- Cleanup goals and non-goals:
  - Goal: the sparse-successor branch of the forced-overlap fallback never loses words that exist only in the successor's new audio.
  - Goal: `crates/gateway/stt/README.md` states what CI runs and what a final decode is conditioned on, as the code at the endpoint does.
  - Non-goals: any change to the predecessor cut, the one-third density threshold, the 5-word native gate, the wire protocol, or the unrelated pre-existing panics listed under Deferred and Out of Scope.
- Success criteria:
  - The new regression test for D1-1 fails on `14324ac77` and passes after the change.
  - Every existing sparse-successor and forward-only-snap test still passes.
  - The README rows named under D1-2 agree with `TakeState::decoded_text` and with `.github/workflows/stt-miri.yml`.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1, introduced by `ced8a44e8`. The sparse-successor branch of `estimate` in `crates/gateway/stt/api/src/take/state/reconcile.rs` (lines 43-60) cuts the successor text with `projected_prefix_end`, which ends in `locate_cut` in `crates/gateway/stt/api/src/take/agreement-projection.rs`. That commit made `locate_cut` search for punctuation only from the projected token forward (`candidate_start = projected.max(1)`, `candidate_end = projected + 3`, lines 109-110). The direction is safe for the predecessor cut, where words moved past the cut are repeated in the successor's overlap. For the successor cut, the words before the cut are the overlap, which the whole-settled predecessor already holds, and the words after it exist only in the pending text. A forward snap by k tokens therefore deletes k words. Condition: alignment fails, the successor is sparse, and exactly one token in the 3 tokens after the projected token ends in punctuation. Loss is at most 3 words with no error, and the sparse warning log cannot show it because it records only the total token count. The native `realtime_long_speech` gate fails only on runs of 5 or more missing words, so CI cannot see it. Reversal cost is low: the helpers are private to the take module and nothing is persisted or on the wire. State at the endpoint: present. Contract contradicted: the `ced8a44e8` message says the successor text is kept "from the projected overlap end".
  - D1-2, introduced by `45061a024` and `784913841`. `crates/gateway/stt/README.md` line 80 says "CI does not load the recommended `base.en` and `small.en` pair, so it checks behavior, not accuracy", and its list of native CI steps omits the new long-speech step, although `45061a024` made the same job download both models and run `realtime_long_speech`. README lines 33, 43 and 58 say final decodes are conditioned on the take's decoded history and that the final prompt is "the glossary and the decoded history", although `784913841` makes `TakeState::decoded_text` return an empty string while a forced window is pending. Impact: documentation only. Reversal cost: a few lines. State at the endpoint: present. The history rows are a narrowing and not a flat contradiction, and the same change records lost style conditioning at seams as a known risk, so the rows should say so.
- Cheap fixes and exposed pre-existing debt:
  - Cheap fixes: none.
  - Exposed pre-existing debt: none retained. One unrelated pre-existing item is listed under Deferred and Out of Scope.
- Rejected candidate counts: 19 in total.
  - Residual-but-acceptable, 7:
    - the predecessor cut can repeat up to 3 words, which is the named trade-off
    - the PCM cap no longer bounds one incoming append batch, which is deliberate, bounded, and fixes a false refusal
    - a skipped caption tick logs only at debug, like the existing worker-queue skip
    - behavior changes for callers (batch prompt, realtime vocabulary, error text, session end) are additive and tied to named defects
    - the history drop is pinned by a scripted decoder, with the native test as the backstop
    - the clip zip is hand-built but hash-pinned in two places and verified against the release
    - three existing tests were lengthened to stay out of the new sparse rule, and their properties are kept
  - Weak or speculative, 9:
    - two defensive `end_session` exits are practically unreachable
    - internal error text on the wire, with no leak path found
    - the Workshop UI treats an id-less error event as a take failure, which matches its documented rules
    - the 800-byte limit is written in `guidance.rs` and in the backend, with a one-way consequence and nothing wrong today
    - a client prompt can drive up to a few hundred glossary refits per decode, which is bounded and unmeasured
    - structural leads such as the duplicated `buffer_too_long`, the six-parameter `estimate`, and the test-only flag
    - forced-window repairs recurring with different causes each time
    - `realtime_long_speech` returns early when its directory is unset, which the new CI step prevents
    - the remaining `run_socket` returns are transport exits
  - False, 2: `prompt_terms` and the backend both count bytes, and `text_start` cannot pass the next overlap because a compile-time assertion in `crates/gateway/stt/api/src/segment/endpoint.rs` bounds the stride.
  - Unrelated pre-existing, 1: sibling panics in `process_closed` and `compact_to`.

</product-contract>
<implementation-contract>

## Technical Design

- D1-1, successor cut without a punctuation snap:
  - Module: `crates/gateway/stt/api/src/take/agreement-projection.rs` and `crates/gateway/stt/api/src/take/state/reconcile.rs`, both private to the take module.
  - The sparse arm of `estimate` must cut the successor text at the projected token itself, with no punctuation search. Ties already round toward the earlier token (`nearest_ties_earlier`), so the remaining error direction is a repeated word at the seam and not a lost one.
  - The predecessor arm keeps the forward-only snap exactly as it is, so `projected_prefix_end` and `locate_cut` still serve it unchanged. Choose between a mode argument on `projected_prefix_end` and a second small function that shares `count_tokens` and `project_tokens`. Prefer whichever changes fewer call sites and test sites, since the projection unit tests call `projected_prefix_end` directly.
  - The sparse warning in `estimate` also logs the cut position (`kept.metrics.selected_tokens`) next to the existing token counts, so a lost or repeated word at a seam can be traced. Log no transcript text.
  - No public interface, wire format, persisted data, trust boundary or dependency direction changes.
- D1-2, README:
  - Edit `crates/gateway/stt/README.md` only. Rewrite the CI sentence at line 80 to say the `base.en` and `small.en` pair loads only in the long-speech step. Add that step to the list of native steps in the same bullet.
  - On lines 33 and 58, and on the "Decoded text" row near line 43, add "unless a forced window is pending, in which case the history is empty" where the text says final decodes are conditioned on the decoded history.
  - Keep line 47, which already says pending text never enters a final decode's prompt.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused:
  - D1-1: in `crates/gateway/stt/api/src/take/state/alignment_tests-adversaries.rs`, beside `a_sparse_successor_keeps_every_predecessor_word`, add a test with the same ranges and a punctuated successor "gamma delta epsilon zeta, eta".
    - Geometry from the existing sparse tests: successor decode range 32_000 to 235_200 samples and overlap end 160_000. The projection is 128_000 of 203_200 samples over 5 tokens, which rounds to token 3.
    - Today the single punctuated token "zeta," falls inside the 3 tokens after the projection, so the cut lands after "zeta," and the kept tail is "eta". After the change the cut stays at token 3 and the kept tail is "zeta, eta", with the comma. The completed text is the predecessor followed by "zeta, eta".
    - Before editing code, confirm from the existing sparse tests that those ranges and rounding apply, and confirm the new test fails. If it already passes, D1-1 is false: keep the test and skip the code change.
  - D1-1 guard: a successor with two punctuated tokens in the band already falls back to the projected cut and passes. Keep it to pin that no snap applies to the sparse arm.
  - D1-2: no test applies to prose.
- Integration and regression:
  - These tests pin the predecessor arm and must keep passing unchanged: `locate_cut_never_picks_a_token_before_the_projected_one`, `a_clause_boundary_before_the_projected_token_never_drops_predecessor_words`, the "its place is to" geometry test, `a_sparse_successor_keeps_every_predecessor_word`, `a_one_word_successor_keeps_every_predecessor_word`, `a_sparse_successor_leaves_pending_text_over_only_the_audio_after_the_overlap`, the exact-one-third and just-under-one-third pair, and `a_forced_window_after_a_sparse_settlement_projects_over_only_the_kept_tail`.
  - No new compiler check, ratchet or native gate is proposed. The unit test is cheaper than a small-run reporting mode in the native test.
- Exit checks:
  - `cargo nextest run --locked -p gateway-stt --all-features` passes.
  - `cargo fmt --all --check` passes.
  - Clippy on `gateway-stt` with `--all-targets --all-features` and warnings denied passes.
  - When the CUDA build and the long-speech fixtures are available on the machine, `realtime_long_speech` still reports no run of 5 or more missing words on any of its six clips. It was passing at `14324ac77`.
  - Read the edited README rows against `TakeState::decoded_text` in `crates/gateway/stt/api/src/take/state.rs` and against the long-speech step in `.github/workflows/stt-miri.yml`.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - D1-1 remedy: cut the successor with no punctuation snap. Rejected: a backward-only snap, which still repeats up to 3 words and adds code. Rejected: accepting the loss and recording it, which contradicts the sparse branch's stated goal, because the accepted "at worst repeat up to 3 words" reasoning in the earlier plan holds for the predecessor cut only. Rejected: a small-run reporting mode in the native test, which adds machinery where a unit test is cheaper. Tradeoff: the cut can land mid-clause and repeat at most about one word at a seam. Verification: the new unit test plus the pinned predecessor-arm tests.
  - D1-2 remedy: edit the README. No code or test covers prose, so the check is reading the rows against the code and the workflow.
- User-resolved architecture choices: none. No remediation changes a public interface, persisted or wire format, component ownership, dependency direction or trust boundary.
- Rejected alternatives, assumptions, and risks:
  - The analysis read code and history and ran nothing. D1-1 rests on a hand trace of `project_tokens` and `locate_cut` that two independent readers reproduced. How often exactly one punctuated token sits in the band is unknown, and whisper tends to punctuate short text.
  - The proportional estimate is a heuristic, so any cut can be off by a word in either direction. This plan only removes the bias toward losing words.
  - Work runs in the `promptforge2` worktree on branch `vibe2`, with no new branch, and nothing is pushed.

### Deferred and Out of Scope

- Sibling `transfer_range` panics in `process_closed` and `compact_to` in `crates/gateway/stt/api/src/take/finalization.rs`. They exist at the baseline and the target did not change them. Revisit if a reachable sequence is shown where the cap cannot hold the fallback copy for a queued range.
- Exporting the backend's glossary limit so `crates/gateway/stt/api/src/guidance.rs` does not restate 800. Revisit if the backend limit changes.
- The Workshop UI rolling back the active take when a gateway-ended session sends an id-less error event. Revisit if gateway-ended sessions become common.
- Raising the skipped-caption log above debug level. Revisit if paused captions need diagnosing in the field.
- The stale short "Authoritative transcription failed" example in the shared wire fixture `server-events.json`. It is an example, not a defect.
- Out of scope: merging or pushing `vibe2`, the first run of the new CI job on GitHub, and any change to the density threshold or the 5-word gate.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <package>`, for example `cargo build --locked -p gateway`. Plain `cargo build` builds only the default member `gateway`; the desktop app is explicit (`cargo build -p workshop`). The headless gateway shape is checked with `cargo check -p gateway --no-default-features`, the one standalone `cargo check` the repo allows beside clippy.
- Focused test command pattern: `cargo nextest run --locked -p <package> <test-name-filter>`. Add `--lib` for unit tests or `--test <target>` for one integration binary, for example `cargo nextest run --locked -p gateway-stt --lib <filter>` or `cargo nextest run --locked -p gateway-stt --test it <filter>`. Native tests are `#[ignore]`d and need the `PROMPTFORGE_WHISPER_*` and `PROMPTFORGE_SILERO_MODEL` environment variables; run them one at a time with `--run-ignored only --test-threads 1`. UI packages use `node --test <file>` from the UI package directory.
- Component test command pattern: `cargo nextest run --locked -p <package>`; for the Workshop crates the group is `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. UI packages: `npm test` in `crates/gateway/config-ui/ui`, and `npm test --workspaces --if-present` in `crates/workshop`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `cargo test -p build-xtask`'s boundary and structural checks through `--workspace`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, and for Workshop `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. Per-package pattern: `CARGO_BUILD_WARNINGS=deny cargo clippy -p <package> --all-targets --all-features`, without `--all-features` for the Workshop crates. On this machine's PowerShell, set the variable first: `$env:CARGO_BUILD_WARNINGS="deny"`. Workspace lints deny `unwrap_used`, `expect_used`, `allow_attributes`, and `unsafe_code`; `clippy.toml` allows `unwrap` and `expect` in tests.
- Formatter check command: `cargo fmt --all --check`. It is also the git pre-commit hook, `rustfmt.toml` sets `style_edition = "2024"`, and `rust-toolchain.toml` pins `stable`.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS="-D warnings"`). Also the facade gate `cargo doc -p promptforge --no-deps` with the same flags, and `cargo +<pinned nightly> xtask api --check` when the `promptforge` facade surface changes; the nightly is named in `crates/build-xtask/src/api/toolchain.rs`.
- Test placement and naming conventions:
  - Unit tests sit beside the code as `#[cfg(test)] mod tests;`. The body lives in `tests.rs` inside a subdirectory module, or in a kebab sibling such as `guidance-tests.rs` and `pcm-tests.rs` wired with `#[path = "guidance-tests.rs"]`. Snake-case names such as `alignment_tests.rs` and `detector_tests.rs` also exist, so match the neighbouring files.
  - Integration tests are one binary per crate at `crates/<crate>/tests/it/main.rs` with a module per area (`realtime_session.rs`, `replay.rs`), shared helpers in `tests/common/mod.rs`, and data in `tests/fixtures/`. Some crates use one file per target instead, such as `native_whisper` and `native_silero` in `backend-whisper`.
  - Test support lives behind a `test-fixtures` Cargo feature in `test_fixtures` modules; a crate enables its own feature through a self dev-dependency. Docs and comments call this "test support" or "fixtures", never a bare "harness".
  - Test names are long snake_case behavior sentences, for example `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. Behavior changes ship with their tests in the same change.
  - UI tests are `*.test.mjs` files run by `node --test`; no workflow runs the `tools/*.test.mjs` files.
  - Nextest puts the STT crates `gateway-stt` and `gateway-stt-backend-whisper` in a `heavy` test group, and gives the `realtime_stt::noise::` and `realtime_stt::capture::` gateway tests a longer timeout.
- Directory map:
  - `crates/` holds every Rust crate. Flat crates: `promptforge` (the Engine facade), `promptforge-plugin`, `harness`, `harness-gateway-client`, `plugin-mcp`, `plugin-user-input`, `plugin-web`, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `build-*` (build and structural tooling), `workspace-hack` (cargo-hakari).
  - Manifestless containers hold the private crates of one family: `crates/promptforge-internal/` (types, parser, lua, vfs, model-client, engine), `crates/harness-internal/runner`, `crates/gateway/` (app, config, config-ui, routing, protocol, local, progress, logging, cloud-providers, web-search, and `stt/` with api, engine, backend-whisper, whisper-ffi), and `crates/workshop/` (desktop, server, server-api, protocol, registry, status, support, user-state, workspace, run-log, agents, menu, gateway, plus the `ui`, `look`, and `platform` npm packages).
  - `crates/shared-ui` is a TypeScript and CSS package, not a Rust crate.
  - `tools/` holds Node scripts for staging the gateway sidecar and live TTS checks. `prompts/` holds example prompts. `guide/` holds the user guide books (gateway, language, workshop), chrome, and landing page. `vibe/` holds dated design and plan records, one per piece of work. `.github/workflows/` holds CI and release workflows, `.githooks/` holds pre-commit (fmt) and pre-push (gateway headless check, clippy, cargo deny), and `.config/` holds nextest and hakari settings.
- Component boundaries:
  - Engine: `promptforge-types` and `promptforge-vfs` are leaves. `promptforge-model-client` depends on types. `promptforge-lua` depends on model-client, types, vfs. `promptforge-parser` depends on lua and types. `promptforge-engine` depends on all of them. `promptforge` re-exports the engine as the public facade. The Harness crates depend on that facade and on `promptforge-plugin`, which itself depends on `promptforge-types` and `promptforge-vfs`.
  - Harness: `harness-runner` depends on `promptforge` and `promptforge-plugin`; `harness` wraps it. `harness-gateway-client` depends on `harness`, `plugin-web`, and `promptforge`. Each `plugin-*` crate depends only on `promptforge-plugin`.
  - Gateway: `gateway-api-types` is the shared wire vocabulary. Dependencies run `gateway` (app) down to `gateway-local`, `gateway-routing`, `gateway-protocol`, `gateway-config`, with `gateway-progress` producing progress. STT runs `gateway-stt` (api) down to `gateway-stt-backend-whisper` and `gateway-stt-engine`, and the backend down to `gateway-whisper-ffi`. Consumers outside the gateway family read only the `Progress` wire type from `gateway-api-types`.
  - Workshop: dependencies flow one way, server then features then services then vocabulary, with `workshop-protocol` and `workshop-registry` as the shared vocabulary. Same-tier crates do not depend on each other. The desktop app `workshop` depends on `workshop-server-api`, never on `workshop-server`. `cargo test -p build-xtask` enforces the tier graph and product boundaries.
- Conventions summary:
  - Rust 2024 edition, one workspace version, every crate inherits `[lints] workspace = true` and `workspace-hack`; dependencies come from `[workspace.dependencies]`.
  - The words Engine, Harness, Host, and Plugin are capitalized and mean one thing each; inside `crates/gateway/stt/` a bare "engine" means the speech engine. Engine crates never mention the Host.
  - Source directories are flat; a subdirectory needs at least three files, and smaller groups use `foo-bar.rs` siblings with `#[path]`. `lib.rs` is a facade of docs, `mod` lines, and re-exports.
  - Every `workshop-*` crate doc carries a `## Invariants` marker. Comments explain non-obvious constraints and cite an upstream URL for workarounds. Error and status messages are written for model consumption: concise, with required versus actual.
  - Unsafe code stays in its owned boundary. Process-global installers (panic hook, logger, tracing subscriber, rustls provider) belong to binary entry points only. Runtime paths never compile native dependencies.
  - JSON that reaches a recorder round-trips exactly (`float_roundtrip`, sorted keys, finite numbers).
  - Workshop UI CSS uses custom properties from `@workshop/look`, and persisted UI values go through the `ui-storage` adapter to the server.
  - Plans and designs live in dated files under `vibe/`, named `YYYY-MM-DD-<n>-<slug>.md`.

</project-survey>
<execution-plan>

## Execution Instructions

Work in the `c:\Users\Vinnie\cursor\promptforge2` worktree on branch `vibe2`. Create no new branch and push nothing. The two steps are independent, so either order works. Each step is its own commit.

<step-1>

### Step 1: Cut the sparse successor at the projected token [completed]

- Component: none
- Work item: D1-1. Remove the forward punctuation snap from the sparse-successor cut so it never drops words that exist only in the successor's new audio.
- Artifacts:
  - `crates/gateway/stt/api/src/take/state/alignment_tests-adversaries.rs`: new test beside `a_sparse_successor_keeps_every_predecessor_word`.
  - `crates/gateway/stt/api/src/take/agreement-projection.rs`: `projected_prefix_end` and `locate_cut`.
  - `crates/gateway/stt/api/src/take/state/reconcile.rs`: the sparse arm of `estimate` (lines 43-60) and its sparse warning log.
- Work:
  1. Write the test first. Reuse the geometry of the existing sparse tests: successor decode range 32_000 to 235_200 samples, overlap end 160_000, successor text "gamma delta epsilon zeta, eta". The projection is 128_000 of 203_200 samples over 5 tokens, which rounds to token 3. First confirm from the existing sparse tests that these ranges and this rounding apply.
  2. Run the new test on the tree at `14324ac77`, before any code edit, and confirm it fails. Today the cut lands after "zeta," and the kept tail is "eta". The test must expect the kept tail "zeta, eta", with the comma, and a completed text of the predecessor followed by "zeta, eta". If it already passes, D1-1 is false: keep the test, skip the code and log edits below, commit the test alone, and flag it in the return.
  3. Make the sparse arm cut the successor at the projected token with no punctuation search. Choose either a mode argument on `projected_prefix_end` or a second small function that shares `count_tokens` and `project_tokens`, whichever changes fewer call sites and test sites. The projection unit tests call `projected_prefix_end` directly. The predecessor arm keeps the forward-only snap exactly as it is, so `locate_cut` still serves it unchanged.
  4. Add the cut position (`kept.metrics.selected_tokens`) to the existing sparse warning in `estimate`, next to the token counts it already logs. Log no transcript text.
  5. Keep a guard test for a sparse successor with two punctuated tokens in the 3-token band. It already falls back to the projected cut and passes, and it pins that no snap applies to the sparse arm. If an existing test already covers that case, leave it unchanged. Otherwise add it beside the new test.
- Verification:
  - The new test passes after the change.
  - These predecessor-arm and sparse tests pass unchanged: `locate_cut_never_picks_a_token_before_the_projected_one`, `a_clause_boundary_before_the_projected_token_never_drops_predecessor_words`, the "its place is to" geometry test, `a_sparse_successor_keeps_every_predecessor_word`, `a_one_word_successor_keeps_every_predecessor_word`, `a_sparse_successor_leaves_pending_text_over_only_the_audio_after_the_overlap`, the exact-one-third and just-under-one-third pair, and `a_forced_window_after_a_sparse_settlement_projects_over_only_the_kept_tail`.
  - Run them with `cargo nextest run --locked -p gateway-stt --all-features <filter>`.
- Commit: one commit containing the test, the code change, and the log change. Make no change to the predecessor cut, the one-third density threshold, the 5-word native gate, or the wire protocol.

</step-1>

<step-2>

### Step 2: Correct the README claims [completed]

- Component: none
- Work item: D1-2. Make the README state what CI runs and what a final decode is conditioned on, as the code at `14324ac77` does.
- Artifacts:
  - `crates/gateway/stt/README.md`, lines 33, 43, 58 and 80.
- Work:
  1. Line 80: rewrite the CI sentence so it says the `base.en` and `small.en` pair loads only in the long-speech step. Add that long-speech step (`realtime_long_speech`) to the list of native steps in the same bullet.
  2. Lines 33 and 58, and the "Decoded text" row near line 43: where the text says final decodes are conditioned on the decoded history, add "unless a forced window is pending, in which case the history is empty".
  3. Leave line 47 alone. It already says pending text never enters a final decode's prompt.
- Verification:
  - Read the edited rows against `TakeState::decoded_text` in `crates/gateway/stt/api/src/take/state.rs`, which returns an empty string while a forced window is pending, and against the long-speech step in `.github/workflows/stt-miri.yml`. Prose has no test.
- Commit: one commit containing only the README change.
- Exit checks. They create no commit, and this step's coding sub-agent runs none of them:
  - `cargo nextest run --locked -p gateway-stt --all-features`, `cargo fmt --all --check`, and clippy with warnings denied (`$env:CARGO_BUILD_WARNINGS="deny"` first on PowerShell, then `cargo clippy -p gateway-stt --all-targets --all-features`) are covered by this step's final verification.
  - After the last commit, the session running the plan runs the native test when the CUDA build and the long-speech fixtures are available: `cargo test --locked -p gateway-stt --all-features --test it realtime_long_speech -- --ignored --test-threads=1`, with `PROMPTFORGE_WHISPER_LIBRARY`, `PROMPTFORGE_WHISPER_MODEL` (`ggml-base.en.bin`), `PROMPTFORGE_WHISPER_FINAL_MODEL` (`ggml-small.en.bin`), `PROMPTFORGE_SILERO_MODEL`, `PROMPTFORGE_WHISPER_BACKEND=cuda` and `PROMPTFORGE_LONG_SPEECH_CLIPS` (the directory holding `clips.json` and the six WAV files) set. It must report no run of 5 or more missing words on any of its six clips, as it did at `14324ac77`. When the fixtures are not on the machine, skip it and say so in the report.

</step-2>

</execution-plan>