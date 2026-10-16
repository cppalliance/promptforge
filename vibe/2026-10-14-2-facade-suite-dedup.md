---
name: Facade suite dedup
overview: Shrink the facade suite's source by deleting tests and helpers that duplicate code elsewhere and trimming prose that restates the code, measured in tokens, with no macros, no formatting changes, and no coverage lost.
todos:
  - id: delete-duplicates
    content: Delete auto_traits.rs, tools.rs, metrics.rs, and the six in-file duplicate tests; update module docs and main.rs
    status: completed
  - id: move-shipped
    content: Move every_shipped_prompt_parses_offline into the Engine suite and delete the facade's walker copy
    status: completed
  - id: tempfile
    content: Add tempfile as a dev-dependency of the facade, engine, and vfs crates and replace the three TempDir copies
    status: completed
  - id: shared-helpers
    content: Add model, descriptor, note, and Trace::text to the harness and apply them; parsing.rs fixtures as tuples
    status: completed
  - id: trim-prose
    content: Trim harness and test prose that restates the code; report the token and line counts
    status: completed
isProject: false
---

# Facade Suite Dedup

### Context for a fresh reader

- Repository: `c:\Users\Vinnie\cursor\runvil` (git). Paths are relative to it unless they start with `../`.
- Baseline: `30a66c78d` (`Close plan: no-trace-fence`). That plan's commits are local and unpushed. Search by name if a cited line has moved.
- Run with the vibe coder (`../tools-public/planning/vibe-coder.md`). It owns the plan copy under `vibe/`, `vibe/ACTIVE`, and the close commit.
- Shell: PowerShell on Windows. Run cargo commands one at a time, don't append `2>&1`, and judge each by its result lines and `$LASTEXITCODE`.
- Before editing a crate, read the root `AGENTS.md` and the crate's own `AGENTS.md` (engine and vfs have one).
- A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.
- Terms:
  - *The facade* is `crates/runvil`. Its `src/lib.rs` only re-exports items from the six crates under `crates/runvil-internal/`, so every behavior lives, and is tested, in those crates.
  - *The facade suite* is `crates/runvil/tests/suite/`. *The harness* is its `support.rs` and `support/scenario.rs`.
  - *The token count* is the measure. It's taken with tiktoken's `o200k_base` over `crates/runvil/tests/suite/**/*.rs`, from the repository root:
    `python -c "import pathlib,tiktoken; e=tiktoken.get_encoding('o200k_base'); print(sum(len(e.encode(p.read_text(encoding='utf-8'), disallowed_special=())) for p in pathlib.Path('crates/runvil/tests/suite').rglob('*.rs')))"`
    It is 17,361 at the baseline (2,041 lines). The harness is 3,914 of that, about 28% of it doc prose.

<product-contract>

## Product Requirements

The facade suite carries less source for models and people to read. Tests that duplicate other tests, helpers that duplicate other helpers, and prose that restates the code all go. No coverage is lost.

- Problem and users:
  - Thirteen facade tests check nothing that isn't checked elsewhere. The five in `crates/runvil/tests/suite/auto_traits.rs` duplicate the `auto` lines in `crates/runvil/public-api.txt`, which `cargo xtask api --check` enforces. Eight behavior tests duplicate internal-crate tests, listed under Technical Design.
  - Several helpers exist in more than one copy:
    - `TempDir` has three: `crates/runvil/tests/suite/prepare.rs`, `crates/runvil-internal/engine/src/execute/tests/suite/vfs.rs`, and `crates/runvil-internal/vfs/src/real/tests.rs`.
    - The fixture walker (`collect_markdown`, `every_prompt_under_parses`) is in both `crates/runvil/tests/suite/shipped.rs` and `crates/runvil-internal/engine/src/execute/tests/suite/parsing.rs`.
    - The facade suite builds model descriptors four times and tool descriptors three times, and repeats the write-then-read Lua three times.
  - Many harness and test comments repeat a name or restate the next line.
- Goals:
  - Delete the 13 duplicate tests.
  - Leave one copy of each duplicated helper.
  - Trim prose that restates the code.
  - Report the token count. The rough expectation is about 13,500, but it's a target, not a gate.
- Non-goals:
  - Formatting changes: no `rustfmt.toml` and no density tweaks. Line count is only a proxy for tokens.
  - Macros: none are added.
  - These tests stay as they are: the 21 that repeat an internal test inside a full run, the 8 that are the only place something is checked, and the 6 unique ones.
  - The deferred cleanup of the internal crates, including the Engine suite's `*_prompt!` macros.
- Success criteria:
  - Every suite and check in the exit criteria passes.
  - `cargo nextest list` for `runvil` differs from the baseline only by the 13 deleted tests and the one moved test. For `runvil-engine`, it differs only by gaining the moved test.
  - `rg -n "macro_rules!" crates/runvil` returns nothing.
  - The token count is reported against 17,361.
- Constraints:
  - The facade suite names Runvil items by `runvil::` paths. It may also use std, `serde_json`, and `tempfile`.
  - Clippy runs with `-D warnings`, so no helper, constant, or import is left unused.
  - Every source file stays under the per-file line ceiling.
- Open questions: None

## Functional Specification

Nothing changes for prompt authors or Rust callers: no public API, behavior, or message changes. Only tests and test helpers change.

- Actors and workflows: a maintainer runs the workspace checks, and a model reads the facade suite.
- Inputs and outputs: none change.
- States and validation: none change.
- Errors and recovery: none change.
- Security and privacy behavior: None
- Acceptance criteria:
  - The test-name lists differ exactly as the success criteria say.
  - `crates/runvil/public-api.txt` is unchanged, and `cargo xtask api --check` passes against it.

</product-contract>
<implementation-contract>

## Technical Design

Five changes: delete the duplicate tests, move the shipped-prompt test to the walker's other copy, replace `TempDir` with `tempfile`, add three shared helpers plus `Trace::text`, and trim restating prose.

- Duplicate tests to delete, each with the internal test that already covers it:
  - All of `auto_traits.rs` (5 tests). `public-api.txt` records every one of those types' auto traits.
  - `ids.rs` `a_model_started_task_is_abandoned_when_its_owning_section_returns`, and its `LOOPER` constant. Covered by engine `execute/tests/model_tasks.rs` `an_owner_that_ends_first_leaves_a_model_task_abandoned_not_cancelled`.
  - `prompt.rs` `declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`. Covered by engine `execute/tests/suite/args_surface.rs` `argv_is_the_parsed_json_on_success` and `a_default_declared_prompt_wraps_interface_prose`.
  - `model.rs` `a_text_completion_served_under_the_bound_models_name_becomes_the_result`. Covered by engine `execute/tests/live_infer.rs` `live_h1_infer_runs_once`.
  - `plugins.rs` `a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`. Covered by types `plugins-tests.rs` `plugin_id_contains_exactly_the_tools_whose_first_segment_it_is` and `plugin_id_requires_exactly_one_segment`.
  - `plugins.rs` `a_prelude_on_the_context_defines_a_function_the_section_calls`. Covered by engine `execute/tests/preludes.rs` `a_library_a_fanout_arm_and_a_spawned_task_all_see_a_prelude_global`.
  - `replay.rs` `flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`. Covered by types `replay-tests.rs` `flags_start_empty_and_round_trip_their_bits`, which tests the same `0b101` case.
  - All of `tools.rs`. Covered by types `tools/tests.rs` `tool_output_has_mandatory_trust` and `tool_error_classifies_and_hides_source`.
  - All of `metrics.rs`. Covered by model-client `client/tests.rs` `a_completion_from_a_bare_result_carries_no_metrics_and_no_raw_exchange` and `the_builders_set_metrics_the_raw_exchange_and_the_finish_reason`.
  - The three whole files go to `../cabinet/_trash/`, and their `mod` lines leave `main.rs`.
  - Each touched file's `//!` doc, and `main.rs`'s, stops describing what was deleted.
- The shipped-prompt test:
  - `every_shipped_prompt_parses_offline` moves into the Engine suite's `parsing.rs`. There it calls the existing `every_prompt_under_parses` on `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../prompts")`.
  - `shipped.rs` loses `collect_markdown`, `every_prompt_under_parses`, `SHIPPED_PARSE`, and that test, and keeps the research-prompt test.
- `tempfile`:
  - Add `tempfile.workspace = true` under `[dev-dependencies]` in `crates/runvil/Cargo.toml`, `crates/runvil-internal/engine/Cargo.toml`, and `crates/runvil-internal/vfs/Cargo.toml`. The vfs crate has no `[dev-dependencies]` table yet.
  - The Engine manifest guard (`crates/build-xtask/src/engine_deps.rs`) doesn't check dev-dependencies, and `tempfile = "3"` is already a workspace dependency.
  - All three `TempDir` structs, and their "no tempfile dependency" comments, go.
  - The vfs crate's zero-dependency rule is loosened to allow `[dev-dependencies]`, by the user's choice. The library stays std only, with no normal or build dependencies:
    - `crates/runvil-internal/vfs/src/lib.rs`: `is_dependency_table` checks only `dependencies` and `build-dependencies`, directly, as sub-tables, or under a `target.<cfg>` qualifier. The manifest test is renamed `the_manifest_declares_no_normal_or_build_dependencies`, and its doc says dev-dependencies serve only the tests.
    - `a_manifest_with_a_target_qualified_dependency_table_is_refused` and `dependency_sub_tables_count_as_dependency_tables` move their `dev-dependencies` cases to the passing side, so a test pins that dev-dependencies are allowed.
    - The rule's wording changes in four places: the comment in `crates/runvil-internal/vfs/Cargo.toml` (drop "never weaken it"), the crate doc in `src/lib.rs` ("std only, no workspace or external crates" becomes a statement about the library), the doc in `build.rs` ("declares no dependencies of any kind" becomes "declares no build dependencies"), and `README.md` ("It has no dependencies at all" becomes a statement about the library).
    - `TempDir` leaves `src/real/tests.rs` and its `atomicity.rs`, `links.rs`, and `semantics.rs` submodules. `io::Error` is mapped into `VfsError` where those tests return it.
- Shared helpers in `support.rs`:
  - `model(name: &str, window: u32, thinking: ThinkingMode) -> TestResult<ModelDescriptor>`. `canned()` calls it. It replaces `prepare.rs` `current_model`, the inline descriptor in `prompt.rs`'s role check, and `model.rs` `fast` and `deep`.
  - `descriptor(id: &str, schema: serde_json::Value) -> TestResult<ToolDescriptor>`. It replaces the inline descriptors in `scenario.rs` and in `prepare.rs`'s two catalog tests, and `shipped.rs` `web_tool`. A test that needs `.structured(true)` chains it.
  - `note(value: &str) -> String`, the one-section greeter that writes the Lua expression `value` to `note.md` and returns what it reads back:
    - It replaces the `NOTE` constant: `greeter(NOTE)` becomes `note("'hello'")`.
    - It replaces `event.rs`'s own `NOTE`, as `note("'hello ' .. args")`.
    - It replaces the `sys.when` block in `timestamp.rs`, as `note("sys.when")`.
- `Trace::text(&self) -> TestResult<&str>` returns the text of `RunResult::Ok`, and otherwise fails with the actual result. It replaces `assert!(matches!(trace.result, RunResult::Ok(text) if text == ...))` with `assert_eq!(trace.text()?, ...)`, and replaces the `let RunResult::Ok(..) = ... else` blocks.
- `parsing.rs`: the four fixtures become one array of `(file stem, ParseErrorKind, message fragment)` tuples. The loop reads `tests/prompts/invalid/<stem>.md` from `CARGO_MANIFEST_DIR`, so each name appears once.
- Prose:
  - Keep comments that state a constraint the code can't show:
    - the driving rules in `scenario.rs`'s module doc;
    - why every scenario defaults to `canned()`;
    - why a held effect gets `Dropped`;
    - the one pointer saying which prepare cases live in the Engine suite.
  - Delete comments that repeat an item's name, restate the next line, or describe Lua that sits right below them.
  - On `Scenario`, the field docs duplicate the builder-method docs, so keep one of each pair.
  - Each test file's module doc keeps one sentence saying what the file tours.
- Data, persistence, failure, security, and privacy constraints: None

</implementation-contract>
<verification-contract>

## Testing Plan

The suites prove nothing broke. The test-name comparison proves exactly the intended tests left. The token count measures the result.

- Unit and integration:
  - `cargo nextest run --locked -p runvil --all-features`
  - `cargo nextest run --locked -p runvil-engine --all-features` (receives the moved test, and its `vfs.rs` drops `TempDir`)
  - `cargo nextest run --locked -p runvil-vfs --all-features` (its real-backend tests drop `TempDir`)
- Regression:
  - Before the first change, save `cargo nextest list --locked -p runvil --all-features` and the same for `-p runvil-engine`. After the last change, compare them with fresh lists.
  - The facade list must lose exactly the 13 deleted tests and `every_shipped_prompt_parses_offline`, and gain nothing.
  - The Engine list must gain exactly `every_shipped_prompt_parses_offline`.
- Exit criteria, using the verification list in `AGENTS.md`:
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, and `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`.
  - Both clippy runs pass with `CARGO_BUILD_WARNINGS=deny`, together with `cargo check -p gateway --no-default-features`.
  - `cargo fmt --all --check`.
  - The workspace docs gate and the facade docs gate (`cargo doc -p runvil --no-deps`) pass with `RUSTDOCFLAGS="-D warnings"`.
  - `cargo +nightly-2026-09-05 xtask api --check`, against the unchanged `public-api.txt`.
  - `node --test crates/workshop/ui/test/docs-claims.mjs`.
  - `rg -n "macro_rules!" crates/runvil` returns nothing.
  - The token count is reported against 17,361, along with the line count against 2,041.

</verification-contract>
<decision-record>

## Decision Record

The user accepted the high-confidence cleanup, dropped the macro and formatting ideas, chose to delete only full duplicates, and set tokens as the measure.

- Decisions:
  - Duplication goes. The user: "If it's a duplication, we should delete it. If it's not doing us any good, we should remove it, because all that does is create a tax on the vibe coding."
  - `auto_traits.rs` is deleted. This overrides the no-trace-fence plan's decision to keep it, because `public-api.txt` already pins every one of those auto traits.
  - Only the 8 full-duplicate behavior tests go. The user picked that over also deleting the 21 tests that repeat an internal test inside a full run.
  - No macros. The user dislikes them because a reader has to learn the macro before reading the code, and wants any macro in use named so they can judge it. This plan adds none.
  - No formatting changes. The user: line count is a proxy for tokens, so "we shouldn't go out of our way to make it more dense, and we shouldn't ... make it more lines."
  - Tokens are the measure. Lines are reported only for reference.
  - The vfs crate's zero-dependency rule is loosened to allow dev-dependencies, so `tempfile` replaces its `TempDir` too. Decomposition found that the rule ("No dependencies, workspace or external. The manifest test enforces this; never weaken it.") and its manifest test refused `[dev-dependencies]` as well. The user chose to loosen it rather than keep a third hand-written temporary directory. The library stays std only: the test still refuses normal and build dependencies, which are what made the crate compile alone and never rebuild for a dependency.
- Rejected alternatives:
  - A `pick!` macro for pulling events or effects out of logs: few tokens saved, and it adds a private construct.
  - `use_small_heuristics = "Max"` for the facade: it saves lines, not tokens.
  - Two harnesses: both would need the same core of context building, effect answering, and recording, so splitting adds a second API without removing code.
  - Deleting the 21 full-run repeats: the user kept them. Revisit if the internal crates gain full-run tests for those features.
  - Moving Lua sources into `.md` fixtures: it moves tokens out of the measured files without removing them.
  - Moving the 8 sole-coverage assertions into the internal crates: about token-neutral, and they work where they are.
- Assumptions, risks, and notes:
  - Risk: the vfs crate's `TempDir` returns `VfsError`, while `tempfile` returns `io::Error`. Its tests may need a `map_err` or a different return type.
  - Note: the facade suite and the facade crate have no `macro_rules!` today. Repo-wide there are 21 definitions, outside this plan:
    - 4 in production code: `events!`, `lifecycle_constants!`, `install_reported_store_fn!`, `nz!`.
    - 7 Engine test prompt builders, two names defined twice.
    - 7 recording test-support macros.
    - 3 in other tests.

### Deferred and Out of Scope

- Deferred: the internal-crates test cleanup. That includes the Engine suite's seven `*_prompt!` builder macros (`effect_prompt!`, `prose_prompt!`, `require_prompt!`, `args_prompt!` twice, `flow_prompt!` twice), which the user may want removed.
- Deferred: the outdated "offers every tool" wording in `crates/runvil-internal/types/src/tools/registry.rs`, `crates/runvil-internal/engine/src/tools.rs`, and a comment in `crates/runvil-internal/engine/src/execute/tests/context.rs`.
- Deferred: the rulebook-audit fixes, as their own plan.

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
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus the facade build with default features: `RUSTDOCFLAGS="-D warnings" cargo doc -p runvil --no-deps`. CI additionally documents `harness` with default features and `runvil-engine` and `workshop-server` with `--document-private-items`.
- Test placement and naming conventions: Unit tests live in a sibling file `<module>-tests.rs`, wired at the end of the module with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;`; a few modules still use an inline `mod tests { }`, and `runvil-lua`, `runvil-parser`, and the `harness-gateway-client` transport keep `src/tests/` directories. Integration tests compile as one binary per crate at `tests/it/main.rs` (most crates) or `tests/suite/main.rs` (`runvil`, `harness`), with one `mod` file per area plus a `support` module; data goes in `tests/fixtures/`, shared helpers in `tests/common/`, and Engine prompt fixtures in `tests/prompts/`. A few crates keep standalone `tests/<name>.rs` binaries (`gateway-stt-engine`, the `native_*` files of `gateway-stt-backend-whisper`, `build-workshop`, `gateway-cloud-providers`). Test functions are snake_case sentences that state the behavior, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`. Tests needing native whisper assets are `#[ignore]`d with a `native_` prefix; Miri-run tests carry a `miri_` prefix. Workshop UI tests are `node:test` plus jsdom files at `crates/workshop/ui/test/<kebab-name>.mjs` or `src/**/*.test.mjs`; Node tool scripts sit beside `tools/<name>.test.mjs`.
- Directory map: `crates/` holds all code. Its top level is the public and shared layer: `runvil` (the Engine's public API facade), `runvil-plugin` (the Plugin API), `harness` and `harness-gateway-client`, the linked Plugins `plugin-web`, `plugin-mcp`, and `plugin-user-input`, the gateway vocabulary and discovery crates `gateway-api-types` and `gateway-api-discovery`, `shared-error-source` and `shared-loopback`, `shared-ui` (a TypeScript and CSS package, not a Rust crate), and the `build-*` tooling (`build-xtask` behind `cargo xtask`, `build-workshop` behind `cargo workshop`, plus `build-ui`, `build-user-guide`, `build-llama-cuda`, `build-ceiling`). Four manifestless containers hold each family's private crates: `crates/runvil-internal/` (Engine: `types`, `parser`, `lua`, `engine`, `vfs`, `model-client`), `crates/harness-internal/` (`runner`), `crates/gateway/` (the `app` binary, `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and `stt/` with `api`, `engine`, `backend-whisper`, `whisper-ffi`), and `crates/workshop/` (the Tauri `desktop` app, `server`, `server-api`, the tier crates, and the npm workspace members `ui`, `look`, `platform`). Outside `crates/`: `guide/` is the user guide source (books for gateway, language, workshop, plus `chrome` and `landing`); `prompts/` holds sample prompt programs; `vibe/` holds dated implementation plans; `tools/` holds Node scripts for staging the gateway sidecar and a live TTS check; `images/` holds README and site art; `.github/workflows/` holds CI (`ci.yml`), release, nightly, Miri, site, and installer workflows; `.githooks/` holds pre-commit (fmt) and pre-push (wording scan, headless gateway check, clippy, cargo deny); `.cargo/config.toml` sets rust-lld and the static CRT on Windows plus the `xtask` and `workshop` aliases; `.config/nextest.toml` sets timeouts and the heavy STT test group; `.cursor/rules/` holds the Workshop architecture and SPA rules. `local/`, `target/`, `target-msrv/`, and `cabinet/` are untracked.
- Component boundaries: Engine: the `runvil` facade depends on all six `runvil-internal` crates; inside, `runvil-engine` depends on `runvil-lua`, `runvil-parser`, and `runvil-model-client`, `runvil-parser` depends on `runvil-lua`, and `runvil-types` and `runvil-vfs` are leaves. Plugin API: `runvil-plugin` depends on `runvil-types` and `runvil-vfs`, and each linked Plugin depends only on `runvil-plugin`. Harness: `harness` depends on `harness-runner`, `runvil`, and `runvil-plugin`; `harness-runner` depends on `runvil` and `runvil-plugin`; `harness-gateway-client` depends on `harness`, `plugin-web`, and `runvil`. Gateway: the `gateway` app sits on top of its family crates, the family reaches outside only through `gateway-api-types`, `gateway-api-discovery`, and the `shared-*` crates, and nothing in it depends on the Engine or Harness; within STT, `gateway-stt` depends on `gateway-stt-engine` and `gateway-stt-backend-whisper`, which depends on `gateway-whisper-ffi`, the subsystem's only `unsafe` code. Workshop: tiers flow server, then features, then services, then vocabulary; `workshop-server` is the composition root and depends on every Workshop crate plus `harness`, `harness-gateway-client`, the Plugins, and `runvil`; the desktop package `workshop` depends only on `workshop-server-api` and `gateway-api-discovery`; `workshop-protocol` and `workshop-support` are leaves and `workshop-registry` depends only on `workshop-protocol`; Workshop reaches the gateway only through `gateway-api-types` and `gateway-api-discovery`. The `build-*` crates have no workspace dependencies. `cargo test -p build-xtask` enforces the product and container boundaries, the Workshop tier graph, the `## Invariants` marker, and lint inheritance.
- Conventions summary: Rust edition 2024 on the stable toolchain with resolver 3 and every dependency declared once in `[workspace.dependencies]`, each non-obvious choice explained by a comment. Workspace lints deny clippy `all` and `pedantic`, `unwrap_used` and `expect_used` outside tests, bare `allow` attributes (use `#[expect(..., reason = "...")]`), and `unsafe_code`; `missing_docs`, `missing_debug_implementations`, and `unreachable_pub` warn and the gate denies warnings; `clippy.toml` bans process-global installers outside binary entry points. Source directories are flat: a subdirectory needs at least three files, otherwise modules sit beside the parent as `parent-label.rs` wired with `#[path]`. Each Workshop crate's `lib.rs` is a facade whose `//!` doc carries a `## Invariants` section listing allowed dependencies. Engine, Harness, Host, and Plugin are capitalized defined terms, and Engine crates say "the caller" instead of naming the Host. Comments state only non-obvious constraints, and workarounds cite their upstream issue URL. Error messages are written for model consumption and name required versus actual. Behavior changes ship with tests in the same change. JSON reaching a recorder or replay round-trips exactly with sorted keys. Cargo features gate real build constraints, not product shape. Workshop UI CSS uses design tokens only, and persisted UI values go through the `ui-storage` adapter. npm members get an `eol=lf` line in `.gitattributes`.

</project-survey>
<execution-plan>

## Execution Instructions

Three components, five steps, one commit per step, in the order below. Every commit compiles, passes the suites it touches, and leaves nothing unused, since clippy runs with `-D warnings`. Steps 2 and 3 depend on no earlier code and could land in either order; the order here is the commit order. The full exit criteria run in Step 5. Paths are relative to the repository root, `c:\Users\Vinnie\cursor\runvil`, unless they start with `../`. Test-list snapshots live in `../cabinet/_scratch/vibe-coder-facade-suite-dedup/`.

- Components, in dependency order:
  1. Duplicate removal (Steps 1 and 2). First, because Steps 4 and 5 edit files it shrinks, so the helpers and the trim touch only code that survives, and the baseline test lists must be saved before any test leaves.
  2. Shared helpers (Steps 3 and 4). Second, because several would-be helper sites leave with the deleted tests (the argument closure in `prompt.rs`, the text-result check in `model.rs`, the prelude greeter in `plugins.rs`), and the trim should judge the helpers' final docs.
  3. Prose trim (Step 5). Last, so the token count measures the finished suite and the exit criteria run on the final tree.
- Pieces and construction:
  - Duplicate removal: the in-suite deletions, then the shipped-prompt move. They share no code, so they are sequential only to keep commits small; the move's test-list check extends Step 1's.
  - Shared helpers: the `tempfile` swap, then the harness helpers. Sequential, because both edit `crates/runvil/tests/suite/prepare.rs`: Step 3 replaces its `TempDir`, and Step 4 its `current_model` and catalog descriptors.
  - Prose trim: one piece in one step, since the rules apply across the whole suite and the measurement needs all of it.

<step-1>

### Step 1: Delete the duplicate facade tests [completed]

- Component: Duplicate removal
- Tests:
  - `cargo nextest run --locked -p runvil --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil --all-targets --all-features`
  - `cargo fmt --all --check`
  - `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly the 13 deleted tests named under Work, each marked `<=`, and nothing marked `=>`)
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. A file leaves the repository by `Move-Item` to `../cabinet/_trash/`, never by deletion.

Before any edit, save the baseline test lists that this step and Steps 2 to 5 compare against:

- `New-Item -ItemType Directory -Force ../cabinet/_scratch/vibe-coder-facade-suite-dedup | Out-Null`
- `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt`
- `cargo nextest list --locked -p runvil-engine --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-before.txt`
- If either file is ever missing, rebuild it from a `git worktree` of the baseline commit `30a66c78d` placed in that scratch directory, never from an edited tree.

Rules: name Runvil items by `runvil::` paths, leave no constant, helper, or import unused, add no macros, make no formatting-only changes, and delete, rename, or move no test beyond the 13 below.

Work:

- Move three whole files with `Move-Item <file> ../cabinet/_trash/`, and remove `mod auto_traits;`, `mod metrics;`, and `mod tools;` from `crates/runvil/tests/suite/main.rs`:
  - `crates/runvil/tests/suite/auto_traits.rs`, holding `a_run_can_move_between_threads_between_calls`, `the_run_configuration_and_outcome_types_are_send_sync_and_static`, `a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin`, `events_and_the_tool_catalog_and_errors_cross_threads`, and `vfs_handles_and_accesses_are_send_and_sync`. The `auto` lines in `crates/runvil/public-api.txt` record every one of those types' auto traits, and `cargo xtask api --check` enforces them. This overrides the no-trace-fence plan's decision to keep the file.
  - `crates/runvil/tests/suite/tools.rs`, holding `tool_output_carries_its_trust_and_a_failure_shows_only_its_message`. Covered by `tool_output_has_mandatory_trust` and `tool_error_classifies_and_hides_source` in `crates/runvil-internal/types/src/tools/tests.rs`.
  - `crates/runvil/tests/suite/metrics.rs`, holding `a_canned_completion_measures_nothing_until_metrics_are_attached`. Covered by `a_completion_from_a_bare_result_carries_no_metrics_and_no_raw_exchange` and `the_builders_set_metrics_the_raw_exchange_and_the_finish_reason` in `crates/runvil-internal/model-client/src/client/tests.rs`.
- Delete six tests in `crates/runvil/tests/suite/`, with every constant, helper, and import only they use:
  - `ids.rs`: `a_model_started_task_is_abandoned_when_its_owning_section_returns` and its `LOOPER` constant. `Effect`, `serde_json::json`, `calls`, `AbandonReason`, and `TaskOrigin` have no other user there. Covered by `an_owner_that_ends_first_leaves_a_model_task_abandoned_not_cancelled` in `crates/runvil-internal/engine/src/execute/tests/model_tasks.rs`.
  - `prompt.rs`: `declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`. Covered by `argv_is_the_parsed_json_on_success` and `a_default_declared_prompt_wraps_interface_prose` in `crates/runvil-internal/engine/src/execute/tests/suite/args_surface.rs`.
  - `model.rs`: `a_text_completion_served_under_the_bound_models_name_becomes_the_result`. Covered by `live_h1_infer_runs_once` in `crates/runvil-internal/engine/src/execute/tests/live_infer.rs`.
  - `plugins.rs`: `a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`, covered by `plugin_id_contains_exactly_the_tools_whose_first_segment_it_is` and `plugin_id_requires_exactly_one_segment` in `crates/runvil-internal/types/src/plugins-tests.rs`; and `a_prelude_on_the_context_defines_a_function_the_section_calls`, covered by `a_library_a_fanout_arm_and_a_spawned_task_all_see_a_prelude_global` in `crates/runvil-internal/engine/src/execute/tests/preludes.rs`.
  - `replay.rs`: `flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`. Covered by `flags_start_empty_and_round_trip_their_bits` in `crates/runvil-internal/types/src/replay-tests.rs`, which tests the same `0b101` case.
- Module docs stop describing what left: `main.rs` drops the auto-trait clause, `ids.rs` the clause about a task's start and end events, `prompt.rs` the clause about argument text reaching Lua as JSON or prose, `model.rs` the clause about a chat answer becoming the result, `plugins.rs` the Plugin-id and prelude clauses, and `replay.rs` the clause about keeping unnamed bits.

</step-1>

<step-2>

### Step 2: Move the shipped-prompt parse test into the Engine suite [completed]

- Component: Duplicate removal
- Tests:
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `cargo nextest run --locked -p runvil --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine -p runvil --all-targets --all-features`
  - `cargo fmt --all --check`
  - `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly the 14 expected facade lines below, each marked `<=`, and nothing marked `=>`)
  - `cargo nextest list --locked -p runvil-engine --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly one line, marked `=>`, naming `every_shipped_prompt_parses_offline` in the Engine suite's `parsing` module)
  - `rg -n "every_shipped_prompt_parses_offline|collect_markdown|SHIPPED_PARSE" crates` (must list only `crates/runvil-internal/engine/src/execute/tests/suite/parsing.rs`)
- Expected facade lines: `shipped::every_shipped_prompt_parses_offline`, plus Step 1's 13 deletions: `auto_traits::a_run_can_move_between_threads_between_calls`, `auto_traits::the_run_configuration_and_outcome_types_are_send_sync_and_static`, `auto_traits::a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin`, `auto_traits::events_and_the_tool_catalog_and_errors_cross_threads`, `auto_traits::vfs_handles_and_accesses_are_send_and_sync`, `tools::tool_output_carries_its_trust_and_a_failure_shows_only_its_message`, `metrics::a_canned_completion_measures_nothing_until_metrics_are_attached`, `ids::a_model_started_task_is_abandoned_when_its_owning_section_returns`, `prompt::declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`, `model::a_text_completion_served_under_the_bound_models_name_becomes_the_result`, `plugins::a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`, `plugins::a_prelude_on_the_context_defines_a_function_the_section_calls`, and `replay::flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`.
- Setup: read the root `AGENTS.md` and `crates/runvil-internal/engine/AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. The baseline lists `facade-before.txt` and `engine-before.txt` were saved in `../cabinet/_scratch/vibe-coder-facade-suite-dedup/` before Step 1; if one is missing, rebuild it from a `git worktree` of the baseline commit `30a66c78d` in that directory, never from an edited tree.

Work:

- `crates/runvil-internal/engine/src/execute/tests/suite/parsing.rs`: add `#[test] fn every_shipped_prompt_parses_offline()`, which calls the file's existing `every_prompt_under_parses` on `Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../prompts")`, the workspace `prompts/` tree three levels above the Engine manifest. Add to the module doc that every shipped prompt parses offline too.
- `crates/runvil/tests/suite/shipped.rs`: delete `SHIPPED_PARSE`, `collect_markdown`, `every_prompt_under_parses`, `every_shipped_prompt_parses_offline`, and the imports only they use (`PathBuf`, `Prompt`). `the_shipped_research_prompt_offers_its_web_tools_by_id_and_runs_end_to_end` stays unchanged, and the module doc keeps only its sentence.
- `crates/runvil/tests/suite/main.rs`: its module doc names the shipped research prompt's run in place of shipped-prompt policy. Keep `#![expect(clippy::expect_used, ...)]`, because `prepare.rs` still calls `expect`.
- Name Runvil items in the facade suite by `runvil::` paths, leave nothing unused, add no macros, make no formatting-only changes, and keep every file under the 500-line ceiling `build_ceiling::check` enforces.

</step-2>

<step-3>

### Step 3: Replace the three `TempDir` copies with `tempfile` [completed]

- Component: Shared helpers
- Tests:
  - `cargo nextest run --locked -p runvil --all-features`
  - `cargo nextest run --locked -p runvil-engine --all-features`
  - `cargo nextest run --locked -p runvil-vfs --all-features` (includes the renamed `the_manifest_declares_no_normal_or_build_dependencies` and the updated table tests)
  - `cargo nextest run --locked -p build-xtask --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-engine -p runvil -p runvil-vfs --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "struct TempDir|no tempfile dependency|never weaken|no dependencies at all|no dependencies of any kind" crates/runvil crates/runvil-internal/engine crates/runvil-internal/vfs` (must print nothing)
  - `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly the 14 expected facade lines below, each marked `<=`, and nothing marked `=>`)
  - `cargo nextest list --locked -p runvil-engine --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly one line, marked `=>`, naming `every_shipped_prompt_parses_offline` in the Engine suite's `parsing` module)
- Expected facade lines: `shipped::every_shipped_prompt_parses_offline`, `auto_traits::a_run_can_move_between_threads_between_calls`, `auto_traits::the_run_configuration_and_outcome_types_are_send_sync_and_static`, `auto_traits::a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin`, `auto_traits::events_and_the_tool_catalog_and_errors_cross_threads`, `auto_traits::vfs_handles_and_accesses_are_send_and_sync`, `tools::tool_output_carries_its_trust_and_a_failure_shows_only_its_message`, `metrics::a_canned_completion_measures_nothing_until_metrics_are_attached`, `ids::a_model_started_task_is_abandoned_when_its_owning_section_returns`, `prompt::declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`, `model::a_text_completion_served_under_the_bound_models_name_becomes_the_result`, `plugins::a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`, `plugins::a_prelude_on_the_context_defines_a_function_the_section_calls`, and `replay::flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`.
- Setup: read the root `AGENTS.md`, `crates/runvil-internal/engine/AGENTS.md`, and `crates/runvil-internal/vfs/AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. The baseline lists `facade-before.txt` and `engine-before.txt` were saved in `../cabinet/_scratch/vibe-coder-facade-suite-dedup/` before Step 1; if one is missing, rebuild it from a `git worktree` of the baseline commit `30a66c78d` in that directory, never from an edited tree.

Work:

- Manifests: add `tempfile.workspace = true` under `[dev-dependencies]` in `crates/runvil/Cargo.toml`, `crates/runvil-internal/engine/Cargo.toml`, and `crates/runvil-internal/vfs/Cargo.toml` (a new table there), each with a one-line comment above it, like its neighbors, saying the suite uses temporary directories. `tempfile = "3"` is already a workspace dependency and already in `Cargo.lock`, and the Engine manifest guard (`crates/build-xtask/src/engine_deps.rs`) does not check dev-dependencies.
- The vfs rule, by the user's choice (Decision Record): apply every vfs bullet under Technical Design. `is_dependency_table` checks only `dependencies` and `build-dependencies`. The manifest test is renamed `the_manifest_declares_no_normal_or_build_dependencies`, with its doc saying dev-dependencies serve only the tests. The two table tests move their `dev-dependencies` cases to the passing side. The rule's wording changes in `Cargo.toml` (drop "never weaken it"), the `src/lib.rs` crate doc, `build.rs`, and `README.md`, so each states that the library is std only with no normal or build dependencies.
- `crates/runvil-internal/vfs/src/real/tests.rs` and its `atomicity.rs`, `links.rs`, and `semantics.rs` submodules: delete `TempDir` and use `tempfile::tempdir()`, mapping `io::Error` into `VfsError` where a test returns `VfsError`.
- `Cargo.lock`: run `cargo metadata --format-version 1 | Out-Null` once, without `--locked`, so the lock file records the three new edges. Its diff must only add `"tempfile",` to the `runvil`, `runvil-engine`, and `runvil-vfs` entries. Commit it with the step; every other command keeps `--locked`.
- `crates/runvil/tests/suite/prepare.rs`: delete `TempDir`, its `Drop` impl, and its "no tempfile dependency" doc. In `two_runs_writing_the_same_real_file_through_the_shared_base_conflict`, take `let temp = tempfile::tempdir()?;` and use `temp.path()` at both `temp.0` sites.
- `crates/runvil-internal/engine/src/execute/tests/suite/vfs.rs`: make the same deletion. In `fanout_interleaving_is_invariant_across_memory_and_real_backends`, which returns nothing, take `tempfile::tempdir().expect("the temp dir creates")` and use `temp.path()` at both `temp.0` sites.
- `crates/runvil/tests/suite/main.rs` keeps `#![expect(clippy::expect_used, ...)]`, because `prepare.rs`'s `expect_err("run b conflicts with run a's live claim")` still uses it.
- The facade suite may use std, `serde_json`, and `tempfile`, and names Runvil items by `runvil::` paths. Leave nothing unused, add no macros, and make no formatting-only changes.

</step-3>

<step-4>

### Step 4: Share the model, descriptor, note, and run-text helpers [completed]

- Component: Shared helpers
- Tests:
  - `cargo nextest run --locked -p runvil --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil --all-targets --all-features`
  - `cargo fmt --all --check`
  - `rg -n "RunResult::Ok" crates/runvil/tests/suite` (must list only `support/scenario.rs`)
  - `rg -n "ModelDescriptor::new|ToolDescriptor::new|NonZeroU32|include_str!|\bNOTE\b" crates/runvil/tests/suite` (must list only `support.rs`)
  - `rg -n "macro_rules!" crates/runvil` (must print nothing)
  - `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly the 14 expected facade lines below, each marked `<=`, and nothing marked `=>`)
- Expected facade lines: `shipped::every_shipped_prompt_parses_offline`, `auto_traits::a_run_can_move_between_threads_between_calls`, `auto_traits::the_run_configuration_and_outcome_types_are_send_sync_and_static`, `auto_traits::a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin`, `auto_traits::events_and_the_tool_catalog_and_errors_cross_threads`, `auto_traits::vfs_handles_and_accesses_are_send_and_sync`, `tools::tool_output_carries_its_trust_and_a_failure_shows_only_its_message`, `metrics::a_canned_completion_measures_nothing_until_metrics_are_attached`, `ids::a_model_started_task_is_abandoned_when_its_owning_section_returns`, `prompt::declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`, `model::a_text_completion_served_under_the_bound_models_name_becomes_the_result`, `plugins::a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`, `plugins::a_prelude_on_the_context_defines_a_function_the_section_calls`, and `replay::flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`.
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. The baseline list `facade-before.txt` was saved in `../cabinet/_scratch/vibe-coder-facade-suite-dedup/` before Step 1; if it is missing, rebuild it from a `git worktree` of the baseline commit `30a66c78d` in that directory, never from an edited tree.

The harness is `crates/runvil/tests/suite/support.rs` and `crates/runvil/tests/suite/support/scenario.rs`. Every file named below is under `crates/runvil/tests/suite/`.

Work:

- New helpers in `support.rs`, `pub(super)` like their neighbors:
  - `model(name: &str, window: u32, thinking: ThinkingMode) -> TestResult<ModelDescriptor>`: id `ModelId::gateway(name)`, one fixed description, and `NonZeroU32::new(window).ok_or("a context window is never zero")?`. `canned()` becomes `model("canned", 8_192, ThinkingMode::Never)` and keeps its doc on why every scenario defaults to it.
  - `descriptor(id: &str, schema: Value) -> TestResult<ToolDescriptor>`: `ToolId::parse(id)?` and one fixed description. A test that needs `.structured(true)` chains it.
  - `note(value: &str) -> String`: the one-section `greeter` prompt that writes the Lua expression `value` to `note.md` and returns `store.read('note.md')`. It replaces the `NOTE` constant, which goes.
- `Trace::text(&self) -> TestResult<&str>` in `support/scenario.rs` returns the text of `RunResult::Ok`, and for any other result fails with an error naming it, such as `expected RunResult::Ok, got {result:?}`. The tool descriptor in `Scenario::prepare` becomes `descriptor(tool.id, json!({ "type": "object" }))?`.
- Apply `model`:
  - `prepare.rs`: delete `current_model`. `report` and `every_declared_role_resolves_to_the_current_model` call `model("current", ...)` with their own windows and thinking modes.
  - `prompt.rs`, in `a_role_check_by_hand_finds_the_same_shortfall_prepare_reports`: `model("canned", 200_000, ThinkingMode::Always)?`.
  - `model.rs`: delete `fast`. `run_asking` and the catalog test use `model("fast", 32_000, ThinkingMode::Never)?`, and the catalog test's second model is `model("deep", 200_000, ThinkingMode::Always)?`. Move the suite's one `ModelId::new(ModelId::GATEWAY, "deep")` call into the `catalog.get` lookup, so the test still shows the two id constructors agree.
- Apply `descriptor`:
  - `prepare.rs`: `a_prepared_context_keeps_the_caller_supplied_catalog` uses `descriptor("web/fetch", ...)?.structured(true)` with its `url` schema, and `prepare_reports_nothing_for_a_declared_plugin_the_catalog_lacks` uses `descriptor("other/search", ...)?` with its empty-properties schema.
  - `shipped.rs`: delete `web_tool`. The catalog takes `descriptor("web/fetch", ...)` and `descriptor("web/search", ...)` with the existing one-string `query` schema.
- Apply `note`: `greeter(NOTE)` becomes `note("'hello'")` in `greeter.rs`, `replay.rs`, `cancel.rs`, and `vfs.rs`. `event.rs`'s own `NOTE` becomes `note("'hello ' .. args")`. The `sys.when` write-then-read greeter in `timestamp.rs` becomes `note("sys.when")`.
- Apply `Trace::text`. Every `assert!(matches!(<trace>.result, RunResult::Ok(text) if text == X))` becomes `assert_eq!(<trace>.text()?, X)`, and every `let RunResult::Ok(..) = ... else` block becomes a `text()` call:
  - `effect.rs`: the reversed and in-order block becomes `assert_eq!(reversed.text()?, in_order.text()?)` plus a check against `"hi there / HI THERE"`, and `assert!(matches!(trace.result, RunResult::Ok(_)))` becomes `trace.text()?;`.
  - `event.rs` has four sites, and its captured-and-quiet tuple becomes two checks that each equal `"HI THERE"`. `greeter.rs` has three, `ids.rs` one, `replay.rs` two, `timestamp.rs` one, and `shipped.rs` one, the `summary` block, still checked against `"a factual summary"`.
  - `plugins.rs`: the roster loop checks `assert_eq!(scenario.run(&source)?.text()?, expected)`.
  - `vfs.rs`: `run_note` returns the trace's text as `TestResult<String>`, and both its checks become `assert_eq!(run_note(..)?, "hello")`.
  - The `RunResult::Cancelled` and `RunResult::Failure` matches stay.
- `parsing.rs`: `InvalidFixture` and `INVALID_FIXTURES` become one array of `(file stem, ParseErrorKind, message fragment)` tuples, in this order:
  - `("missing-h1", ParseErrorKind::Structure, "requires an H1")`
  - `("unrecognized-lua-label", ParseErrorKind::Fence, "unrecognized fence label")`
  - ``("malformed-epilog", ParseErrorKind::Lua, "section `Transform` epilog")``
  - `("list-h3-non-list-content", ParseErrorKind::List, "empty bullet item")`
  - `invalid_prompt_files_report_public_error_contracts` reads `tests/prompts/invalid/<stem>.md` under `env!("CARGO_MANIFEST_DIR")` at run time, parses it under the name `invalid/<stem>.md`, and keeps both failure messages: the wrong kind with the error, and the missing fragment with the error. It may return `TestResult`.
- Every test keeps its name and expected values. Drop each import the replacements leave unused, such as `NonZeroU32`, `ModelId`, `ToolId`, `RunResult`, and `greeter`. Re-export `Trace` from `support` only if a test names the type. Name Runvil items by `runvil::` paths, add no macros, make no formatting-only changes, and keep every file under the 500-line ceiling `build_ceiling::check` enforces.

</step-4>

<step-5>

### Step 5: Trim restating prose and confirm the exit criteria [completed]

- Component: Prose trim
- Tests:
  - `cargo nextest list --locked -p runvil --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/facade-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly the 14 expected facade lines below, each marked `<=`, and nothing marked `=>`)
  - `cargo nextest list --locked -p runvil-engine --all-features | Out-File -Encoding utf8 ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt; Compare-Object (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-before.txt) (Get-Content ../cabinet/_scratch/vibe-coder-facade-suite-dedup/engine-after.txt) | ForEach-Object { "$($_.SideIndicator) $($_.InputObject)" }` (must print exactly one line, marked `=>`, naming `every_shipped_prompt_parses_offline` in the Engine suite's `parsing` module)
  - `python -c "import pathlib,tiktoken; e=tiktoken.get_encoding('o200k_base'); print(sum(len(e.encode(p.read_text(encoding='utf-8'), disallowed_special=())) for p in pathlib.Path('crates/runvil/tests/suite').rglob('*.rs')))"` (report the token count against 17,361 at baseline; the target is about 13,500, and a higher count does not fail the step)
  - `(Get-ChildItem crates/runvil/tests/suite -Recurse -Filter *.rs | ForEach-Object { (Get-Content $_.FullName).Count } | Measure-Object -Sum).Sum` (report the line count against 2,041 at baseline, for reference only)
  - `rg -n "macro_rules!" crates/runvil` (must print nothing)
  - `git diff --exit-code 30a66c78d -- crates/runvil/public-api.txt` (must print nothing)
  - `npm ci --prefix crates/workshop`
  - `npm ci --prefix crates/gateway/config-ui/ui`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo check -p gateway --no-default-features`
  - `cargo fmt --all --check`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc -p runvil --no-deps`
  - `cargo +nightly-2026-09-05 xtask api --check`
  - `node --test crates/workshop/ui/test/docs-claims.mjs`
- Expected facade lines: `shipped::every_shipped_prompt_parses_offline`, `auto_traits::a_run_can_move_between_threads_between_calls`, `auto_traits::the_run_configuration_and_outcome_types_are_send_sync_and_static`, `auto_traits::a_cancel_handle_is_send_sync_and_static_and_its_future_is_unpin`, `auto_traits::events_and_the_tool_catalog_and_errors_cross_threads`, `auto_traits::vfs_handles_and_accesses_are_send_and_sync`, `tools::tool_output_carries_its_trust_and_a_failure_shows_only_its_message`, `metrics::a_canned_completion_measures_nothing_until_metrics_are_attached`, `ids::a_model_started_task_is_abandoned_when_its_owning_section_returns`, `prompt::declared_arguments_take_json_and_an_undeclared_prompt_takes_plain_prose`, `model::a_text_completion_served_under_the_bound_models_name_becomes_the_result`, `plugins::a_plugin_holds_its_own_tools_and_a_two_part_name_is_not_one`, `plugins::a_prelude_on_the_context_defines_a_function_the_section_calls`, and `replay::flags_from_a_newer_record_keep_the_bits_this_version_does_not_name`.
- Setup: read the root `AGENTS.md` before editing. Run cargo commands one at a time in PowerShell, without `2>&1`, and judge each by its result lines and `$LASTEXITCODE`. The baseline lists `facade-before.txt` and `engine-before.txt` were saved in `../cabinet/_scratch/vibe-coder-facade-suite-dedup/` before Step 1; if one is missing, rebuild it from a `git worktree` of the baseline commit `30a66c78d` in that directory, never from an edited tree.

Work:

- Trim the comments in the harness (`crates/runvil/tests/suite/support.rs` and `crates/runvil/tests/suite/support/scenario.rs`) and in every test file under `crates/runvil/tests/suite/`. Change comments only: no code, test-name, or formatting changes, and no `rustfmt.toml`.
- Keep the comments that state a constraint the code can't show:
  - the driving rules in `scenario.rs`'s module doc;
  - why every scenario defaults to `canned()`;
  - why a held effect gets `Dropped`;
  - the one pointer, in `prepare.rs`'s module doc, saying which prepare cases live in the Engine suite.
- Delete comments that repeat an item's name, restate the next line, or describe Lua that sits right below them.
- On `Scenario`, each field doc duplicates its builder method's doc, so keep one of each pair.
- Each test file's module doc keeps one sentence saying what the file tours.
- Doc comments that stay must still pass clippy's pedantic doc lints, with identifiers in backticks. They also follow the root `AGENTS.md` wording rules: code in `crates/runvil` says "the caller" and never mentions the Host.
- Report the token count against 17,361 and the line count against 2,041.

</step-5>

</execution-plan>
