---
name: No-trace fence rule
overview: Replace the parser's named "removed fence form" errors with one general error for lua-prefixed fence labels, remove every trace of removed features from code, tests, lints, and comments, apply the appropriate code review fixes to the promptforge facade crate, and rebuild that crate's test suite on a small harness that cuts its size by about 40%.
todos:
  - id: parser-rule
    content: "One commit: lua-prefixed label scan replaces the two named checks, Fence docs rewritten, parser tests rewritten with neutral labels, facade fixture swapped"
    status: pending
  - id: retired-lints
    content: Trash the two retired_symbols files, cut the retired scan out of engine_guards.rs, its tests, and the tidy.rs doc, and delete the docs-claims RETIRED list
    status: pending
  - id: sweep
    content: Drop the always_offer absence clause; sweep crates/ and prompts/ for other removed-feature traces
    status: pending
  - id: json-module
    content: promptforge::json module, serde_json as a normal dependency, re-bless public-api.txt, example JSON import and timer fix
    status: pending
  - id: review-fixes
    content: Positive-fixture test and its directory walker copied to the engine suite, tool-offering wording
    status: pending
  - id: harness
    content: Suite harness (prompt builder, preloaded answers, Scenario -> Driver -> Trace) plus greeter.rs migration and the line-budget checkpoint
    status: pending
  - id: harness-migrate
    content: Migrate the remaining suite files in two commits; same test names; line budget met
    status: pending
isProject: false
---

# No-Trace Fence Rule, Review Fixes, and Facade Test Harness

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\promptforge` (git). Paths are relative to it unless they start with `../`.
- Baseline: `7476ed81d` (`Close plan: remove-hakari`), equal to `origin/master`. Every citation was checked there; search by name if a line has moved.
- Shell: PowerShell on Windows. Run cargo commands one at a time, since they share the target directory's lock, and don't append `2>&1`. Judge a cargo step by its result lines and `$LASTEXITCODE`.
- Before editing a crate, read the root `AGENTS.md` and that crate's `AGENTS.md` if it has one (engine, lua, types, and vfs do).
- Plan record: when execution starts, copy this plan to `vibe/` under the next free `YYYY-MM-DD-N-no-trace-fence.md` name for that day, and write that path as the only line of `vibe/ACTIVE`, which doesn't exist today. Mark each work item completed in that copy as it lands. Close with a commit `Close plan: no-trace-fence` that removes `vibe/ACTIVE`. Nothing is pushed.
- Terms:
  - Engine, Harness, Host, and Plugin are as `AGENTS.md` defines them.
  - *The facade* is `crates/promptforge`, and *the facade suite* is `crates/promptforge/tests/suite/`.
  - *The test harness* is the facade suite's new support code. It has nothing to do with the Harness.
  - A fence's *label* is its info string, the text after the opening backticks. A *label near-miss* is a label like `lua extra` or `Lua`. A *shape near-miss* is an opener labeled exactly `lua` but malformed, such as indented or with four backticks.

<product-contract>

## Product Requirements

PromptForge drops every trace of removed features. The parser stops naming removed fence forms and instead rejects any `lua`-prefixed fence label that is not exactly `lua`. The lints and tests that remember removed names go away. The same change applies the appropriate code review fixes to the `promptforge` facade crate and rebuilds its test suite on a small harness so the suite is much smaller.

- Problem and users:
  - Prompt authors get bespoke "was removed" errors for `lua shared` and `lua prompt` (`crates/promptforge-internal/parser/src/parse.rs`, `crates/promptforge-internal/parser/src/fence.rs`), while other unknown labels such as `lua extra` or `Lua` are silently kept as prose.
  - Maintainers keep removed names alive through a retired-symbol lint (`crates/build-xtask/src/engine_guards.rs`, `crates/build-xtask/src/retired_symbols.rs`), a retired-phrase list (`crates/workshop/ui/test/docs-claims.mjs`), and an absence assertion (`crates/promptforge-internal/lua/src/tools/tests-offering.rs`).
  - Readers of the facade suite (`crates/promptforge/tests/suite/`, about 2,350 lines) wade through the same parse, prepare, and drive plumbing rewritten in nearly every file.
- Goals:
  - One general fence-label error that knows nothing about any removed form.
  - No removed-feature names in code, comments, tests, or lints under `crates/` and `prompts/`.
  - The appropriate review fixes for the facade crate, listed under Technical Design.
  - A facade suite of about 1,450 lines or fewer, harness included, with every test kept. The measure is the raw line count of `crates/promptforge/tests/suite/**/*.rs`, which is 2,349 at the baseline.
- Non-goals:
  - Rewriting the language guide in the separate `promptforge-docs` repository.
  - Editing the dated plan logs under `vibe/`.
  - A harness for `crates/promptforge-internal/`, which is deferred.
- Success criteria:
  - `rg -n -g '!target' -g '!vibe/**' "lua shared|lua prompt|RETIRED|retired.symbol|always_offer" crates prompts` returns no hits. Today it hits 15 files, all named in the work items. The `.` matters: it also catches the hyphenated "retired-symbol".
  - Every suite and check in the exit criteria passes, and the facade suite keeps its test names.
  - The facade suite meets the line budget.
- Constraints:
  - No backward compatibility and no trace of removed features, in code, tests, lints, or comments.
  - A file leaves the repository by moving to the workspace's `cabinet/_trash/` (`../cabinet/_trash/` from the repository root), never by deletion.
  - The facade suite uses `promptforge::` paths only (`crates/promptforge/tests/suite/main.rs`).
  - Every source file stays under the per-file line ceiling that `build_ceiling::check` enforces (`crates/promptforge/build.rs`).
  - Clippy runs with `-D warnings`, so every harness helper must be used. Each helper lands in the commit that first uses it, so no commit leaves dead code.
- Open questions: None

## Functional Specification

Prompt authors see one new parse error, Rust callers gain a `promptforge::json` module, and maintainers lose the retired-name lints. Everything else changes only tests.

- Actors and workflows:
  - A prompt author parses a prompt with `Prompt::parse`.
  - A Rust caller builds tool descriptors and reads tool arguments through the facade.
  - A maintainer runs the workspace checks.
- Inputs and outputs:
  - Input: a prompt source containing fenced code blocks.
  - Output: a parsed `Prompt`, or a `ParseError` of kind `Fence`.
- States and validation:
  - Invalid: a top-level fence whose label starts with `lua` in any letter case but is not exactly `lua`, such as `lua shared`, `lua prompt`, `lua extra`, `Lua`, `LUA`, or `luau`.
  - Unchanged and valid: other-language fences such as `json` and bare fences stay prose; fences nested inside a longer fence stay prose; openers labeled exactly `lua` but malformed in shape, such as indented or four-backtick openers, stay prose.
  - The check covers the whole body, including notes before the H1, and fails the parse before any Lua compiles.
- Errors and recovery:
  - The first offending fence fails the parse with kind `Fence`, located at its opening line, with a message in this form for the label `lua extra`: ``unrecognized fence label `lua extra`; the only Lua fence label is `lua` ``.
  - No message mentions a removed form. The author recovers by fixing the label.
- Security and privacy behavior: None
- Acceptance criteria:
  - Each invalid label above fails with kind `Fence` and that message; each valid case parses as it does today.
  - `promptforge::json::Value`, and `promptforge::json::json!` when the API check admits it, are usable from the facade alone.
  - The `build-xtask` structural tests and `docs-claims.mjs` pass with the retired-name lints gone.

</product-contract>
<implementation-contract>

## Technical Design

The parser replaces two named checks with one label scan. The facade gains a JSON module and a corrected example. The retired-name lints are deleted outright. The facade suite gets a three-part harness that turns setup and driving into data, so each test keeps only its assertions.

- Architecture:
  - Parser:
    - One scanner reads each top-level fence's info string from the pulldown-cmark event stream. `crates/promptforge-internal/parser/src/fence.rs` already uses that stream so that fence lines nested in longer fences stay prose.
    - `Prompt::parse` returns the first unrecognized label as a located `Fence` error.
    - The leading `lua prompt` check in `split_h1` and the `exact_shared_openings` scan are deleted.
    - The `Prompt::parse` doc (`crates/promptforge-internal/parser/src/parse.rs`) and the `Fence` variant doc (`crates/promptforge-internal/parser/src/error.rs`) state the rule without naming removed forms.
  - Lints:
    - The retired-symbol scan and its seed list go, together with their tests. The rest of the Engine guards stay.
      - Moved to `cabinet/_trash/`: `crates/build-xtask/src/retired_symbols.rs` and `crates/build-xtask/src/retired_symbols-tests.rs`.
      - Edited, not deleted, because `tidy.rs`, `api/load.rs`, `doc_hidden.rs`, and `test_support_leak.rs` use its `ENGINE_CONTAINER`, `collect_crates`, `engine_crates`, and `engine_guard_violations`: `crates/build-xtask/src/engine_guards.rs` loses `RETIRED_SEEDS`, `retired_symbol_violations`, its call in `engine_guard_violations`, and the scan's mentions in the module and function docs.
      - `crates/build-xtask/src/engine_guards-tests.rs` loses `engine_sources_name_no_retired_symbols`, `a_retired_symbol_reintroduced_in_an_engine_crate_fails_the_guard`, and the "retired symbol" clause in its module doc.
      - `crates/build-xtask/src/main.rs` loses `mod retired_symbols;`, and the `crates/build-xtask/src/tidy.rs` module doc drops "retired-symbol scan" from its list.
    - The `RETIRED` phrase list and its loop leave `crates/workshop/ui/test/docs-claims.mjs`. The lowercase-"host" check in that file still catches the lowercase forms of those phrases. Capitalized forms such as "the Engine's Host" are no longer banned.
  - Facade suite test harness, in `crates/promptforge/tests/suite/support.rs` and `crates/promptforge/tests/suite/support/scenario.rs`:
    - Prompt builder:
      - `prompt(front, &[(heading, lua)])` writes fixed frontmatter (`name: greeter`, a fixed description, `promptforge: 0`), then the `front` lines, then `# Greeter`, then one `lua`-fenced `## heading` section per entry.
      - `greeter(lua)` is the one-section `## Greet` case.
      - The builder trims every line, so sources can be indented raw strings, which Lua and flow-style YAML both accept.
    - Preloaded answers:
      - The reply constructors are `text`, `calls`, and `fail`.
      - A tool is an id plus a handler; the provided ones are `shout()` and `Tool::untrusted(id, text)`.
      - `canned()` is the default model, and `TestResult` is the shared result alias.
    - `Scenario`, `Driver`, `Trace`:
      - `Scenario` holds a test's setup as data: `name`, `args`, `started_at`, `model`, `tool`, `replies`, `hold`, `reverse`, and a `context` lambda that edits the `RunContext`. It offers `prepare()`, `start()`, and `run()`.
      - `Driver` steps a run by hand for tests that act mid-run: `pump`, `pump_until_held`, `finish`.
      - `Trace` holds the result, the event log (parse events followed by run events), and every effect record paired with its answer record and provenance.
- Modules and interfaces:
  - Harness driving rules:
    - A store effect gets `perform_vfs_op`, and a timer gets `EffectAnswer::Timer` at once.
    - A chat effect gets the next scripted reply, served under the bound model's name, and a tool call gets its registered handler.
    - An effect matching `hold` waits, and gets `Dropped` once the run is decided or the effect is withdrawn. The harness never answers `Dropped` otherwise.
  - The harness fails the test, instead of hanging, in three cases: the chat script runs out, scripted replies are left over at `Done`, or an undecided run issues nothing new while only held effects remain.
  - Every scenario defaults to `canned()`, sets `provenance_start` to the parse-event count, and always calls `prepare`, failing when prepare reports a gap.
- File and public API changes:
  - `promptforge::json` re-exports `serde_json::Value`, and also `serde_json::json` if `cargo xtask api` admits a macro.
    - `serde_json` moves from `[dev-dependencies]` to `[dependencies]` in `crates/promptforge/Cargo.toml`.
    - The facade shape check admits only single-item re-exports from declared dependencies (`crates/build-xtask/src/facade_shape.rs`), and `serde_json` is already on the API allowlist (`crates/build-xtask/src/api/items.rs`).
    - `crates/promptforge/public-api.txt` is re-blessed with `cargo +nightly-2026-09-05 xtask api --bless` (that toolchain is installed), with the new module as its only change.
  - The `greeter` example (`crates/promptforge/examples/greeter.rs`) imports JSON from `promptforge::json` and answers a timer with `EffectAnswer::Timer` at once, without sleeping; its `answer` doc says that a caller with a real clock waits `seconds` before answering. It otherwise stays raw API code.
  - The `RunContext::tools` doc (`crates/promptforge-internal/engine/src/execute/config.rs`) and the matching comment in `crates/promptforge/tests/suite/prepare.rs` say that the catalog lists every tool a run can use, Lua calls one by id, and a section offers one to its model with `tools.offer` (`crates/promptforge-internal/lua/src/tools.rs`).
- Data, persistence, failure, security, and privacy constraints: None

</implementation-contract>
<verification-contract>

## Testing Plan

Parser tests pin the new rule using only neutral labels. The facade suite proves the harness without losing a test. The workspace checks prove that the lints, the API listing, and the docs agree with the code.

- Unit:
  - Parser tests (`crates/promptforge-internal/parser/src/tests/h1_fences.rs`, `crates/promptforge-internal/parser/src/tests/section_fences.rs`, `crates/promptforge-internal/parser/src/tests/line_mapping.rs`, `crates/promptforge-internal/parser/src/tests.rs`):
    - Every test that names a removed form is deleted.
    - New tests use only neutral labels such as `lua extra`, `Lua`, and `luau`. They cover the error in the H1, in an H2, and before the H1, its reported line, failure before any Lua compiles, and a nested label staying prose.
    - Label near-misses now expect the error. Shape near-misses and `non_lua_fence_stays_in_prose` still expect prose.
  - `crates/promptforge-internal/lua/src/tools/tests-offering.rs` drops its `tools.always_offer == nil` clause.
- Integration and end-to-end:
  - In `crates/promptforge/tests/suite/parsing.rs`, the `removed-lua-prompt.md` row and fixture leave, and an `invalid/unrecognized-lua-label.md` fixture using `lua extra` checks for kind `Fence` and the fragment `unrecognized fence label`.
  - Every facade suite file except `parsing.rs`, `tools.rs`, `metrics.rs`, and `auto_traits.rs` moves onto the harness, keeping each test's name and assertions.
  - `every_positive_fixture_parses_offline` moves from `crates/promptforge/tests/suite/shipped.rs` into `crates/promptforge-internal/engine/src/execute/tests/suite/parsing.rs`, reading `crates/promptforge-internal/engine/tests/prompts/valid` and `crates/promptforge-internal/engine/tests/prompts/execution`.
    - The engine suite has no directory walker (its tests load named files with `include_str!`), so `collect_markdown` and `every_prompt_under_parses` are copied over with it. They port unchanged, because the engine suite's `crate::parser::Prompt::parse(source, name).0` matches the facade's call.
    - `shipped.rs` keeps its copy for `every_shipped_prompt_parses_offline`, and its module doc drops the clause about the Engine's positive fixtures.
- Regression, security, and performance:
  - `cargo nextest list -p promptforge` shows identical test names before and after the harness migration. The snapshot is taken after the fixture test has moved.
- Exit criteria (the commands are the verification list in `AGENTS.md`):
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` passes; it includes the parser, facade, engine, Lua, and `build-xtask` suites and the structural checks.
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` passes. This is now the only Workshop test run; the Windows Workshop steps were merged into one nextest command (`b4a5fc823`) and the `workshop-server` headless build and its test step were deleted (`8bfa189c2`).
  - Both clippy runs pass with `CARGO_BUILD_WARNINGS=deny`, together with `cargo check -p gateway --no-default-features`.
  - `cargo fmt --all --check` passes.
  - The workspace docs gate and the facade docs gate (`cargo doc -p promptforge --no-deps`) pass with `RUSTDOCFLAGS="-D warnings"`.
  - `cargo xtask api --check` passes on the nightly named in `crates/build-xtask/src/api/toolchain.rs`, against the re-blessed `crates/promptforge/public-api.txt`.
  - `node --test crates/workshop/ui/test/docs-claims.mjs` passes.
  - The success-criteria search returns no hits, and the facade suite meets the line budget.

</verification-contract>
<decision-record>

## Decision Record

The user set a no-trace rule, chose a narrow fence-label error, kept the docs repository and plan logs out of scope, asked for the appropriate review fixes, approved a harness to shrink the tests, and moved the rulebook-audit fixes to a separate plan. The remaining calls follow from those choices and from the repository's existing checks.

- Decisions:
  - No trace of removed features in code, tests, lints, or comments, and no compatibility handling. The user said: "I do not want ANY backward compatibility and I want NO TRACE of removed features. Not in tests. Not in lints. Not in comments. Not in compatibility."
  - An unrecognized fence label is a parse error, so removed forms need no handling of their own. The user said: "unrecognized label fence should be an error".
  - The error covers only `lua`-prefixed labels, and nested fences stay prose. The user picked both options when offered the choices.
  - Apply every appropriate review fix. The user said: "I want all the fixes from the code review which are appropriate".
  - Build a test harness with a small DSL to cut lines. The user said: "The goal here is to reduce the amount, number of lines of code... maybe making like a little domain-specific language", and approved the design with "this sounds very good, update the plan".
  - Keep the plan lean. The user said: "keep the plan lean and effective".
  - No new auto-trait checks. The review asked for them on the belief that nothing pinned the facade's auto traits, but `crates/promptforge/public-api.txt` records every facade type's auto traits on an `auto` line (for example `auto promptforge::Prompt: Freeze, RefUnwindSafe, Send, Sync, Unpin, UnwindSafe`), and `cargo xtask api --check` fails on any change (`crates/build-xtask/src/api/listing.rs`). `crates/promptforge/tests/suite/auto_traits.rs` stays as it is.
  - The rulebook-audit fixes, eleven commits with one per audit unit, belong to a separate plan. The user said: "we will do those 11 commits in a separate plan."
  - Harness choices:
    - Every scenario defaults to the canned model, because it does nothing without declared roles and can be overridden.
    - Tests stay separate, with sentence names, so one failure names one test.
    - The example stays raw API code, because it is the user-facing run loop.
    - The builder is named `Scenario`, because "Harness" names a product component in this repository.
- Rejected alternatives:
  - Erroring on every non-`lua` fence: prompts show models JSON and code examples in fences. Revisit if authors ask for strict fences.
  - Erroring on malformed `lua` shapes or on nested fences: the user did not choose these. Revisit if authors are confused by silent prose.
  - A `removed-lua-shared` parse fixture: it would preserve a trace of a removed feature.
  - Re-exporting the whole `serde_json` crate: the facade shape check admits only single-item re-exports (`crates/build-xtask/src/facade_shape.rs`). Revisit if that check changes.
  - Merging tests into tables across files: one failing case would fail a whole group. Revisit if the suite stops being organized as per-file tours.
  - Adding `Send` and `Sync` assertions for `Prompt`, `Step`, `Effect`, and `EffectAnswer`: `crates/promptforge/public-api.txt` already pins them. Revisit if the API listing stops recording auto traits.
- Assumptions, risks, and notes:
  - Risk: `cargo xtask api` may refuse a re-exported macro. If it does, `promptforge::json` re-exports `Value` alone and the final report says so.
  - Note: the workspace has no `workspace-hack` crate and no cargo-hakari since `76ee63f3c`, so moving `serde_json` into the facade's `[dependencies]` needs no generated-crate step.
  - Assumption: the harness needs no new crates, and the line budget assumes about 300 lines of harness code.
  - Note: tests that serialize logs keep calling `serde_json` directly, which is now a normal dependency of the crate.
  - Note: the docs site picks up the new `Fence` wording on the next upstream push.

### Deferred and Out of Scope

- Deferred: a shared harness for `crates/promptforge-internal/`. Revisit after the facade harness lands, as its own plan that starts with an inventory of local helpers.
  - The engine tests (about 31,500 lines under `crates/promptforge-internal/engine/src/`) already have drivers and fixtures in `crates/promptforge-internal/engine/src/test_support.rs`. They wrap them in per-file context builders, prompt builders, and drive wrappers, and many of them deliberately work below `Run`.
  - The Lua tests repeat VM builders, including two versions of `lua_over` (`crates/promptforge-internal/lua/src/plugins-tests.rs`, `crates/promptforge-internal/lua/src/tools/tests.rs`).
  - The parser tests repeat one frontmatter header, but some of them depend on exact layout.
- Deferred: the rulebook-audit fixes, as eleven commits with one per audit unit, in their own plan after this one lands.
  - That plan works from the merged audit report of 110 findings, not the raw per-unit files, and gives each cross-crate finding to the commit for the crate that defines the type.
  - Several of its findings touch files this plan edits, and this plan leaves them alone: the `f64` timer duration that the `greeter` example and the harness answer, `#[non_exhaustive]` on facade types, and the duplicated engine and Lua test setup, which overlaps the internal-crates harness deferred above.
- Out of scope: the language guide in the `promptforge-docs` repository. The user said: "leave it alone".
- Out of scope: the dated plan logs under `vibe/`. The user chose to leave them.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member). The UI bundles are built by crate build scripts, so run `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first. The desktop app is a separate, explicit build: `cargo build --locked -p workshop`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>`; drop `--all-features` when the package is `workshop`, `workshop-server`, or `workshop-server-api`. For one Workshop UI test file: `node --test crates/workshop/ui/test/<name>.mjs`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features` (again without `--all-features` for `workshop`, `workshop-server`, `workshop-server-api`). Workshop UI packages: `npm test --workspace ui` from `crates/workshop`. Gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. CI also runs `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`.
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`. Companion gates: `cargo check -p gateway --no-default-features` (headless build shape), `cargo test -p build-xtask` (boundary and structural checks), `node --test crates/workshop/ui/test/docs-claims.mjs` (wording and rulebook scan), `cargo +<pinned nightly> xtask api --check` (facade surface, nightly named in `crates/build-xtask/src/api/toolchain.rs`), and `npm run typecheck --workspaces --if-present` in `crates/workshop`. Never run a standalone `cargo check --workspace` beside clippy.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus the facade build with default features: `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`. CI additionally documents `harness` with default features and `promptforge-engine` and `workshop-server` with `--document-private-items`.
- Test placement and naming conventions: Unit tests live in a sibling file `<module>-tests.rs`, wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`; a few modules still use an inline `mod tests { }`, and `promptforge-lua`, `promptforge-parser`, and the `harness-gateway-client` transport keep `src/tests/` directories. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`promptforge`, `harness`), with one `mod` file per area plus a `support` module; data goes in `tests/fixtures/`, shared helpers in `tests/common/`, and Engine prompt fixtures in `tests/prompts/`. A few crates keep standalone `tests/<name>.rs` binaries (`gateway-stt-engine`, the `native_*` files of `gateway-stt-backend-whisper`, `build-workshop`, `gateway-cloud-providers`). Test functions are snake_case sentences that state the behavior, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. Tests needing native whisper assets are `#[ignore]`d with a `native_` prefix; Miri-run tests carry a `miri_` prefix. Workshop UI tests are `node:test` plus jsdom files at `crates/workshop/ui/test/<kebab-name>.mjs` or `src/**/*.test.mjs`; Node tool scripts sit beside `tools/<name>.test.mjs`.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `promptforge` (the Engine's public API facade), `promptforge-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, the gateway vocabulary and discovery crates `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package, not a Rust crate), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/promptforge-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`), `crates/gateway/` (the `app` binary, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and `stt/` with `api`, `engine`, `backend-whisper`, `whisper-ffi`), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates, and the npm workspace members `ui`, `look`, `platform`). Outside `crates/`: `guide/` is the user guide source (books for gateway, language, workshop, plus `chrome` and `landing`); `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans; `tools/` holds Node scripts for staging the gateway sidecar and a live TTS check; `images/` holds README and site art; `.github/workflows/` holds CI (`ci.yml`), release, nightly, Miri, site, and installer workflows; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets rust-lld and the static CRT on Windows plus the `xtask` and `workshop` aliases; `.config/nextest.toml` sets timeouts and the heavy STT test group; `.cursor/rules/` holds the Workshop architecture and SPA rules. `local/`, `target/`, `target-msrv/`, and `cabinet/` are untracked.
- Component boundaries: Engine: the `promptforge` facade depends on all six `promptforge-internal` crates; inside, `promptforge-engine` depends on `promptforge-lua`, `promptforge-parser`, and `promptforge-model-client`, `promptforge-parser` depends on `promptforge-lua`, and `promptforge-types` and `promptforge-vfs` are leaves. Plugin API: `promptforge-plugin` depends on `promptforge-types` and `promptforge-vfs`, and each linked Plugin depends only on `promptforge-plugin`. Harness: `harness` depends on `harness-runner`, `promptforge`, and `promptforge-plugin`; `harness-runner` depends on `promptforge` and `promptforge-plugin`; `harness-gateway-client` depends on `harness`, `plugin-web`, and `promptforge`. Gateway: the `gateway` app sits on top of its family crates, the family reaches outside only through `gateway-api-types`, `gateway-api-discovery`, and the `shared-*` crates, and nothing in it depends on the Engine or Harness; within STT, `gateway-stt` depends on `gateway-stt-engine` and `gateway-stt-backend-whisper`, which depends on `gateway-whisper-ffi`, the subsystem's only `unsafe` code. Workshop: tiers flow server, then features, then services, then vocabulary; `workshop-server` is the composition root and depends on every Workshop crate plus `harness`, `harness-gateway-client`, the Plugins, and `promptforge`; the desktop package `workshop` depends only on `workshop-server-api` and `gateway-api-discovery`; `workshop-protocol` and `workshop-support` are leaves and `workshop-registry` depends only on `workshop-protocol`; Workshop reaches the gateway only through `gateway-api-types` and `gateway-api-discovery`. The `build-*` crates have no workspace dependencies. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` outside tests, bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings; `clippy.toml` bans process-global installers outside binary entry points. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Each Workshop crate's `lib.rs` is a facade whose `//!` doc carries a `## Invariants` section listing allowed dependencies. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints, and workarounds cite their upstream issue URL. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. Cargo features gate real build constraints, not product shape. Workshop UI CSS uses design tokens only, and persisted UI values go through the `ui-storage` adapter. npm members get an `eol=lf` line in `.gitattributes`.

</project-survey>
<execution-plan>

## Execution Instructions

Four components, eight steps, one commit per step, in the order below. Every commit compiles, passes the suites it touches, and leaves no dead code, since clippy runs with `-D warnings`. Steps 2, 4, and 5 depend on nothing earlier and could land in any order; the order here is the commit order. Paths are relative to the repository root.

- Components, in dependency order:
  1. Fence label rule (Step 1). First, because the parser's named removed-form errors and their tests are most of the hits for the success-criteria search, and the sweep in Step 3 should start from a tree where they are gone.
  2. Trace removal (Steps 2 and 3). Second, because the lints are self-contained and the sweep is only meaningful once the parser and lint traces are gone.
  3. Facade review fixes (Steps 4 and 5). Third, because the harness imports JSON through `promptforge::json`, and the test-name snapshot taken at the start of Step 6 must come after the positive-fixture test leaves `shipped.rs`.
  4. Facade test harness (Steps 6 to 8). Last, because it builds on both facade fixes and touches only the facade suite.
- Pieces and construction:
  - Fence label rule: the scanner and docs, the parser tests, and the facade fixture are built jointly in one step, because the old tests and the old fixture row assert the old "fence form was removed" messages and fail the moment the rule changes.
  - Trace removal: the `build-xtask` lint removal and the `docs-claims.mjs` list removal are built jointly, since both delete retired-name lists and neither has other users. The sweep follows sequentially, so its search sees only what Steps 1 and 2 left behind.
  - Facade review fixes: the JSON module, then the positive-fixture move with the tool-offering wording. They share no code, so they are sequential only to keep commits small.
  - Facade test harness: the harness core with the `greeter.rs` migration, then two migration batches. Sequential, because each batch adds helpers on top of the earlier ones and the `greeter.rs` budget checkpoint gates the rest.

<step-1>

### Step 1: Fence label rule replaces the named removed-form checks [completed]

- Component: Fence label rule
- Tests:
  - `cargo nextest run --locked -p promptforge-parser --all-features`
  - `cargo nextest run --locked -p promptforge-engine --all-features`
  - `cargo nextest run --locked -p promptforge --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p promptforge-parser -p promptforge --all-targets --all-features`
  - `rg -n -g '!target' "lua shared|lua prompt" crates/promptforge-internal/parser crates/promptforge` (must print nothing)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.

The rule:

- Invalid: a top-level fence whose label (its info string, the text after the opening backticks) starts with `lua` in any letter case but is not exactly `lua`, such as `lua extra`, `Lua`, `LUA`, or `luau`. The removed forms fall under this rule and get no handling of their own.
- The first offending fence fails the parse with `ParseErrorKind::Fence`, located at its opening line, with this message for the label `lua extra`: ``unrecognized fence label `lua extra`; the only Lua fence label is `lua` ``. No message mentions a removed form.
- The scan covers the whole body, including notes before the H1, and fails the parse before any Lua compiles.
- Still valid and unchanged: other-language fences such as `json`, bare fences, fences nested inside a longer fence, and openers labeled exactly `lua` but malformed in shape, such as indented or four-backtick openers, which stay prose. A label near-miss errors whatever its opener shape whenever pulldown-cmark reports it as a fence, which includes four-backtick and one-to-three-space-indented openers.

Work:

- `crates/promptforge-internal/parser/src/fence.rs`: add one scanner that reads each top-level fence's info string from the pulldown-cmark event stream, the same `Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(_)))` stream the file already uses so that fence lines nested in longer fences stay prose, and returns the first unrecognized label with its line. Delete `exact_shared_openings` and the leading `lua prompt` check in `split_h1`, plus any helper only they used.
- `crates/promptforge-internal/parser/src/parse.rs`: in `Prompt::parse`, replace the `exact_shared_openings(body)` call with the new scan, returning the located `Fence` error. Rewrite the `Fence` bullet of the `Prompt::parse` doc to state the rule without naming removed forms.
- `crates/promptforge-internal/parser/src/error.rs`: rewrite the `Fence` variant doc the same way.
- Parser tests, using only neutral labels such as `lua extra`, `Lua`, and `luau`:
  - Delete every test and case that names a removed form: in `src/tests/h1_fences.rs`, `lua_shared_fence_in_h1_is_a_parse_error_pointing_to_require`, `removed_shared_fence_error_points_at_the_first_fence`, `lua_shared_fence_in_h2_is_a_parse_error_pointing_to_require`, `removed_lua_prompt_form_is_a_targeted_error_when_leading`, `lua_prompt_form_after_prose_is_ordinary_prose`, and `removed_shared_fence_fails_before_its_lua_compiles`; the `lua shared` case in `src/tests.rs` (near line 84); and the `lua shared` case in `src/tests/line_mapping.rs` (near line 307).
  - Rewrite `shared_fence_markers_must_be_exact` and `shared_markers_inside_longer_fences_remain_prose` in `h1_fences.rs` with neutral labels, under names that mention neither shared nor removed forms.
  - New tests cover: the error in the H1, in an H2, and before the H1; the exact message above; the reported line being the fence's opening line (in `line_mapping.rs`); failure before any Lua compiles, where the recorded parse events are only `PARSE_STARTED` then `PARSE_FAILED` and the error text omits the fence body; and a `lua extra` fence nested inside a four-backtick `markdown` fence staying prose, both in the H1 and in a section.
  - Label near-misses now expect the error: the `Lua` and `lua extra` cases of `section_fence_markers_must_be_exact` in `src/tests/section_fences.rs` move to an error test. Shape near-misses (a four-backtick `lua` opener, a one-space-indented `lua` opener) and `non_lua_fence_stays_in_prose` still expect prose.
- Facade fixture:
  - `Move-Item crates/promptforge/tests/prompts/invalid/removed-lua-prompt.md ../cabinet/_trash/`, and remove its `InvalidFixture` row from `crates/promptforge/tests/suite/parsing.rs`.
  - Add `crates/promptforge/tests/prompts/invalid/unrecognized-lua-label.md`, a small prompt whose only defect is a fence labeled `lua extra`, and its row: `name: "invalid/unrecognized-lua-label.md"`, `kind: ParseErrorKind::Fence`, `message_fragment: "unrecognized fence label"`.

</step-1>

<step-2>

### Step 2: Remove the retired-name lints [completed]

- Component: Trace removal
- Tests:
  - `cargo nextest run --locked -p build-xtask --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p build-xtask --all-targets --all-features`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
  - `rg -n "retired.symbol|retired_symbol|RETIRED" crates/build-xtask crates/workshop/ui/test` (must print nothing)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.

Work:

- `Move-Item crates/build-xtask/src/retired_symbols.rs ../cabinet/_trash/`, the same for `crates/build-xtask/src/retired_symbols-tests.rs`, and remove `mod retired_symbols;` from `crates/build-xtask/src/main.rs`.
- `crates/build-xtask/src/engine_guards.rs`: edit, do not delete, because `tidy.rs`, `api/load.rs`, `doc_hidden.rs`, and `test_support_leak.rs` use its `ENGINE_CONTAINER`, `collect_crates`, `engine_crates`, and `engine_guard_violations`. Remove `RETIRED_SEEDS`, `retired_symbol_violations`, its call in `engine_guard_violations`, the scan's mentions in the module and function docs, and any import or helper only the scan used. The other Engine guards stay.
- `crates/build-xtask/src/engine_guards-tests.rs`: remove `engine_sources_name_no_retired_symbols`, `a_retired_symbol_reintroduced_in_an_engine_crate_fails_the_guard`, the "retired symbol" clause in the module doc, and imports only they used.
- `crates/build-xtask/src/tidy.rs`: drop "retired-symbol scan" from the module doc's list.
- `crates/workshop/ui/test/docs-claims.mjs`: delete the `RETIRED` phrase list and its loop. Keep the lowercase-"host" check, which still catches the lowercase forms of those phrases. Capitalized forms such as "the Engine's Host" are no longer banned; that is accepted.

</step-2>

<step-3>

### Step 3: Sweep the remaining removed-feature traces

- Component: Trace removal
- Tests:
  - `cargo nextest run --locked -p promptforge-lua --all-features`
  - `cargo nextest run --locked -p <package> --all-features` for every other package the sweep edits, dropping `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`, and `node --test crates/workshop/ui/test/<name>.mjs` for any Workshop UI test file it edits
  - `rg -n -g '!target' -g '!vibe/**' "lua shared|lua prompt|RETIRED|retired.symbol|always_offer" crates prompts` (must print nothing; the `.` also catches the hyphenated "retired-symbol")
- Setup: read the root `AGENTS.md` and `crates/promptforge-internal/lua/AGENTS.md` before editing, plus the `AGENTS.md` of any other crate the sweep touches where one exists (engine, types, and vfs have one). Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.

Work:

- `crates/promptforge-internal/lua/src/tools/tests-offering.rs`: drop the `tools.always_offer == nil` clause and keep the rest of that assertion.
- Run `rg -n -i -g '!target' -g '!vibe/**' "removed|retired|no longer|formerly|legacy|deprecated|backward|was replaced" crates prompts` and fix every comment or test that describes or asserts a removed feature: a former API, syntax, behavior, or name, including absence checks that exist only to prove a feature is gone. Delete such text, or restate it as what the code does now. Add no compatibility handling.
- Leave other senses alone, such as provider model retirement, file or item removal at runtime, and deprecations in third-party dependencies. Edit nothing under `vibe/` and nothing outside `crates/` and `prompts/`.
- List every ambiguous hit you left alone, with its path and line, in the step report.

</step-3>

<step-4>

### Step 4: `promptforge::json` module and greeter example fixes

- Component: Facade review fixes
- Tests:
  - `cargo nextest run --locked -p promptforge --all-features`
  - `cargo run --locked -p promptforge --example greeter`
  - `cargo nextest run --locked -p build-xtask --all-features`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p promptforge --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p promptforge --no-deps`
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.

Work:

- `crates/promptforge/Cargo.toml`: move `serde_json.workspace = true` from `[dev-dependencies]` to `[dependencies]`. No generated-crate step is needed, since the workspace has had no `workspace-hack` crate or cargo-hakari since `76ee63f3c`.
- `crates/promptforge/src/lib.rs`: add an inline `pub mod json` beside the other inline modules, documented like them, re-exporting `serde_json::Value` and, if `cargo xtask api` admits a macro, `serde_json::json`. The facade shape check (`crates/build-xtask/src/facade_shape.rs`) admits only single-item re-exports from declared dependencies, so do not re-export the whole crate. `serde_json` is already on the API allowlist (`crates/build-xtask/src/api/items.rs`). If the API tooling refuses the macro, re-export `Value` alone and say so in the step report.
- Acceptance: `promptforge::json::Value`, and `promptforge::json::json!` when admitted, are usable from the facade alone.
- Re-bless with `cargo +nightly-2026-09-05 xtask api --bless`, then confirm that `git diff crates/promptforge/public-api.txt` shows only the new module's lines.
- `crates/promptforge/examples/greeter.rs`:
  - Import JSON from `promptforge::json` instead of `serde_json`, at the `serde_json::json!` call near line 90. If only `Value` is re-exported, build the same schema through `promptforge::json::Value` with no `serde_json` path, and describe the approach in the step report.
  - In `answer`, a timer gets `EffectAnswer::Timer` at once, without sleeping, instead of `EffectAnswer::Dropped`. Update the `answer` doc to say that a caller with a real clock waits `seconds` before answering. Leave the `f64` duration type alone, since a separate rulebook-audit plan owns it.
  - Everything else stays raw API code, because the example is the user-facing run loop. The facade suite includes this file as a module and runs `greet`, so its items stay `pub`.

</step-4>

<step-5>

### Step 5: Move the positive-fixture test into the Engine suite and fix the tool-offering wording

- Component: Facade review fixes
- Tests:
  - `cargo nextest run --locked -p promptforge-engine --all-features`
  - `cargo nextest run --locked -p promptforge --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p promptforge-engine -p promptforge --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p promptforge --no-deps`
  - `rg -n "every_positive_fixture_parses_offline" crates` (must list only `crates/promptforge-internal/engine/src/execute/tests/suite/parsing.rs`)
- Setup: read the root `AGENTS.md` and `crates/promptforge-internal/engine/AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.

Work:

- Move `every_positive_fixture_parses_offline` from `crates/promptforge/tests/suite/shipped.rs` into `crates/promptforge-internal/engine/src/execute/tests/suite/parsing.rs`, reading `crates/promptforge-internal/engine/tests/prompts/valid` and `crates/promptforge-internal/engine/tests/prompts/execution` relative to the Engine crate's manifest directory.
  - The Engine suite has no directory walker, since its tests load named files with `include_str!`, so copy `collect_markdown` and `every_prompt_under_parses` along. They port unchanged, because the Engine suite's `crate::parser::Prompt::parse(source, name).0` matches the facade's call.
  - `shipped.rs` keeps its copies for `every_shipped_prompt_parses_offline`, and its module doc drops the clause "and so does every fixture the Engine keeps as a positive example".
  - Every source file stays under the per-file line ceiling that `build_ceiling::check` enforces.
- Tool-offering wording: the `RunContext::tools` doc in `crates/promptforge-internal/engine/src/execute/config.rs` and the matching comment in `crates/promptforge/tests/suite/prepare.rs` say that the catalog lists every tool a run can use, Lua calls one by id, and a section offers one to its model with `tools.offer` (defined in `crates/promptforge-internal/lua/src/tools.rs`).

</step-5>

<step-6>

### Step 6: Facade test harness, proven on `greeter.rs`

- Component: Facade test harness
- Tests:
  - `cargo nextest run --locked -p promptforge --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p promptforge --all-targets --all-features`
  - `cargo fmt --all --check`
  - `(Get-Content crates/promptforge/tests/suite/greeter.rs).Count` (report the number against the checkpoint below)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.

Before any edit, snapshot the facade test names for the comparison in Step 8. Step 5 has already moved the positive-fixture test out, so the snapshot reflects the final set:

- `New-Item -ItemType Directory -Force ../cabinet/_scratch/vibe-coder-no-trace-fence | Out-Null`
- `cargo nextest list --locked -p promptforge --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-no-trace-fence/tests-before.txt`

Work:

- Build the test harness in `crates/promptforge/tests/suite/support.rs` and a new `crates/promptforge/tests/suite/support/scenario.rs`, wired from `support.rs` with `mod scenario;`. The flat-directory rule exempts top-level `tests/` trees, so the one-file subdirectory is allowed. It follows the Technical Design: the prompt builder (`prompt`, `greeter`), the preloaded answers (`text`, `calls`, `fail`, `shout()`, `Tool::untrusted`, `canned()`, `TestResult`), `Scenario`, `Driver`, and `Trace`, the driving rules, the three cases where it fails instead of hanging, and the scenario defaults.
- Land only the helpers `greeter.rs` uses. Clippy denies unused code, so each remaining helper lands in the step that first uses it. Keep the existing `context(name)` helper while `prepare.rs` still uses it.
- Use `promptforge::` paths only, take JSON from `promptforge::json`, and add no new crates. Keep `scenario.rs` under the per-file line ceiling that `build_ceiling::check` enforces (`crates/promptforge/build.rs`).
- Harness choices already decided: every scenario defaults to the canned model, because it does nothing without declared roles and can be overridden; the builder is named `Scenario`, because "Harness" names a product component in this repository; tests stay separate with sentence names, so one failure names one test, and are never merged into tables.
- Migrate `crates/promptforge/tests/suite/greeter.rs` onto the harness, keeping every test's name and assertions. It still includes `crates/promptforge/examples/greeter.rs` as a module and runs `greet`.
- Budget checkpoint: `parsing.rs`, `tools.rs`, `metrics.rs`, `auto_traits.rs`, and `main.rs` stay untouched and hold about 213 lines. With about 300 lines of harness, the 13 migrated files must shrink from about 2,125 lines to about 940, a 56% cut, which puts `greeter.rs` (194 lines at baseline) near 85 lines. If `greeter.rs` shrinks well short of that, ending above about 110 lines, stop and report before Step 7.

</step-6>

<step-7>

### Step 7: Migrate the `effect`, `event`, `ids`, and `cancel` suites

- Component: Facade test harness
- Tests:
  - `cargo nextest run --locked -p promptforge --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p promptforge --all-targets --all-features`
  - `cargo fmt --all --check`
  - `(Get-ChildItem crates/promptforge/tests/suite -Recurse -Filter *.rs | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum` (report the running total)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.

Work:

- Move `crates/promptforge/tests/suite/effect.rs` (200 lines at baseline), `event.rs` (219), `ids.rs` (244), and `cancel.rs` (122) onto the test harness in `crates/promptforge/tests/suite/support.rs` and `support/scenario.rs`, as the Technical Design specifies it. Add each remaining helper, such as `Driver` with `pump`, `pump_until_held`, and `finish` for tests that act mid-run, in this commit only if one of these files uses it.
- Keep every test's name and assertions. Tests stay separate with sentence names, so one failure names one test, and are never merged into tables.
- Use `promptforge::` paths only, take JSON from `promptforge::json`, and keep every file under the per-file line ceiling.
- Budget: together these four files should land near 345 lines, the 56% cut that keeps the suite on track for about 1,450 lines.

</step-7>

<step-8>

### Step 8: Migrate the remaining suites and confirm the exit criteria

- Component: Facade test harness
- Tests:
  - `cargo nextest list --locked -p promptforge --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-no-trace-fence/tests-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-no-trace-fence/tests-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-no-trace-fence/tests-after.txt)` (must print nothing)
  - `(Get-ChildItem crates/promptforge/tests/suite -Recurse -Filter *.rs | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum` (must be about 1,450 or less; 2,349 at baseline)
  - `rg -n -g '!target' -g '!vibe/**' "lua shared|lua prompt|RETIRED|retired.symbol|always_offer" crates prompts` (must print nothing)
  - `npm ci --prefix crates/workshop`
  - `npm ci --prefix crates/gateway/config-ui/ui`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p promptforge --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.

Work:

- Move `crates/promptforge/tests/suite/model.rs` (126 lines at baseline), `plugins.rs` (105), `replay.rs` (73), `timestamp.rs` (56), `vfs.rs` (120), `prompt.rs` (202), `prepare.rs` (304), and `shipped.rs` (160 at baseline, less after the positive-fixture test left) onto the test harness in `crates/promptforge/tests/suite/support.rs` and `support/scenario.rs`, as the Technical Design specifies it, adding each remaining helper as its first user arrives.
- Keep every test's name and assertions. Tests stay separate with sentence names and are never merged into tables. Tests that serialize logs keep calling `serde_json` directly, which is a normal dependency of the crate since Step 4.
- `prepare.rs` keeps its tool-offering comment: the catalog lists every tool a run can use, Lua calls one by id, and a section offers one to its model with `tools.offer`. `shipped.rs` keeps `collect_markdown`, `every_prompt_under_parses`, `every_shipped_prompt_parses_offline`, and the shipped research prompt's end-to-end run over its web tools.
- Remove the old `support::context` helper once nothing uses it.
- Leave `parsing.rs`, `tools.rs`, `metrics.rs`, `auto_traits.rs`, and `main.rs` as they are. Add no auto-trait checks, because `crates/promptforge/public-api.txt` already pins every facade type's auto traits and `cargo xtask api --check` fails on any change.
- Use `promptforge::` paths only and keep every file under the per-file line ceiling.
- The line budget is the raw line count of `crates/promptforge/tests/suite/**/*.rs`, harness included: about 1,450 or fewer, down from 2,349 at baseline.

</step-8>

</execution-plan>
