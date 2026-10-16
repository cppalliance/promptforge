---
name: Require run debt removal
overview: "Remove the debt the require plan's eight commits (43245d4b5..67be02738) left behind: prompt-line mapping that breaks on reused or long Lua chunk names, plus one orphaned public mapper with stale docs. Two reversible work items, no public, persisted, or wire changes."
todos:
  - id: chunk-ids
    content: "D1-1, D1-2: compile prompt blocks under a short `line {source_line}` chunk name, map through it to the display location, add the five regression tests"
    status: completed
  - id: gate-mapper
    content: "D1-3: gate `map_runtime_error` behind test-support, add the parser dev-dependency, rewrite the stale docs"
    status: completed
  - id: exit-checks
    content: "Run the exit checks: workspace nextest, clippy and docs with warnings denied, `cargo test -p build-xtask`, fmt"
    status: completed
isProject: false
---

# Require run debt removal

<product-contract>

## Product Requirements

- Scope and target work
  - Repository: `c:\Users\Vinnie\cursor\runvil`. Baseline `43245d4b5` (`upstream/master` as of the last fetch, also the merge base). Endpoint and disposition `67be02738` (`HEAD`, clean worktree).
  - Target: the eight commits of the require run, `22e2701c2`, `42282ddfe`, `60aa72619`, `278ab4c0a`, `2aa8d4e18`, `1cd835acb`, `019d1b7ee`, `67be02738`. Design record: `vibe/2026-10-12-1-require-replaces-lua-shared.md`.
  - All paths below are under `crates/runvil-internal/` unless stated.
- Cleanup goals
  - A runtime error in any prompt Lua block, including a required library, maps to its prompt-file line whatever the section's title length or how many sections share that title.
  - No Lua block's error is ever rewritten through another block's starting line, H1 blocks included.
  - `LuaProgram::map_runtime_error` is test-only, and no doc comment presents it as the production mapping path.
- Non-goals
  - Mapping errors raised in a function that one block defined and another block called. Today mapping covers the started block and the libraries loaded into its VM; such errors stay unmapped, and that is correct as long as they are never mapped wrongly.
  - Changing display locations (``section `T` prologue``, ``section `T` lua N``, ``section `T` (line L) prologue``, ``H1 `T` lua``) or the mapped output form `{location}:{line}:`.
  - Any change to `require` behavior, events, reach, or caching.
- Success criteria
  - Every regression test in the Testing Plan fails at `67be02738` and passes after the change.
  - The full suite, clippy with warnings denied, the docs builds with `-D warnings`, and `cargo test -p build-xtask` pass.

## Functional Specification

### Debt Inventory

- Debt added
  - D1-2 (worsened): long chunk names lose prompt-line mapping.
    - Evidence: `60aa72619` lengthened block names. `parser/src/build.rs::block_label` adds ` (line L)` when another section shares the title, and `parser/src/fence.rs::lua_block_location` adds an ordinal to middle blocks. `lua/src/program.rs::compile_chunk` passes the display location to `set_name`, so Lua renders the chunk as `[string "<location>"]`. Vendored Lua 5.5.0 sets `LUA_IDSIZE` to 60 (`target/debug/build/mlua-sys-*/out/include/luaconf.h`), and `luaO_chunkid` keeps a string chunk name whole only when it is under 45 bytes. Longer names are cut and end in `...`. `map_chunk_line_to_absolute` matches only the full marker, so a cut name passes through unmapped.
    - Relationship to target: the cap already broke unique titles of 26 bytes or more at the baseline. `60aa72619` extends the break to shared titles of about 15 bytes once the heading line reaches 100, for example ``section `Research Worker Notes` (line 120) prologue`` (51 bytes). The same section was 40 bytes and mapped at `43245d4b5`.
    - Impact: errors in such sections report chunk-relative lines, which contradicts the design record's acceptance criterion that an error in library code shows the library's location and its prompt-file line. Non-ASCII titles reach the cap sooner.
    - Reversal cost: low. Chunk names are process-local (bytecode is never persisted), and `LuaProgram` is absent from `crates/runvil/public-api.txt`. Tests that pin raw markers need edits.
    - Target state: mapping identity is independent of display-name length.
- Cheap fixes
  - D1-1: H1 Lua blocks share one chunk name. `parser/src/fence.rs` line 64 names every H1 Lua block ``H1 `{title}` lua``. The doc on `lua_block_location` in the same file states "no two blocks of a prompt may share one". When the started H1 block's program maps an error marker from another H1 block, it rewrites that marker through the wrong starting line. `scheduler/step.rs` then turns that error into a `RequirementsUnmet` notice carrying the wrong line. This predates the target, in a file the target touched twice (`60aa72619`, `019d1b7ee`).
  - D1-3: `lua/src/program.rs::LuaProgram::map_runtime_error` (line 174) lost its only production caller when `60aa72619` switched `SectionVm::map_chunk_failure` to `map_runtime_error_among`. Its remaining callers are tests: `engine/src/lua/tests/errors.rs:427`, `lua/src/tests/diagnostics.rs:257`, and `parser/src/tests/line_mapping.rs` lines 12, 164, and 205. Doc comments in `lua/src/vm/run.rs` (lines 104-106 on `start_block_coro`, 237-239 on `block_failure`, 258-260 on `map_chunk_failure`) still name it as the mapping path. No plan or `Deferred:` trailer reserves it. The same target gated its other orphan, `run_loaded_with_control`, behind `test-support`.
- Exposed pre-existing debt: none.
- Rejected candidates (16)
  - Residual-but-acceptable (8):
    - D1-7: a load abandoned when the run ends emits no closing event, the same gap chunk events have.
    - D1-8: an H1 library failure ends the run as `RequirementsUnmet`, which the design accepts.
    - D1-9: the walk running a library is a documented sharp edge.
    - D1-13: duplication in test-only helpers.
    - D1-14: setup ordering is documented, and its store-conflict repair is verified by `engine/src/execute/tests/preludes.rs:211`.
    - D1-15: the second reach view shares `find_one` and `not_found`.
    - D1-17: load-time callbacks resolve from the library's position, as the design specifies.
    - D1-19: the public surface is the planned one.
  - Weak/speculative (6): D1-4, D1-5, D1-6, D1-10, D1-12, and D1-18. None has a demonstrated consequence. D1-4 and D1-5 are deferred with revisit conditions below.
  - False (1): D1-11. `REQUIRE_PROGRAM` is an immutable `LazyLock`, not mutable global state.
  - Unrelated pre-existing (1): D1-16. The `lua shared` scan covering the preface predates the target; only the message changed.

</product-contract>
<implementation-contract>

## Technical Design

- Shared cause of D1-1 and D1-2: mapping identifies a block by finding its full display location inside Lua's rendered chunk id. The display string therefore has to be both unique and under 45 bytes, and nothing enforces either. Mapping identity is separated from the display name.
- `lua/src/program.rs`
  - `LuaProgram::compile` (prompt blocks; production callers are only `parser/src/fence.rs` line 65 and `parser/src/build.rs` line 197) compiles under a short chunk name derived from the block's starting prompt line, proposed `line {source_line}`, which Lua renders as `[string "line 120"]`. Two Lua blocks of one prompt never start on the same line, and every owner a VM maps through comes from one parsed prompt, so the name is unique among owners and stays at 15 bytes or less.
  - `LuaProgram::compile_internal` keeps its explicit name (the embedded `@crates/...` chunks). The program records the name it was compiled under, for example in a new private field.
  - `location()` keeps returning the display location.
  - `map_chunk_line_to_absolute` takes owners as (chunk name, display location, source line). It matches `[string "{chunk name}"]:N:` and writes `{display location}:{absolute}:`, plus the single leading tag in the same form. The mapped output form is unchanged, so `location:line:` tags in existing tests keep their text. The overflow, first-owner tie-break, and already-mapped pass-through rules stay.
  - `map_runtime_error_among` builds owners from each program's chunk name, display location, and source line.
- D1-3
  - Gate `LuaProgram::map_runtime_error` with `#[cfg(any(test, feature = "test-support"))]`.
  - In `parser/Cargo.toml`, add `runvil-lua` with the `test-support` feature under `[dev-dependencies]` (line 18). `engine/Cargo.toml` already enables it for dev builds.
  - Rewrite the docs that name it: the three `lua/src/vm/run.rs` comments above, and the link from `map_runtime_error_among`'s doc in `program.rs`. They should name `map_runtime_error_among` as plain code, not an intra-doc link, because it is crate-private and the gated method is absent from non-test docs.
- Parser docs: update the `lua_block_location` doc (`parser/src/fence.rs` lines 267-274) and the `block_label` doc (`parser/src/build.rs` near line 275). They currently justify unique labels by mapping. After the change, labels are display names and the chunk name carries mapping identity.
- No public interface, persisted format, wire format, ownership, dependency-direction, or trust-boundary change.

</implementation-contract>
<verification-contract>

## Testing Plan

- Regression tests. Each one fails at `67be02738`, and the failing assertion is named.
  - D1-1, in `parser/src/tests/line_mapping.rs` or `parser/src/tests/h1_fences.rs`.
    - Setup: parse an H1 with two Lua blocks separated by prose, built like `h1_fences.rs::h1_plain_lua_and_prose_are_live_blocks` (`prompt.h1_blocks`), with `assert(false)` in the first block. Load and call the first block's program to get the raw error.
    - Assertion: `second.map_runtime_error(&raw_error).to_string() == raw_error.to_string()`.
    - Why it fails at the endpoint: both programs are named ``H1 `T` lua``, so the second rewrites the first block's marker through its own start line.
    - Positive check: `assert_runtime_error_line(first, <absolute line>)`, which passes before and after.
  - D1-2, in `lua/src/tests/diagnostics.rs`.
    - Setup: `LuaProgram::compile` with a display location of 60 or more bytes, `source_line` 40, and source `assert(false)`. Call it and map the error with `map_runtime_error`.
    - Assertion: the message contains `{location}:40:`.
    - Why it fails at the endpoint: Lua cuts the chunk name to 45 bytes plus `...`, so the marker never matches.
  - D1-2, in `parser/src/tests/line_mapping.rs`, covering the older unique-title range.
    - Setup: a section `## Gather Background Research Notes` (30-byte title, 49-byte prologue name) whose prologue is `assert(false)`.
    - Assertion: `assert_runtime_error_line(program, <absolute line>)`.
    - Why it fails at the endpoint: the cut marker leaves only the chunk-relative `:1:` in the message.
  - D1-2, in `parser/src/tests/line_mapping.rs`, covering the range the target worsened.
    - Setup: sections `### Research Worker Notes` under `## A` and under `## B`. Pad `## A` with prose lines so the second heading sits at line 100 or later.
    - Assertion: `assert_runtime_error_line(under_b, <absolute line>)`.
    - Why it fails at the endpoint: the display name ``section `Research Worker Notes` (line 1NN) prologue`` is 51 bytes, so Lua cuts it.
    - Extra check: `under_a.map_runtime_error(&raw_error)` leaves the error unmapped, before and after.
  - D1-2, at the engine level, in `engine/src/execute/tests/suite/require/failures.rs`.
    - Setup: a section requires `## Shared Research Helper Functions` (32-byte title, 51-byte prologue name), and the library's only block raises while loading.
    - Assertion: the run's error message contains ``section `Shared Research Helper Functions` prologue:<absolute line>:``.
    - Why it fails at the endpoint: the cut chunk name leaves the error unmapped.
- Existing checks to keep and reuse
  - `parser/src/tests/line_mapping.rs::every_block_of_a_section_maps_only_its_own_errors` and `::sections_sharing_a_title_get_locations_named_by_heading_line` keep passing, since display locations are unchanged.
  - The `require` error suites under `lua/src/tests/require/` and `engine/src/execute/tests/suite/require/` keep passing.
  - Tests that pin raw unmapped markers, such as `lua/src/tests/diagnostics.rs` lines 134-178, move to the `[string "line N"]` form.
  - Tests that load several programs with the same `source_line` and expect each to map separately get distinct lines.
- D1-3 checks
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`.
  - `$env:RUSTDOCFLAGS='-D warnings'` with `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` and `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. These catch links to the gated method.
  - `cargo test -p build-xtask`, which includes the test-support leak check.
  - A search for `map_runtime_error(` finds only test callers.
- Exit: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features` and `cargo fmt --all --check` pass. No structural ratchets are added.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - D1-1 and D1-2 share one remedy: a short chunk name derived from the block's starting line, kept separate from the display location (high confidence: it removes both the reuse and the length failure, and changes no mapped output). Tradeoff: an error that stays unmapped, such as a cross-block call, shows `[string "line 120"]:3:` instead of the display name. That form still points at a real prompt line.
  - Display locations stay as they are, including the ordinals and ` (line L)` from `60aa72619`. Removing them would churn pinned test strings for no behavior gain.
  - D1-3 gates `map_runtime_error` behind `test-support` instead of deleting it, because tests in three crates use it. This follows the target's own handling of `run_loaded_with_control`.
- User-resolved architecture choices: none. No remediation is hard to reverse.
- Rejected alternatives
  - Numbering H1 blocks only, as ``H1 `T` lua N``. It fixes D1-1 but lengthens names and leaves D1-2.
  - Shortening or hashing long titles. It keeps identity tied to display text and still needs a length budget.
  - Accepting a cut marker when its 45-byte prefix is unique. Library programs can share prefixes, so it could map wrongly.
  - Mapping through every block a VM has run. That is a feature beyond debt removal.
- Assumptions and risks
  - The 45-byte rule comes from upstream Lua 5.5.0 `luaO_chunkid`. Only `LUA_IDSIZE 60` and the version were confirmed from vendored headers. The D1-2 lua-crate regression test settles the rule directly at the endpoint.
  - Analysis limits: no repository code was executed during analysis; `upstream/master` was not refreshed; `1cd835acb` (test migration) was spot-checked, not read line by line.

### Deferred and Out of Scope

- D1-4, loader bookkeeping on a memory error inside `begin_load` (`lua/src/require.rs::begin_load_capture` pushes onto `Loading` and steps the depth before mlua builds the return tables). Revisit if `jump` refuses with the library-loading message outside a load, or when the loader is next changed. Then move fallible work before the push, or have `end_load` pop only a matching top entry.
- D1-5, a cached `require` still resolves and deep-copies the library (`engine/src/execute/walk_target.rs::build_library` clones every `LuaProgram`). The copy sits on the Rust heap, outside the Lua memory limit. Revisit when a measurement shows heap growth for a looped cached `require` of a large library. Then build the `Library` only on a cache miss, or hold programs behind `Arc`.
- D1-6, library-load events name the library by heading only. Revisit when a consumer outside tests must tell same-heading libraries apart.
- Mapping errors raised in a function one block defined and another block called. Revisit if authors report unmapped cross-block lines.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked` builds only the gateway, the workspace's one default member (`crates/gateway/app`); the desktop app is `cargo build --locked -p workshop`, and one crate builds with `cargo build --locked -p <crate>`. The gateway config UI and `workshop-server` build scripts bundle TypeScript through `build-ui`, so a fresh checkout first needs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` (both `node_modules` trees are present on this machine). The Engine crates' build scripts, and most others, run `build_ceiling::check()` (`runvil-vfs` includes its source by path because it declares no dependencies), which fails the build when any Rust file under the crate's `src`, `tests`, `benches`, or `examples` exceeds 500 lines.
- Focused test command pattern: `cargo nextest run --locked -p <crate> --all-features <test-name-substring>`, for example `cargo nextest run --locked -p runvil-parser --all-features line_mapping`. Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. On the Engine crates `--all-features` turns on the crate's own `test-support`. A single JS test file: `node --test <path-to-file.mjs>`.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features`, with the same workshop exception. This plan's components are `runvil-lua`, `runvil-parser`, and `runvil-engine`; the other Engine packages are `runvil` (facade), `runvil-plugin`, `runvil-types`, `runvil-vfs`, and `runvil-model-client`. Workshop UI packages: `npm test --workspaces --if-present` from `crates/workshop`; gateway config UI: `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`, then `node --test crates/workshop/ui/test/docs-claims.mjs` (the wording scan; it needs no `npm ci`). The workspace run includes `build-xtask`, whose boundary and structural checks also run alone as `cargo test -p build-xtask` and print on demand with `cargo xtask tidy`. Nextest settings are in `.config/nextest.toml` (60 s slow-test period, terminated after three).
- Linter command: `CARGO_BUILD_WARNINGS=deny cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`, then `CARGO_BUILD_WARNINGS=deny cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, plus `cargo check -p gateway --no-default-features` for the headless build shape. The shell here is PowerShell, so set `$env:CARGO_BUILD_WARNINGS='deny'` first. Never run a standalone `cargo check --workspace`; clippy already covers it. The pre-push hook also runs `cargo deny --log-level error check` (cargo-deny 0.20.2 is installed).
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: with `RUSTDOCFLAGS="-D warnings"` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`), run `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, then `cargo doc -p runvil --no-deps` and `cargo doc -p harness --no-deps` for the facades with default features, then `cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`. When the `runvil` facade's public surface changes, also run `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`, installed on this machine), which compares against `crates/runvil/public-api.txt`; `--bless` rewrites the listing. `LuaProgram` does not appear in that listing.
- Test placement and naming conventions:
  - A module's unit tests live in a sibling file wired as `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `lua/src/coro.rs` beside `coro-tests.rs`, `parser/src/build.rs` beside `build-tests.rs`); extra topic files take `<stem>-tests-<topic>.rs` (`messages-tests-view.rs`, `prelude-tests-environment.rs`). A few modules keep an inline `mod tests { }`.
  - Crate-wide tests use a `tests.rs` holding shared helpers plus a `tests/` directory of topic modules: `lua/src/tests.rs` (topics such as `diagnostics.rs` and `require.rs`, with `tests/require/errors.rs`, `prose.rs`, `reports.rs`), `parser/src/tests.rs` (`line_mapping.rs`, `h1_fences.rs`, `section_fences.rs`, and others), `engine/src/execute/tests.rs` (`tests/suite/require/` with `effects.rs`, `failures.rs`, `loading.rs`, `prose.rs`, `reach.rs`, `walk.rs`, plus `tests/scheduler/`), and `engine/src/lua/tests/` (including `errors.rs`).
  - Integration tests compile as one binary per crate: `tests/suite/main.rs` for the `runvil` and `harness` facades, `tests/it/main.rs` for eighteen others. Each topic is a `mod` file. Prompt fixtures sit under `crates/runvil-internal/engine/tests/prompts/` (`execution/`, `invalid/`, `valid/`, including `valid/require-library.md`) and `crates/runvil/tests/prompts/invalid/`.
  - Test functions are snake_case sentences stating the behavior, such as `every_block_of_a_section_maps_only_its_own_errors`. File-local assertion helpers sit at the top of the topic file, such as `assert_runtime_error_line` in `parser/src/tests/line_mapping.rs`.
  - Test-only items are gated `#[cfg(any(test, feature = "test-support"))]` (for example `run_loaded_with_control` in `lua/src/vm/run.rs`). `runvil-lua`, `runvil-parser`, `runvil-engine`, and `runvil-plugin` each define `test-support`; gateway crates use `test-fixtures`. Companion crates enable these only from `[dev-dependencies]`: `runvil-engine` does so for `runvil-lua` and `runvil-parser`, while `runvil-parser`'s only dev-dependency today is `mlua`. `build-xtask`'s `test_support_leak` check fails when a non-dev dependency table enables a runvil or Harness crate's `test-support`.
  - Doctests are banned: `build-xtask`'s `no_doctests` check fails on any compiled code block in a doc comment. Benches use criterion with `harness = false` and `required-features = ["test-support"]` (`lua/benches/surface.rs`, `engine/benches/models_loop.rs`).
  - JS tests are `*.test.mjs` beside the source or `*.mjs` under `test/`, run by `node --test`.
- Directory map:
  - `crates/`: all code. Top-level public crates (`runvil`, `runvil-plugin`, `harness`, `harness-gateway-client`, `gateway-api-types`, `gateway-api-discovery`), Plugins (`plugin-web`, `plugin-mcp`, `plugin-user-input`), shared utilities (`shared-error-source`, `shared-loopback`, and `shared-ui`, a TypeScript and CSS package rather than a crate), tooling (`build-xtask` for structural checks and `cargo xtask`, `build-workshop` behind the `cargo workshop` alias, `build-ui`, `build-user-guide`, `build-ceiling`, `build-llama-cuda`), and `workspace-hack` for cargo-hakari.
  - `crates/runvil-internal/`: private Engine crates `types`, `vfs`, `model-client`, `lua`, `parser`, `engine`.
    - `lua`: the sandboxed Lua runtime. `program.rs` holds `LuaProgram` (compile, bytecode, and runtime-error line mapping), `vm/` holds section VMs (`install.rs`, `run.rs`, `state.rs`), and `require.rs` holds `require` with its embedded `__impl_require.lua`. The other embedded sources are `__impl_globals.lua`, `__impl_coro.lua`, `__impl_tasks.lua`, `__impl_fanout.lua`, and `__impl_store.lua`, each loaded with `include_str!` under an `@crates/...` chunk name.
    - `parser`: turns one markdown prompt (frontmatter, H1, H1 blocks, nested sections) into alternating Lua and prose blocks; `fence.rs` and `build.rs` compile each Lua block and name its display location.
    - `engine`: the `Run` state machine that steps a parsed prompt and emits effects; `execute/` (including `execute/scheduler/step.rs` and `execute/walk_target.rs`), `lua/`, `model/`, `error/`, and `test_support/`.
    - `types`: shared vocabulary, including the `LuaLibraryLoadStarted`, `LuaLibraryLoadSucceeded`, and `LuaLibraryLoadFailed` events in `types/src/event.rs`.
  - `crates/harness-internal/runner`: the private Harness crate `harness-runner`.
  - `crates/gateway/`: private gateway crates; `app` is the `runvil-gateway` binary, beside `config`, `config-ui`, `local`, `logging`, `progress`, `protocol`, `routing`, `cloud-providers`, `web-search`, and the `stt/` speech subsystem whose public member is `api` (`gateway-stt`).
  - `crates/workshop/`: the Workshop Host; `desktop` (Tauri app, package `workshop`), `server` and `server-api`, tier crates `agents`, `gateway`, `menu`, `protocol`, `registry`, `run-log`, `status`, `support`, `user-state`, `workspace`, and the npm workspaces `ui`, `look`, `platform`.
  - `guide/`: user guide and docs site sources (`books`, `chrome`, `landing`). `prompts/`: example prompts. `tools/`: Node scripts for gateway sidecar staging and live TTS checks, each with a `.test.mjs`. `vibe/`: dated plan and run records, earlier ones archived in month folders. `images/`: README banners. `local/`: gitignored machine-local config and fixtures. `cabinet/`: untracked local staging.
  - `.github/workflows/`: CI (`ci.yml`), release, and nightly jobs. `.githooks/`: pre-commit runs fmt; pre-push runs docs-claims, the headless check, workspace clippy, and cargo-deny. `.config/`: nextest and hakari config. `.cargo/config.toml`: the `workshop` and `xtask` aliases, the Windows `rust-lld` linker with static CRT, and `CLIPPY_DISABLE_DOCS_LINKS`. `.cursor/rules/`: `workshop-architecture.mdc` and `workshop-spa.mdc`. Besides the root `AGENTS.md`, several crates carry their own, including `runvil-internal/lua`, `engine`, `types`, and `vfs`.
- Component boundaries (enforced by `cargo test -p build-xtask`):
  - Engine internal graph: `types` and `vfs` depend on no Engine crate (`vfs` has no dependencies at all); `model-client` on `types`; `lua` on `types`, `vfs`, and `model-client`; `parser` on `lua` and `types`; `engine` on all five. The `runvil` facade depends on all six, and `runvil-plugin` on `types` and `vfs`. Code that would make `lua` depend on `parser` stays in `lua`. Engine manifests name no `tokio`, `tokio-util`, `async-trait`, or `reqwest` outside `[dev-dependencies]`, except optional entries only `test-support` enables.
  - Engine crates (`runvil*`) depend on no gateway, workshop, Harness, or Plugin crate. Other families reach the Engine only through `runvil` and `runvil-plugin`; `crates/runvil-internal/` is private to those two.
  - Harness crates (`harness*`) depend outside their family only on `runvil`, `runvil-plugin`, and `workspace-hack`. Others reach the Harness only through `harness` and `harness-gateway-client`; `harness-runner` is private to `harness`.
  - Plugin crates depend only on `runvil-plugin`, `shared-*`, `workspace-hack`, and outside libraries. The only product crate allowed to depend on a Plugin is `harness-gateway-client`, which implements `plugin-web`'s `SearchProvider`.
  - Gateway crates depend on no runvil, workshop, or Harness crate. Their containers are private to the family, and Workshop sees the gateway only through `gateway-api-types` and `gateway-api-discovery`.
  - Workshop crates follow a one-way tier graph. The desktop app depends on no workshop crate except `workshop-server-api`, which depends only on `workshop-server`.
  - `shared-*` crates depend on no product crate.
- Conventions summary:
  - Engine, Harness, Host, and Plugin are capitalized terms with exactly one meaning each (root `AGENTS.md` Definitions). Engine crate docs, comments, and tests say "the caller" and never mention the Host; `docs-claims.mjs` enforces the wording.
  - Every member inherits strict workspace lints: clippy `all` and `pedantic` at deny, `unwrap_used` and `expect_used` denied outside tests, `#[allow]` banned in favor of `#[expect(..., reason = "...")]`, `unsafe_code` denied, `missing_docs` and `unreachable_pub` warned, broken and private intra-doc links denied. A crate-private item therefore cannot be an intra-doc link target from a public doc. `clippy.toml` bans installing process-global state outside binary entry points.
  - Crate roots open with a `//!` doc, many with a `## Invariants` section.
  - Source directories are flat: a subdirectory needs three or more files, otherwise use `parent-label.rs` siblings wired with `#[path]`. No Rust file may exceed 500 lines. Current sizes of files this plan edits: `lua/src/tests/diagnostics.rs` 365, `parser/src/build.rs` 367, `lua/src/vm/run.rs` 360, `lua/src/program.rs` 291, `parser/src/fence.rs` 291, `parser/src/tests/line_mapping.rs` 230, `engine/src/execute/tests/suite/require/failures.rs` 230, `parser/src/tests/h1_fences.rs` 198.
  - A removed Engine symbol is added to `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`; a symbol gated behind `test-support` is not removed.
  - Dependencies are declared once in `[workspace.dependencies]`, with comments explaining each pin and disabled feature.
  - Error and status messages are written for model consumption: concise, naming what is missing with required versus actual. Comments explain only non-obvious constraints.
  - Behavior changes ship with tests in the same change. JSON that reaches a recorder must round-trip exactly.

</project-survey>
<execution-plan>

## Execution Instructions

Objective: a runtime error in any prompt Lua block maps to its prompt-file line no matter how long or how shared the block's display name is, and `LuaProgram::map_runtime_error` becomes test-only with no doc presenting it as the production path. Path: Bounded, two steps, no components. Step 1 lands first so its regression tests are confirmed failing against unchanged endpoint code, and so `map_runtime_error` is still public when the new parser tests first compile. Step 2 then gates the method and adds the parser dev-dependency that keeps those tests compiling. Both steps edit docs in `lua/src/program.rs`. All paths are under `crates/runvil-internal/` unless stated.

<step-1>

### Step 1: Short line-based chunk names for prompt Lua blocks [completed]

- Component: `none`
- Tests:
  - `cargo nextest run --locked -p runvil-lua -p runvil-parser -p runvil-engine --all-features`
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy -p runvil-lua -p runvil-parser -p runvil-engine --all-targets --all-features`
  - `cargo fmt --all --check`

Fixes D1-1 and D1-2 by separating mapping identity from the display name.

- Debt being removed
  - D1-1: every H1 Lua block is named ``H1 `{title}` lua`` (`parser/src/fence.rs` line 64). When the started H1 block's program maps an error marker from another H1 block, it rewrites that marker through the wrong starting line, and `engine/src/execute/scheduler/step.rs` turns the error into a `RequirementsUnmet` notice carrying the wrong line.
  - D1-2: `compile_chunk` passes the display location to `set_name`. Vendored Lua 5.5.0 sets `LUA_IDSIZE` to 60, and `luaO_chunkid` keeps a string chunk name whole only when it is under 45 bytes; longer names are cut and end in `...`. `map_chunk_line_to_absolute` matches only the full `[string "<location>"]` marker, so a cut name passes through unmapped. Unique titles of 26 bytes or more already broke at `43245d4b5`; `60aa72619` extended the break to shared titles of about 15 bytes once the heading line reaches 100. Non-ASCII titles reach the cap sooner.
- Changes in `lua/src/program.rs`
  - `LuaProgram::compile` (prompt blocks; its only production callers are `parser/src/fence.rs` line 65 and `parser/src/build.rs` line 197) compiles under the chunk name `line {source_line}`, which Lua renders as `[string "line 120"]`. Two Lua blocks of one prompt never start on the same line, and every owner a VM maps through comes from one parsed prompt, so the name is unique among owners and stays at 15 bytes or less.
  - `LuaProgram::compile_internal` keeps its explicit `@crates/...` name. The program records the name it was compiled under in a new private field.
  - `compile_chunk` sets the recorded chunk name, not the display location.
  - `location()` keeps returning the display location.
  - `map_chunk_line_to_absolute` takes owners as (chunk name, display location, source line). It matches `[string "{chunk name}"]:N:` and writes `{display location}:{absolute}:`, and rewrites the single leading tag in the same form. Keep the overflow, first-owner tie-break, and already-mapped pass-through rules.
  - `map_runtime_error_among` builds owners from each program's chunk name, display location, and source line; `map_runtime_error` uses the same owner form.
  - Update the docs that describe the `[string "{location}"]` marker (`program.rs` lines 121 and 229 at `67be02738`) and the `map_chunk_line_to_absolute` doc.
- Changes in the parser docs
  - `lua_block_location` doc (`parser/src/fence.rs` lines 267-274) and `block_label` doc (`parser/src/build.rs` near line 275) currently justify unique labels by mapping. Rewrite them so labels are display names and the chunk name carries mapping identity.
- Must not change
  - Display locations: ``section `T` prologue``, ``section `T` lua N``, ``section `T` (line L) prologue``, ``H1 `T` lua``, including the ordinals and ` (line L)` that `60aa72619` added.
  - The mapped output form `{location}:{line}:`, so existing `location:line:` assertions keep their text.
  - `require` behavior, events, reach, and caching. No public, persisted, or wire change; `LuaProgram` is absent from `crates/runvil/public-api.txt`.
  - Errors raised in a function that one block defined and another block called stay unmapped (non-goal), and must never be mapped wrongly. Such an unmapped error now shows `[string "line 120"]:3:` instead of the display name; this is the accepted tradeoff.
  - Do not use the rejected alternatives: numbering only H1 blocks, shortening or hashing long titles, accepting a cut marker by unique 45-byte prefix, or mapping through every block a VM has run.
- Regression tests. Write these first, confirm each fails against the unchanged code at `67be02738` for the stated reason, then make them pass.
  1. D1-1, in `parser/src/tests/line_mapping.rs` or `parser/src/tests/h1_fences.rs`. Parse an H1 with two Lua blocks separated by prose, built like `h1_fences.rs::h1_plain_lua_and_prose_are_live_blocks` (`prompt.h1_blocks`), with `assert(false)` in the first block. Load and call the first block's program to get the raw error. Assert `second.map_runtime_error(&raw_error).to_string() == raw_error.to_string()`. It fails today because both programs are named ``H1 `T` lua``, so the second rewrites the first block's marker through its own start line. Positive check: `assert_runtime_error_line(first, <absolute line>)`, which passes before and after.
  2. D1-2, in `lua/src/tests/diagnostics.rs`. `LuaProgram::compile` with a display location of 60 or more bytes, `source_line` 40, and source `assert(false)`. Call it and map the error with `map_runtime_error`. Assert the message contains `{location}:40:`. It fails today because Lua cuts the chunk name to 45 bytes plus `...`, so the marker never matches.
  3. D1-2, older unique-title range, in `parser/src/tests/line_mapping.rs`. A section `## Gather Background Research Notes` (30-byte title, 49-byte prologue name) whose prologue is `assert(false)`. Assert `assert_runtime_error_line(program, <absolute line>)`. It fails today because the cut marker leaves only the chunk-relative `:1:`.
  4. D1-2, range the target worsened, in `parser/src/tests/line_mapping.rs`. Sections `### Research Worker Notes` under `## A` and under `## B`; pad `## A` with prose lines so the second heading sits at line 100 or later. Assert `assert_runtime_error_line(under_b, <absolute line>)`. It fails today because ``section `Research Worker Notes` (line 1NN) prologue`` is 51 bytes, so Lua cuts it. Extra check: `under_a.map_runtime_error(&raw_error)` leaves the error unmapped, before and after.
  5. D1-2, engine level, in `engine/src/execute/tests/suite/require/failures.rs`. A section requires `## Shared Research Helper Functions` (32-byte title, 51-byte prologue name), and the library's only block raises while loading. Assert the run's error message contains ``section `Shared Research Helper Functions` prologue:<absolute line>:``. It fails today because the cut chunk name leaves the error unmapped.
- Existing tests
  - Keep passing without edits: `parser/src/tests/line_mapping.rs::every_block_of_a_section_maps_only_its_own_errors` and `::sections_sharing_a_title_get_locations_named_by_heading_line` (display locations are unchanged), and the `require` error suites under `lua/src/tests/require/` and `engine/src/execute/tests/suite/require/`.
  - Update: the tests in `lua/src/tests/diagnostics.rs` lines 134-224 that build raw `[string "..."]` messages and call `map_chunk_line_to_absolute` directly. Lines 134-178 pin display-location markers such as `[string "section ..."]` and move to the `[string "line N"]` form; lines 194-224 use owner names `x`, `lib`, and `caller` and move to the new owner form.
  - Tests that load several programs with the same `source_line` and expect each to map separately get distinct source lines.
- Constraints: no Rust file may exceed 500 lines (`lua/src/tests/diagnostics.rs` 365, `lua/src/program.rs` 291, `parser/src/tests/line_mapping.rs` 230, `engine/src/execute/tests/suite/require/failures.rs` 230, `parser/src/tests/h1_fences.rs` 198 at `67be02738`). Use `#[expect(..., reason = "...")]`, never `#[allow]`. No compiled code blocks in doc comments.
- Acceptance: all five regression tests pass along with the three crates' full suites, clippy, and fmt.

</step-1>

<step-2>

### Step 2: Gate `map_runtime_error` behind test-support and pass the exit checks [completed]

- Component: `none`
- Tests:
  - `$env:CARGO_BUILD_WARNINGS='deny'; cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `$env:RUSTDOCFLAGS='-D warnings'; cargo doc --locked --no-deps --all-features -p runvil-engine --document-private-items`
  - `cargo test -p build-xtask`
  - `rg -n "map_runtime_error\(" crates --glob '!**/tests/**'` must print exactly one line, the gated definition in `crates/runvil-internal/lua/src/program.rs`.
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo fmt --all --check`

Fixes D1-3 and runs the whole plan's exit checks.

- Debt being removed
  - `LuaProgram::map_runtime_error` (`lua/src/program.rs`, line 174 at `67be02738`) lost its only production caller when `60aa72619` switched `SectionVm::map_chunk_failure` to `map_runtime_error_among`. Its remaining callers are tests: `engine/src/lua/tests/errors.rs:427`, `lua/src/tests/diagnostics.rs:257`, `parser/src/tests/line_mapping.rs` lines 12, 164, and 205, plus the Step 1 regression tests in `lua/src/tests/` and `parser/src/tests/`. Doc comments in `lua/src/vm/run.rs` still name it as the mapping path.
- Changes
  - Gate `map_runtime_error` with `#[cfg(any(test, feature = "test-support"))]`, the same way the target gated `run_loaded_with_control` in `lua/src/vm/run.rs`. Do not delete it, and do not add it to `RETIRED_SEEDS` in `crates/build-xtask/src/engine_guards.rs`, because a gated symbol is not removed.
  - In `parser/Cargo.toml` `[dev-dependencies]` (line 18, today only `mlua.workspace = true`), add `runvil-lua = { workspace = true, features = ["test-support"] }`, matching `engine/Cargo.toml` line 42. Never enable `test-support` from a non-dev table; `build-xtask`'s `test_support_leak` check fails on that.
  - Rewrite the doc comments that name `map_runtime_error` as the mapping path: `lua/src/vm/run.rs` lines 104-106 on `start_block_coro`, 237-239 on `block_failure`, and 258-260 on `map_chunk_failure` (line numbers at `67be02738`), plus the link from `map_runtime_error_among`'s doc in `lua/src/program.rs`. Name `map_runtime_error_among` as plain code (`` `map_runtime_error_among` ``), not an intra-doc link, because it is crate-private and the gated method is absent from non-test docs. Afterward no doc comment presents `map_runtime_error` as the production mapping path.
- Must not change: mapping behavior from Step 1, any public, persisted, or wire surface, and `crates/runvil/public-api.txt`. No structural ratchets are added.
- Acceptance: every Tests command passes, the search shows only the gated definition, and the full workspace suite, workspace clippy and docs with warnings denied, `cargo test -p build-xtask`, and fmt all pass as the plan's exit checks.

</step-2>

</execution-plan>
