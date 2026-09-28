---
name: Capability harness debts
overview: "Debt collector result for the capability harness redesign run (15 commits, 35 candidates), plus the small fixes the operator asked for: one shared read-only proxy builder in the Lua crate, a refusal notice that stops naming a registered capability as missing, three doc corrections, and a CI gate for the engine's private-item docs. D1-1 is closed by owner decision with no change."
todos:
  - id: l1-proxy
    content: Move the three read-only proxy builders in promptforge-lua (prelude.rs, argv.rs, sys.rs) onto one crate-private builder in proxy.rs, pinning every refusal text and getmetatable label first (L-1)
    status: pending
  - id: d1-18-merge
    content: Make Requirements::merge drop missing_required entries that missing_services already names, with a failing-first runner regression test and a requirements unit test (D1-18)
    status: pending
  - id: doc-fixes
    content: Correct the sessions AGENTS.md broker sentence, the prelude environment doc, and the guide's Letting the model ask sentence, then regenerate the combined guide (D1-26, D1-27, D1-32)
    status: pending
  - id: d1-2-engine-docs-gate
    content: Add a CI step to the docs job that builds promptforge-engine's private-item docs with warnings denied (D1-2)
    status: pending
isProject: false
---

# Capability harness debt removal

<product-contract>

## Product Requirements

- Scope and target work: the capability harness redesign run, plan `vibe/2026-09-28-2-capability-harness-redesign.md`, commits `255ffa14` through `9fd8e981` on `master`, baseline `a7e50ec5`. The debt collector analyzed all 15 target commits and challenged its own findings; this plan carries the result, plus the small fixes the operator asked for: the one item the run's ledger deferred, and four small corrections taken from the collector's rejected candidates. The repository is `c:\Users\Vinnie\cursor\promptforge`, and every path below is relative to it.
- Cleanup goals: one read-only proxy builder in the Lua crate instead of three copies; a refusal notice that no longer names a registered capability as missing; three doc corrections the run made necessary; a CI docs step that fails when a private-item doc link in `promptforge-engine` breaks.
- Non-goals: any run-log compatibility work (owner decision under D1-1); private-item docs gates for crates other than `promptforge-engine`; any behavior change beyond D1-18's notice.
- Success criteria:
  - Every refusal message and `__metatable` string that the `argv`, `sys`, and prelude seals produce is byte-identical before and after L-1.
  - The D1-18 regression test fails before its change and passes after it.
  - The three corrected docs say what the code does.
  - CI's `docs` job builds `promptforge-engine`'s private-item docs with warnings denied, and that build passes.
  - Every repository gate passes: the full suite and doctests, both clippy runs, `cargo fmt --all --check`, the docs builds, `cargo +nightly-2026-09-05 xtask api --check` with no `public-api.txt` change, and the combined guide with no diff after regeneration.
- Writing: plain English, single dashes only, never em dashes or double dashes. Error messages stay as they are unless an item says otherwise.

## Functional Specification

### Debt Inventory

- Debt added:
  - D1-1 (introduced, `f1baf7a1` and `bb91d353`): the run log's persisted `EffectRecord`, `AnswerRecord`, and `Event` shapes changed with no format marker. Tool-call records gained a required `origin`, and the user-input variants were removed, so rows written by earlier builds would not deserialize through the current types. Owner decision on 2026-09-28: there are no old logs, so the plan adds no backward compatibility, no format marker, no serde default, and no doc caveat. No work item; the target state is the code as it is.
- Cheap fixes under the collector's rules: none. The challenger accepted no cheap fix.
- Operator-requested small fixes. The operator asked for every small deferred item to be fixed where that can be done reliably:
  - L-1 (the run ledger's one deferral, from Step 6): the read-only proxy pattern (an empty table whose metatable has an `__index` passthrough, a raising `__newindex`, and a `__metatable` label) is built in three places: `read_only_proxy` in `crates/promptforge-internal/lua/src/prelude.rs` (added by `fd734831`), `freeze_table` in `crates/promptforge-internal/lua/src/argv.rs`, and the proxy inside `seal_sys` in `crates/promptforge-internal/lua/src/sys.rs`.
  - D1-18 (rejected as residual-but-acceptable, taken at the operator's request): take a host with no input broker and a prompt that requires `promptforge/user-input` and binds its ask tool under a frontmatter alias, as the guide's "Letting the model ask" example does. The refusal carries an extra line naming `promptforge/user-input` as a missing required capability, although it is registered. Activation records a `MissingService` and skips `create` (`crates/harness-internal/capabilities/src/activation.rs`). Then `fill_tool_bindings` (`crates/promptforge-internal/engine/src/execute/fill.rs`) adds the slot's capability to `missing_required`, because it contributed nothing to the catalog. And `Requirements::merge` (`crates/promptforge-internal/engine/src/execute/requirements.rs`, called once, at `crates/harness-internal/runner/src/prepare.rs` line 288) dedupes within each list but not across them.
  - D1-26 (residual-but-acceptable, taken): `crates/harness-internal/sessions/AGENTS.md` line 3 says the crate owns "the user-input wait registry with the input performer over it". `InputPerformer` was deleted by `bb91d353`, and agents read this file as guidance.
  - D1-27 (weak or speculative, wording taken): the `environment` doc in `crates/promptforge-internal/lua/src/prelude.rs` (line 119) ends "so no prelude can change what another one sees". Each prelude does get its own environment and lookup table. But the `string`, `table`, `math`, `tools`, and `store` tables in that environment are the VM's shared tables, which any prelude or author code can change.
  - D1-32 (weak or speculative, taken): in "Letting the model ask", `guide/src/language/05-lua-environment.md` near line 782 says "On a host with no input broker it reads the fixed sentence instead". But the example just above declares the capability as a plain, required entry, and a host with no broker refuses that prompt before the run starts.
- Exposed pre-existing debt:
  - D1-2 (exposed, taken at the operator's request): no CI docs build covers `promptforge-engine`'s private-item rustdoc. That gap caused manual repairs in two different plans: `eca41e57` for `workshop-server`, which added a gate for that crate, and `6c818272` for the engine, which added none. At `9fd8e981` the `docs` job in `.github/workflows/ci.yml` runs the public workspace docs build and the two facade builds; the only private-items build is the `workshop-server` step in the `check-workshop` job. The engine's private-items build passes at `6c818272` and later, after Step 14 of the redesign run repaired its links.
- Rejected candidates, 33 of 35:
  - 15 false: the claims do not hold at `9fd8e981`. No reader of earlier builds' logs exists; the full-id fallback reaches only declared capabilities' tools and never model calls; the setup order and the seals hold; relocated tests cover the removed ones; the guide's counts are right; and the session id carried in the origin is already stored in `runs.session_id`.
  - 10 residual-but-acceptable: each is the plan's specified design or an owner deferral, such as `ServiceGap` reserved for the menu and `ToolPerformer::call` receiving no origin.
  - 7 weak or speculative: unreachable in production, or no demonstrated consequence.
  - 1 unrelated pre-existing.
  - D1-18, D1-26, D1-27 (wording only), and D1-32 are taken above at the operator's request.

</product-contract>
<implementation-contract>

## Technical Design

- L-1: one crate-private builder in a new module, `crates/promptforge-internal/lua/src/proxy.rs`, declared in `lua/src/lib.rs`.
  - Shape: `read_only_proxy(lua, index: Value, refusal, label: &str) -> mlua::Result<Table>`, where `refusal` is a closure that builds the whole error text from the refused key. It returns an empty table whose metatable holds `__index` set to `index` (a table or a function), a `__newindex` that raises `refusal(key)`, and `__metatable` set to `label`. The closure's bounds are whatever `Lua::create_function` requires in this build.
  - `prelude.rs`: `seal` and `read_only_var` call the shared builder with the text they produce today, `"{refusal} '{field}'"` via `field_name`, and the module's own copy is removed.
  - `argv.rs`: `freeze_table` calls it with `"argv is frozen outside H1: cannot set field {field}"`, where `field` is `'name'` for a string key and the key's `Debug` form otherwise, and the label `argv is frozen`.
  - `sys.rs`: `seal_sys` calls it with its existing `__index` function, the text `"sys is read-only; cannot set '{field}'"`, where `field` is the string key or the key's `Debug` form, and the label `sys is sealed`.
  - Not part of this item: the `var` guard in `sys.rs`, whose `__newindex` validates and stores values and so is not read-only.
  - Every refusal text and label stays byte-identical, including how a non-string key renders.
- D1-18: `Requirements::merge` drops, after folding `other` in, every `missing_required` entry whose capability also appears in `missing_services`. Such an entry can come only from `fill_tool_bindings` seeing an exact tool slot whose capability contributed nothing. Activation records a missing service only for a registered capability whose `create` it skipped, so the service line already names the real cause. The `merge` doc comment says so. No type or signature changes, so `crates/promptforge/public-api.txt` is unaffected.
- D1-2: a new step in the `docs` job of `.github/workflows/ci.yml`, after "Harness facade docs", named "Engine docs (private items)", running `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`. The job already sets `RUSTDOCFLAGS: -D warnings` for every step, so the step needs no `env`. `--all-features` is needed because the engine's `test_support` module sits behind a feature and its doc links are part of what broke. The step goes in the `docs` job rather than beside the `workshop-server` step in `check-workshop`, because the engine is not a workshop crate and the Workshop steps build without `--all-features`. A rustdoc build is a compiler check, not the kind of structural enforcement `AGENTS.md` reserves for owner approval.
- D1-26, D1-27, D1-32: documentation only.
  - D1-26: the `AGENTS.md` sentence names the user-input wait registry and the session broker, `SessionInputBroker`, which implements `InputBroker` for the `promptforge/user-input` capability.
  - D1-27: the `environment` doc says each prelude gets its own environment and lookup table, so no prelude sees another prelude's globals, and that the library tables, `tools`, and `store` in it are shared with the rest of the VM.
  - D1-32: the guide sentence says the model reads the fixed sentence when the capability is declared optional on a host with no broker, and that a host with no broker refuses a required declaration before the run starts. Then regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`.

</implementation-contract>
<verification-contract>

## Testing Plan

- L-1: pin the behavior before refactoring.
  - Add Lua-crate tests, beside the existing `argv` and `sys` tests in `crates/promptforge-internal/lua/src/tests.rs` or in `prelude-tests.rs`. They assert the full refusal text for a string key and for a non-string key, and the `getmetatable` result, for a frozen `argv` table outside H1, for `sys`, and for a sealed prelude global. They pass before and after the refactor.
  - Existing pins to keep passing: `tests.rs` near lines 2075 and 2083 (`sys is read-only; cannot set 'when'` and `'extra'`), `tests.rs` near lines 3385-3434 (`argv is frozen`), and the seal tests in `prelude-tests.rs`.
  - Then run the `promptforge-lua` component tests.
- D1-18:
  - Regression test in `crates/harness-internal/runner/tests/it/prepare-input.rs`. A prompt declares `promptforge/user-input` required and binds `tools: { ask: promptforge/user-input/ask }`, and is prepared on a host with `input: None`. It is refused, and the notice holds "- promptforge/user-input needs an input broker, and this host provides none" and no line naming `promptforge/user-input` as a missing required capability. The test must fail before the change.
  - Unit test in `crates/promptforge-internal/engine/src/execute/requirements-tests.rs`. Merge a report whose `missing_required` holds a capability into one whose `missing_services` names it, and the reverse. Either way, that capability leaves `missing_required`, and an unrelated missing capability stays.
  - Then run the `harness-runner` and `promptforge-engine` suites.
- D1-32: regenerate the combined guide, confirm a second regeneration shows no further diff, and build the books with `cargo xtask site --books-only`.
- D1-26 and D1-27: build `promptforge-lua`'s docs with `RUSTDOCFLAGS=-D warnings`.
- D1-2: run the new step's command locally with `RUSTDOCFLAGS=-D warnings` and confirm it exits 0; confirm by reading `ci.yml` that the step sits in the `docs` job, whose `env` sets `RUSTDOCFLAGS: -D warnings`. Optional fault injection, reverted before commit: break one private intra-doc link in the engine and confirm the command fails.
- Exit: the full repository gates as the Project Survey records them, including both clippy runs, `cargo fmt --all --check`, the docs builds, `cargo +nightly-2026-09-05 xtask api --check` with no `public-api.txt` change, and no guide diff after regeneration.

</verification-contract>
<decision-record>

## Decision Record

- D1-1, owner decision on 2026-09-28: "There are no old logs. do not add any backward compatibility or comments or anything." So: no format marker on `runs`, no `#[serde(default)]` on `origin`, no legacy variants, and no doc caveat about reading stored logs across builds. The options put to the owner were docs only, a `record_format` column on `runs`, and a defaulted `origin`.
- D1-2, operator decision on 2026-09-28: widen the scope to this exposed debt and gate the engine's private-item docs. Rejected: listing the command in the `AGENTS.md` Verification section instead, which relies on each run remembering it, the way Step 14 found the rot only by hand; `AGENTS.md` does not list the `workshop-server` private-items build either, so the gate lives in CI alone. Rejected for now: gating `promptforge-lua` and the `harness-*` crates too, because nobody has shown their private docs are broken, and a gate on a crate that already fails would block CI.
- Small fixes, operator decision: fix every small deferred item where that can be done reliably. The run ledger deferred one item (L-1). The collector's rejected candidates supplied four small corrections the operator accepted: D1-18, D1-26, D1-27 (wording only), and D1-32. The run's other deferral, `ToolPerformer::call` receiving no tool-call origin (the `Deferred:` trailer on `f1baf7a1`), is an owner decision with its own revisit condition, and is not taken.
- L-1 passes each caller's refusal text as a closure rather than unifying the three messages. The messages are observable and partly pinned by tests, so unifying them would change behavior for no gain. Rejected: moving `argv` and `sys` onto the prelude message format.
- D1-18 fixes the report in `Requirements::merge`, where the two lists first meet. Rejected: fixing it in `fill_tool_bindings`, which runs in the engine before activation's report exists; and suppressing the line in `notice` only, which would leave `missing_required` wrong for any other reader.
- Risk: L-1 touches the seals that protect `sys`, `argv`, and prelude globals. The pinned texts, the `getmetatable` results, and the existing seal tests hold that contract.

### Deferred and Out of Scope

- Private-item docs gates for `promptforge-lua` and the `harness-*` crates. Revisit when one of them needs a manual repair of its private rustdoc.
- `ToolPerformer::call` receiving the tool-call origin. Revisit with the first host policy that decides by caller.
- The remaining rejected candidates, including D1-17 (a web-only build failure refused with the generic missing-capability notice), D1-30, D1-31, and D1-33.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p <package>`. Plain `cargo build` builds only the default member, `crates/gateway/app` (package `gateway`). Crates whose build scripts bundle a UI into `OUT_DIR` (`workshop-server`, `gateway-config-ui`, and their dependents) need `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first. Before any `-p workshop` build, CI builds `cargo build --locked -p gateway --no-default-features` and stages it with `node tools/stage-gateway-sidecar.mjs stage --target x86_64-pc-windows-msvc --source target/debug/promptforge-gateway.exe` (undo with `node tools/stage-gateway-sidecar.mjs remove --target x86_64-pc-windows-msvc`). Clippy is a superset of `cargo check`, so never run a standalone `cargo check --workspace` beside it. `.cargo/config.toml` aliases `cargo workshop` to `run -p build-workshop --` and `cargo xtask` to `run -p build-xtask --`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-substring>`, adding `--test it` to target only the crate's integration target (`--test suite` for the `promptforge` and `harness` facades). Drop `--all-features` for `workshop`, `workshop-server`, and `workshop-server-api`. Nextest skips doctests, so a doc example needs `cargo test --locked --doc -p <package>`. One UI test file: `node --test test/<name>.mjs` from `crates/workshop/ui` (or `look`, `platform`); one config UI test: `node --test src/<path>.test.mjs` from `crates/gateway/config-ui/ui`.
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test --locked --doc -p <package> --all-features`, with the same no-`--all-features` exception for the workshop trio; `workshop-server` also has `cargo nextest run --locked -p workshop-server --features headless`. One UI package: `npm test --workspace <ui|look|platform>` from `crates/workshop`; the gateway config UI: `npm test` from `crates/gateway/config-ui/ui`. The structural harness alone: `cargo test -p build-xtask`; its nightly-only fixtures: `cargo +nightly-2026-09-05 nextest run --locked -p build-xtask --run-ignored only`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` and `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`. CI adds `cargo nextest run --locked -p workshop-workspace --all-features`, `cargo nextest run --locked -p workshop-server --features headless`, the gateway process-ownership race tests (`cargo test --locked -p gateway --no-default-features --features test-fixtures --test it <name>`), and `npm test --workspaces --if-present` from `crates/workshop` plus `npm test` from `crates/gateway/config-ui/ui`. The workspace run includes `build-xtask`, the structural harness (tier graph, `## Invariants` markers, lint inheritance, 500-line ceiling, product-boundary matrix).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the headless gate `cargo check -p gateway --no-default-features`, `cargo deny check`, and `cargo audit`. UI typecheck: `npm run typecheck --workspaces --if-present` from `crates/workshop`, and `npm run typecheck` from `crates/gateway/config-ui/ui`. The `.githooks/pre-push` hook runs the headless check, the workspace clippy, and `cargo deny check` when installed.
- Formatter check command: `cargo fmt --all --check` (`rustfmt.toml` sets `style_edition = "2024"`; `.githooks/pre-commit` runs it). No UI formatter or JS linter is configured.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`). The facades also build alone with default features: `cargo doc -p promptforge --no-deps` and `cargo doc -p harness --no-deps`. CI adds `cargo doc --locked --no-deps -p workshop-server --document-private-items`; the user guide is `cargo xtask site --books-only`. The `promptforge` surface check is `cargo +nightly-2026-09-05 xtask api --check` (the nightly pinned in `crates/build-xtask/src/api/toolchain.rs`), compared against the committed `crates/promptforge/public-api.txt`; the `harness` facade has no committed listing.
- Test placement and naming conventions:
  - Unit tests sit in a sibling file `<module>-tests.rs`, wired as `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (about 140 such files, for example `capabilities/src/registry-tests.rs`, `engine/src/execute/run-tests.rs`, `types/src/event-tests.rs`, `sessions/src/input-tests.rs`). Larger suites become a `tests/` module directory beside the module, such as `engine/src/execute/tests/` (with `scheduler/` and `suite/`), `models/src/transport/tests/`, and `workspace/src/workspace/tests/`; the lua crate keeps one `src/tests.rs`. The `build-*` tooling crates use inline `mod tests {}` blocks.
  - Integration tests are one `tests/it/` target per crate (`tests/suite/` in the `promptforge` and `harness` facades). Its `main.rs` declares the modules and opens with `#![expect(clippy::expect_used, clippy::unwrap_used, reason = ...)]`; files split into kebab siblings such as `session-infer.rs` and `session-close.rs`, or into subdirectories such as `workshop/server/tests/it/agents/`, with shared helpers in `support.rs`. A few crates also keep standalone files under `tests/` (`gateway/stt/engine`, `gateway/cloud-providers`, `gateway/stt/backend-whisper`, `build-workshop`).
  - Shared fixtures live in `test_support` modules (the engine's `src/test_support/` with `host.rs`, `tools.rs`, and recording and tokio driver helpers; `test_support.rs` in the runner, parser, gateway app, and workshop workspace), exported to siblings behind a `test-support` feature (engine, lua, parser, runner, sessions) or a `test-fixtures` feature (gateway, workshop, and `gateway-api-discovery` crates).
  - Test functions are snake_case sentences stating the behavior, such as `a_direct_launch_recovers_the_lease_from_a_terminated_owner`.
  - `clippy.toml` allows `unwrap` and `expect` in tests; the workspace lints deny both elsewhere.
  - Criterion benches: `engine/benches/models_loop.rs` and `lua/benches/surface.rs`.
  - UI tests run under `node --test`: `crates/workshop/{ui,look,platform}/test/*.mjs` (the ui package holds 96, with `test/helpers/`) and `crates/gateway/config-ui/ui/src/**/*.test.mjs` (25). Node scripts in `tools/` keep a sibling `<name>.test.mjs`.
- Directory map:
  - `crates/`: every Rust crate and UI package. Its root is the public layer: the `promptforge` and `harness` facades, `gateway-api-types`, `gateway-api-discovery`, `shared-error-source`, `shared-loopback`, `shared-ui` (TypeScript and CSS, not a crate), the `build-*` tooling crates (`build-llama-cuda`, `build-ui`, `build-user-guide`, `build-workshop`, `build-xtask`), and `workspace-hack` (cargo-hakari). `crates/README.md` describes each root crate.
  - `crates/promptforge-internal/`: the engine's private crates `engine`, `types`, `lua`, `parser`, `vfs`, and `model-client`.
  - `crates/harness-internal/`: the harness's private crates `runner`, `models`, `capabilities`, `log`, `sessions`, `web`, `webfetch`, and `web-search`.
  - `crates/workshop/`: `desktop` (the Tauri app, package `workshop`), `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `status`, `support`, `user-state`, and `workspace`, plus the npm workspaces `ui`, `look`, and `platform`.
  - `crates/gateway/`: `app` (package `gateway`), `cloud-providers`, `config`, `config-ui` (with its own `ui/`), `local`, `logging`, `progress`, `protocol`, `routing`, `web-search`, and the nested `stt/` subsystem (`api`, `engine`, `backend-whisper`, `whisper-ffi`).
  - `guide/`: user guide chapter sources (`src/`), mdBook books (`books/`), `landing/`, `chrome/`, the three product guides, and `CONTRIBUTING.md`.
  - `prompts/`: sample prompt programs. `tools/`: Node scripts (gateway sidecar staging, a live TTS probe) and Python maintenance scripts under `tools/scripts/`.
  - `vibe/`: `archdoc.md`, dated plan and run records (`YYYY-MM-DD-N-slug.md`, older ones in `2026-07/`, `2026-08/`, `2026-09/` month folders), reference notes, and a gitignored `scratch/`.
  - `.github/workflows/`: `ci.yml` (its `ci-green` job is the single required check) and release, nightly, site, and native-library workflows; `.github/fixtures/` holds the installer smoke fixtures. `.githooks/`: pre-commit and pre-push. `.config/`: `nextest.toml` and `hakari.toml`. `.cargo/config.toml`: rust-lld with the static CRT on Windows, and the `cargo workshop` and `cargo xtask` aliases.
  - Root files: `Cargo.toml` (explicit container member list, `default-members` is the gateway app), `rust-toolchain.toml` (stable), `clippy.toml`, `rustfmt.toml`, `deny.toml`, `dist-workspace.toml`, and `gateway.local.example.toml`. `local/`, `target/`, and `target-msrv/` are gitignored; `images/` holds README art.
- Component boundaries:
  - Executor: `promptforge` is a facade of single-item re-exports over `promptforge-internal/*`, and the only promptforge crate that code outside the family may name. `promptforge-*` crates depend on no gateway, workshop, or harness crate.
  - Harness: `harness` is the facade over `harness-internal/*` and the only door into that container. `harness-*` crates depend only on `promptforge` and their siblings, never on gateway, shared, workshop, or private `promptforge-*` crates.
  - Workshop: its crates may name `harness`, `promptforge`, the gateway public pair, and `shared-*`, never the gateway family's private crates. The desktop app depends on `workshop-server-api`, never `workshop-server`. Inside the family, dependencies flow one way: server, then features, then services, then vocabulary. In the SPA, lazy panels never import a module inside the entry bundle.
  - Gateway: its private crates depend on the root pair and `shared-*`, never on promptforge, harness, or workshop crates; `gateway-stt` is the only family-visible crate of the nested `stt/` subsystem.
  - `shared-*` crates depend on no product crate. A crate in a family container depends only on root crates and its own siblings; `build-*` crates are exempt.
  - Every `workshop-*` and `harness-*` crate's `lib.rs` opens with a `//!` doc holding an `## Invariants` marker listing what it may and may not depend on. `cargo test -p build-xtask` enforces these rules, and `cargo xtask api --check` enforces the `promptforge` surface.
- Conventions summary:
  - Rust 2024 edition on the stable toolchain. Workspace lints that every member inherits through `[lints] workspace = true`: `unsafe_code = "forbid"`, `missing_docs = "warn"`, clippy `all` and `pedantic` denied, `unwrap_used` and `expect_used` denied, broken and private intra-doc links denied. Unsafe code stays in its owning boundary with a safety comment before each block.
  - Source directories are flat: one or two files beside a parent module are `foo-bar.rs` wired with `#[path = "foo-bar.rs"] mod bar;`; at three files they become a `foo/` subdirectory, and they flatten back below three. Files in marker crates stay at or under 500 lines; split before an edit crosses that.
  - Behavior changes ship with tests in the same change. No new structural enforcement (parsers, allowlists, counts, topology checks) without explicit user approval.
  - Error and status messages are written for a model reader: concise, self-contained, naming required versus actual.
  - Comments explain only non-obvious constraints; every workaround cites its upstream issue URL.
  - JSON that reaches the run log round-trips exactly: sorted keys, finite numbers, `float_roundtrip`, never `preserve_order`.
  - Cargo features gate real constraints (toolchains, heavy native builds), not product shape. Library and serve paths return failures instead of exiting or installing process-global state.
  - CI passes `--locked` on builds and tests and fails when a build dirties the tree.
  - UI: CSS sits beside its TypeScript, component CSS uses `--ws-*` tokens only, and the SPA never touches `localStorage`; persisted UI state goes through the server to `ui-state.json` or the `.pfwork` file.

</project-survey>
<execution-plan>

## Execution Instructions

- Status: decomposed on 2026-09-28 against local `master` at `9fd8e981` ("Close plan: capability-harness-redesign") with a clean worktree. Every file, function, test, and CI step named below was found at its stated place at that commit.
- Scope: four components, one step each, in dependency order. Each step's Todo line names the frontmatter todo it builds. D1-1 has no step, by owner decision.
- Component order:
  1. **Lua seal proxy** (L-1) - first. It is the only change to the seals that guard `sys`, `argv`, and prelude globals, so it lands while the rest of the tree is unchanged, and Step 3's D1-27 edit to `prelude.rs` must follow it.
  2. **Refusal notice dedupe** (D1-18) - second. It shares no file with component 1 and could run beside it. It lands before the doc corrections so D1-32's guide sentence about refusal is written against the final refusal behavior, and before the engine docs gate so that gate's local run covers the new `merge` doc comment.
  3. **Doc corrections** (D1-26, D1-27, D1-32) - third, after Step 1, because D1-27 edits `prelude.rs` in its final shape.
  4. **Engine private docs gate** (D1-2) - last. It depends on no other step; landing it last means its local run checks the engine's private docs as the plan leaves them.
- Standing rules for every step:
  - Each step is one commit holding its code, docs, and tests.
  - Line numbers refer to `9fd8e981`. Step 1 moves lines in `prelude.rs`, `argv.rs`, and `sys.rs`; re-locate any reference by content before editing.
  - Every refusal text, `__metatable` label, and error message stays byte-identical. The plan's one observable change is D1-18's dropped duplicate line in the refusal notice.
  - No type or signature change reaches `crates/promptforge/public-api.txt`.
  - Per-step hygiene beyond each step's Verify line: the workspace clippy run and `cargo fmt --all --check` from the Project Survey.
  - Not done here: the `var` guard in `sys.rs` (its `__newindex` validates and stores), private-item docs gates for any crate but `promptforge-engine`, and everything under Deferred and Out of Scope.
  - Writing: plain English, single dashes only, never em dashes or double dashes.

<step-1>

### Step 1: Build every read-only seal through one proxy builder [completed]

- Component: Lua seal proxy
- Piece: pins and shared builder, built jointly in one commit. The pins are the test set that covers the refactor, and they must pass on the unchanged code before any caller moves. The builder cannot land alone, because a crate-private function with no caller fails `dead_code` under the denied-warnings clippy run.
- Todo: `l1-proxy`
- Depends on: nothing
- Read: Technical Design, the L-1 bullets; Testing Plan, the L-1 bullets; Decision Record, the L-1 closure and Risk bullets; Project Survey.
- Build, in this order:
  - Pins first (see Tests). Run them against the unchanged code and confirm they pass before touching any caller. For each integer key, capture the text the unchanged code raises and write it into the test as a literal.
  - New `crates/promptforge-internal/lua/src/proxy.rs` with a `//!` header, declared as `mod proxy;` in `lua/src/lib.rs` among the other private modules. It holds `pub(crate) fn read_only_proxy(lua: &Lua, index: Value, refusal, label: &str) -> mlua::Result<Table>`, where `refusal` is a closure from the refused key (`&Value`) to the whole error text, with whatever bounds `Lua::create_function` requires in this build. It returns an empty table whose metatable holds `__index` set to `index`, a `__newindex` that raises `mlua::Error::runtime(refusal(&key))`, and `__metatable` set to `label`. Its doc comment comes from the prelude copy (`prelude.rs` lines 230-233), reworded for a caller-built refusal.
  - `prelude.rs`: delete the local `read_only_proxy` (lines 234-250). `seal` (line 220) and `read_only_var` (line 140) pass closures that produce today's `"{refusal} '{field}'"` through `field_name` (line 253), which stays in `prelude.rs` as its only user. The labels stay `"{name} is sealed"` and `var is read-only`.
  - `argv.rs`: `freeze_table` (line 180) becomes one builder call with `__index` set to the data table, the refusal `argv is frozen outside H1: cannot set field {field}`, and the label `argv is frozen`. `{field}` is `'name'` for a string key and the bare `Debug` form otherwise, so only string keys are quoted. It keeps mapping the `mlua` error through `Error::lua`.
  - `sys.rs`: `seal_sys` (line 143) keeps building its `__index` function (lines 154-168) and hands it to the builder with the refusal `sys is read-only; cannot set '{field}'` and the label `sys is sealed`. `{field}` is the string key or the key's `Debug` form, quoted either way. Its metatable used `set` where the builder uses `raw_set`; on a fresh table with no metatable of its own the two behave the same. The `var` guard below it (from line 193) is untouched.
- Tests:
  - `crates/promptforge-internal/lua/src/tests.rs`, beside the `sys` pins near lines 2075-2083 and the `argv` pins near lines 3385-3434: for `sys`, and for a frozen `argv` table outside H1, assert the complete refusal text for a string key and for an integer key, and that `getmetatable` returns `sys is sealed` and `argv is frozen`.
  - `crates/promptforge-internal/lua/src/prelude-tests.rs`, beside the seal test near lines 120-140 and the `var` view test near lines 389-403: assert the complete refusal text for a string key and an integer key on a sealed prelude global (``kit is read-only: capability `acme/kit` defines it; cannot set 'extra'``) and on a prelude's `var` view (`var is read-only inside a capability prelude; cannot set '<field>'`). The existing `getmetatable` pins (`kit is sealed` and the `var` label) stay as they are.
  - Existing pins that must pass unchanged: the `sys` and `argv` tests above and every seal test in `prelude-tests.rs`.
- Verify: the component test pattern for `promptforge-lua` (nextest, then doctests, both with `--all-features`); the workspace clippy run; `cargo fmt --all --check`.
- Commit: one commit with `proxy.rs`, the `lib.rs` declaration, the three migrated callers, and the new pins.
- Done when: the pins pass before and after the move, and apart from the `var` guard no read-only seal builds its own `__newindex` or `__metatable` outside `proxy.rs`.

</step-1>

<step-2>

### Step 2: Stop naming a registered capability as missing [completed]

- Component: Refusal notice dedupe
- Piece: `Requirements::merge`, one step.
- Todo: `d1-18-merge`
- Depends on: nothing
- Read: Product Requirements, the D1-18 bullet under Debt Inventory; Technical Design, the D1-18 bullet; Testing Plan, the D1-18 bullets; Decision Record, the D1-18 bullet; Project Survey.
- Build, in this order:
  - Tests first (see Tests). Confirm the runner regression test fails at `9fd8e981`, with the notice holding `- missing required capability: promptforge/user-input` beside the service line.
  - `crates/promptforge-internal/engine/src/execute/requirements.rs`: `Requirements::merge` (line 61), after folding `other` in, drops every `missing_required` entry whose capability also appears as a `MissingService::capability` in `missing_services`. Such an entry can come only from `fill_tool_bindings` (`execute/fill.rs`) seeing an exact tool slot whose capability contributed nothing because activation skipped its `create`, so the service line already names the real cause. The `merge` doc comment (lines 57-60) says so. No type or signature changes, and `notice` is untouched.
- Tests:
  - `crates/harness-internal/runner/tests/it/prepare-input.rs`, after `a_required_user_input_declaration_on_a_host_without_a_broker_is_refused` (line 175): a new test that prepares `user_input_prompt` with the declaration `REQUIRED` followed by `tools:\n  ask: promptforge/user-input/ask\n` on `user_input_services(&log, None)`. The helper appends the declaration verbatim after `capabilities:`, so the `tools:` block lands in the same frontmatter. It asserts a `PrepareError::Refused` of kind `RunErrorKind::RequirementsUnmet` whose text holds `- promptforge/user-input needs an input broker, and this host provides none` and does not hold `missing required capability: promptforge/user-input`. The file is 327 lines and `harness-runner` is a marker crate, so keep it at or under 500.
  - `crates/promptforge-internal/engine/src/execute/requirements-tests.rs`, beside `merge_folds_in_missing_services_without_repeating_one`: one test merges a report whose `missing_required` holds `promptforge/user-input` and an unrelated `acme/other` into a report whose `missing_services` names `promptforge/user-input` (built with `missing_input`). A second test does the reverse, which is the direction `crates/harness-internal/runner/src/prepare.rs` line 288 takes. Either way `missing_required` ends as `[acme/other]` and `missing_services` is unchanged.
- Verify: the component test pattern for `harness-runner` and `promptforge-engine`; the workspace clippy run; `cargo fmt --all --check`.
- Commit: one commit with the `merge` change, its doc comment, and both tests.
- Done when: the regression test fails before the change and passes after it, and both suites pass.

</step-2>

<step-3>

### Step 3: Correct the three docs the redesign left stale [completed]

- Component: Doc corrections
- Piece: three corrections, built jointly in one commit. None changes behavior, so no test covers any one of them alone, and one pass (the Lua docs build, the guide regeneration, and the book build) verifies all three.
- Todo: `doc-fixes`
- Depends on: Step 1, because D1-27 edits `prelude.rs` after Step 1 rewrote it.
- Read: Product Requirements, the D1-26, D1-27, and D1-32 bullets under Debt Inventory; Technical Design, the D1-26, D1-27, and D1-32 bullets; Testing Plan, the D1-32 and the D1-26 and D1-27 bullets; Project Survey.
- Build:
  - D1-26, `crates/harness-internal/sessions/AGENTS.md` line 3: replace "the user-input wait registry with the input performer over it" with the user-input wait registry and the session broker, `SessionInputBroker` (in `src/input-tool.rs`), which implements `InputBroker` for the `promptforge/user-input` capability. The rest of the line stays.
  - D1-27, the `environment` doc in `crates/promptforge-internal/lua/src/prelude.rs` (lines 118-119 at `9fd8e981`): each prelude gets its own environment and lookup table, so no prelude sees another prelude's globals, and the `string`, `table`, `math`, `tools`, and `store` tables it reads through that lookup are shared with the rest of the VM. Plain text only, since no gate builds `promptforge-lua`'s private docs.
  - D1-32, `guide/src/language/05-lua-environment.md` line 782, the last sentence of the paragraph after the `tools:` example in "Letting the model ask": the model reads the fixed sentence when the capability is declared with `optional: true` on a host with no input broker, and a host with no broker refuses the required declaration shown above before the run starts. Then regenerate `guide/promptforge-language-guide.md` with `cargo run --locked -q -p build-user-guide`.
- Tests: none new; these are documentation fixes.
- Verify: run the guide regeneration a second time and confirm no further diff; `cargo xtask site --books-only`; `cargo doc --locked --no-deps --all-features -p promptforge-lua` with `RUSTDOCFLAGS` set to `-D warnings` (PowerShell: `$env:RUSTDOCFLAGS='-D warnings'`); `cargo fmt --all --check`.
- Commit: one commit with the three source edits and the regenerated combined guide.
- Done when: each corrected passage says what the code does, and the guide regenerates with no diff.

</step-3>

<step-4>

### Step 4: Gate the engine's private-item docs in CI [completed]

- Component: Engine private docs gate
- Piece: one CI step, one step.
- Todo: `d1-2-engine-docs-gate`
- Depends on: nothing. Placed last so its local run checks the engine docs including Step 2's `merge` doc comment.
- Read: Product Requirements, the D1-2 bullet under Debt Inventory; Technical Design, the D1-2 bullet; Testing Plan, the D1-2 bullet and Exit; Decision Record, the D1-2 bullet; Project Survey, Docs command.
- Build: in `.github/workflows/ci.yml`, the `docs` job (line 115, whose `env` at line 118 sets `RUSTDOCFLAGS: -D warnings` for every step), add a step after "Harness facade docs" (lines 148-149) named "Engine docs (private items)" running `cargo doc --locked --no-deps --all-features -p promptforge-engine --document-private-items`, with no `env` of its own and no comment. The `workshop-server` private-items step in `check-workshop` (line 215) is untouched.
- Tests: none new; the check is the CI step itself.
- Verify: run the step's command locally with `$env:RUSTDOCFLAGS='-D warnings'` and confirm it exits 0; read `ci.yml` to confirm the step sits in the `docs` job under that job's `env`. Optional fault injection, reverted before commit: break one private intra-doc link in the engine and confirm the command fails.
- Commit: one commit with the CI step.
- Done when: the command exits 0 locally, the step is in the `docs` job, and the Exit gates below pass.

</step-4>

- Exit: after Step 4, the full repository gates from the Project Survey: the full-suite test command with its doctests; the Linter command (both clippy runs, the headless check, `cargo deny check`, `cargo audit`); `cargo fmt --all --check`; the docs builds (the workspace build, both facades, the `workshop-server` private-items build, and the new engine private-items build); `cargo +nightly-2026-09-05 xtask api --check` with no `public-api.txt` change; and the combined guide with no diff after regeneration.

</execution-plan>
