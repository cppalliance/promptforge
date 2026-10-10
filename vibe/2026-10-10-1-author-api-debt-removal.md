---
name: Author API debt removal
overview: Remove the one debt the author API reshape introduced (a false "not offered" suffix when a model names an offered tool by canonical id), close the exposed gap that lets Engine wording violations land on master because no local gate runs the docs-claims scan, and clean up two owner-chosen leftovers - internal names that still describe the removed tools.add API, and a Harness doc that still calls alias the prompt-local name.
todos:
  - id: d1-1-wire-lookup
    content: "D1-1: add ToolSet::wire_binding, use it in model-call dispatch and the scope gate's global_exists, fix the field doc, with a failing-first canonical-id test in tool_loop.rs"
    status: pending
  - id: d1-2-local-gate
    content: "D1-2: run node --test crates/workshop/ui/test/docs-claims.mjs from the root AGENTS.md Verification list and .githooks/pre-push, proven by fault injection"
    status: pending
  - id: d1-12-renames
    content: "D1-12: rename ToolsAddEntry, collect_tools_add_entries, add_local_params_schema, tool_alias, and ToolCallCounts::aliases (with its alias parameters) to names that match the offer API"
    status: pending
  - id: d1-17-harness-doc
    content: "D1-17: correct the ToolPerformer::call doc in performers.rs so alias is the wire name, or the canonical id when the run could not offer the tool"
    status: pending
isProject: false
---

# Author API debt removal

<product-contract>

## Product Requirements

- Scope and target work
  - Repository: promptforge, at `70f7fd577` on `master` (also `origin/master`).
  - Target: `2b53936e6..70f7fd577`, the seven commits of the author API reshape plan (`vibe/2026-10-09-5-author-api-reshape.md`): `da5b2f88e`, `20d879ab1`, `4dc463c64`, `0ea5e7711`, `4e2f7ce65`, `dc08e068d`, `70f7fd577`.
  - Removal scope: one introduced debt (D1-1), one exposed pre-existing debt (D1-2), and two rejected candidates (D1-12 and D1-17), all three added by owner decision.
- Cleanup goals and non-goals
  - Goal: the out-of-scope tool-call error claims "not offered in this section" only when the model named, by wire name, a tool the run offers and the section did not advertise.
  - Goal: the dispatch rule and the scope-gate rule for model-supplied tool names live in one lookup, so they cannot drift apart again.
  - Goal: an Engine change that breaks the Engine wording rule fails a local gate before push, not only CI's `ui` job.
  - Goal: no crate-private name or doc in the Lua crate or the Harness runner still describes the removed `tools.add` and `tools.add_local` API or calls a tool id an alias.
  - Non-goal: changing any author-facing or specified error text, any facade item, or any wire or persisted format.
  - Non-goal: porting the docs-claims scan to Rust, or adding new wording rules.
  - Non-goal: the rejected candidates listed under Deferred and Out of Scope.
- Success criteria
  - A new engine test, in which a model requests the canonical id of a tool its section advertised, fails at `70f7fd577` and passes after the change, and the two existing out-of-scope tests still pass unchanged.
  - The root `AGENTS.md` `## Verification` list and `.githooks/pre-push` both run `node --test crates/workshop/ui/test/docs-claims.mjs`, and a deliberately injected forbidden word in an Engine doc makes that command fail.
  - `git grep` over `crates/promptforge-internal/lua/src` finds none of `ToolsAddEntry`, `collect_tools_add_entries`, `add_local_params_schema`, `tool_alias`, or `fn aliases(`, and `ToolPerformer::call`'s doc no longer says "prompt-local name".
  - The facade surface check shows no diff, and every root `AGENTS.md` verification command passes.

## Functional Specification

### Debt Inventory

- Debt added
  - D1-1 (introduced, `4dc463c64`): the out-of-scope suffix calls an offered tool "not offered" when the model names it by canonical id.
    - Evidence: at `70f7fd577`, `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs` lines 269 to 275 build `global_exists` as `tool_set_snapshot()?.offered_binding(name).is_some()`. `ToolSet::offered_binding` (`crates/promptforge-internal/lua/src/handles.rs` lines 203 to 214) matches by canonical id whenever `name` contains `/`. `crates/promptforge-internal/engine/src/error.rs` line 235 appends ` (a catalog tool that was not offered in this section)` whenever `global_exists` is true. Model-call dispatch in `crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 180 to 183 already filters with `.filter(|binding| binding.alias() == alias)`, so dispatch treats a model-supplied id as unresolved, but the scope gate does not.
    - Relationship to target work: at the baseline `2b53936e6` the closure looked up slot aliases, which never contain `/`, so a canonical id never earned the suffix. `4dc463c64` widened the lookup to `offered_binding` and created the false case.
    - Impact: when a model requests `web/fetch` while its section advertises `web_fetch`, the round fails with `... in-scope aliases: [..., "web_fetch", ...] (a catalog tool that was not offered in this section)`. The error ends the round and reaches the author through the `models.loop` call site, so it points the author at the wrong fix (an already-present `tools.offer`) instead of the real cause (the model used the id, not the wire name). It contradicts the design record, which attaches the suffix only to "a model call to a catalog tool its section did not offer". Reachability is medium-low: it needs a model to emit a function name containing `/`. It is deterministic once that happens.
    - Reversal cost: low. Crate-private engine path and a `ToolSet` method outside the facade; no wire, persisted, or facade change.
    - Target state: the scope gate and model-call dispatch both resolve model-supplied names through one wire-name-only lookup on `ToolSet`; a canonical id from the model gets the plain out-of-scope error without the suffix.
- Cheap fixes: none.
- Exposed pre-existing debt (included by owner decision)
  - D1-2 (exposed): the Engine wording rule runs in no local Engine gate, so Engine work keeps landing text it forbids.
    - Evidence: the only enforcement is the Node test "Engine crates name the caller, never the Host or the Harness's crates and types" in `crates/workshop/ui/test/docs-claims.mjs`. The root `AGENTS.md` names that file as the enforcer (lines 23 and 27), but its `## Verification` list (lines 50 to 58) has no Node command, and `.githooks/pre-push` runs only the headless gateway check, clippy, and `cargo deny`. CI's `ui` job runs it.
    - Recurrence: `2e15892a6` (plan `vibe/2026-10-09-2-messages-userdata-refactor.md`) added "the Workshop chat agent's turn loop" to `crates/promptforge-internal/engine/src/execute/tests/models_loop-author-shapes.rs` line 3; it stayed on `master` for 15 commits until `20d879ab1` reworded it as a prerequisite fix. An earlier corrective episode on 2026-10-06 (`28bb8f7ab`, `f938e8de4`, `e141d1565`, `b99b6cbc3`, then `0a367c8f8` adding the scan) fixed the same kind of drift. The shared cause is that the Engine's local verification never runs the wording scan.
    - Impact: an Engine-only change passes every command in the root `## Verification` list while breaking a rule the same file states, and the fix lands later on unrelated work. The same placement gap covers the file's other rules (the rulebook Definitions guard and the Plugin activation guard), which the same command also runs.
    - Reversal cost: low. Tooling and docs only.
    - Target state: the root `AGENTS.md` `## Verification` list and `.githooks/pre-push` both run the existing scan.
- Rejected candidates included by owner decision
  - D1-12 (residual, `4dc463c64`): crate-private names in `promptforge-lua` still describe the removed API. At `70f7fd577`, `crates/promptforge-internal/lua/src/tools/decode.rs` has `ToolsAddEntry` (line 46, whose `alias` field holds the name a `tools.offer` or `tools.always_offer` argument stands for), `collect_tools_add_entries` (line 71, which decodes those two functions' arguments), `add_local_params_schema` (line 130, which builds the `tools.offer_local` schema), and `tool_alias` (line 25, which reads a string or a tool object's id). `crates/promptforge-internal/lua/src/scope.rs` has `ToolCallCounts::aliases` (line 71) and `alias` parameters on `new`, `ensure`, `increment`, and `get`, though the keys are catalog tool ids or local aliases. `tools.add` and `tools.add_local` no longer exist, so the names point readers at a removed API. The reshape kept them on purpose to limit churn; none is in `crates/promptforge/public-api.txt`.
  - D1-17 (residual, made stale by `4dc463c64`): `crates/harness-internal/runner/src/performers.rs` lines 79 and 80 document `ToolPerformer::call`'s `alias` as "the prompt-local name the call used". Since `4dc463c64`, `Effect::ToolCall.alias` holds the tool's wire name, or its canonical id when the run could not offer the tool (`crates/promptforge-internal/engine/src/execute/run/effect.rs`). The value feeds only diagnostics, so the doc misleads without changing behavior.
- Rejected candidates: 16
  - Residual but acceptable (11): D1-3, D1-4, D1-6, D1-7, D1-8, D1-9, D1-10, D1-13, D1-18, D1-19, D1-20. Each is a recorded owner decision at prompt language version 0, a crate-private shape with no consequence, or reachable public behavior that is not dead code.
  - Weak or speculative (4): D1-5, D1-11, D1-14, D1-16. Structural leads or risks that depend entirely on the deferred per-model tool-name rewrite, with no demonstrated consequence today.
  - Unrelated pre-existing (1): D1-15.
  - False (0).

</product-contract>
<implementation-contract>

## Technical Design

- D1-1, one model-facing lookup on `ToolSet`:
  - `crates/promptforge-internal/lua/src/handles.rs`: add `ToolSet::wire_binding(&self, name: &str) -> Option<&ToolBinding>`, which returns the offered binding whose wire name (`ToolBinding.alias`) equals `name` and never matches by canonical id. Doc: a model reaches tools only by the wire names a round advertised. `offered_binding` keeps its signature and behavior for script calls and Lua lookups.
  - `crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs` lines 180 to 183: the model-call branch (`call_id.is_some()`) calls `tool_set.wire_binding(alias)` in place of `offered_binding(alias).filter(|binding| binding.alias() == alias)`. Behavior is unchanged.
  - `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs` lines 269 to 275: the `global_exists` closure calls `wire_binding(name).is_some()` in place of `offered_binding(name).is_some()`.
  - `crates/promptforge-internal/engine/src/error.rs` lines 240 and 241: the `OutOfScopeToolCall.global_exists` field doc says the name is the wire name of a tool the run offers. The error text at line 235 is unchanged.
  - `ToolSet` is outside the facade, so `crates/promptforge/public-api.txt` does not change.
- D1-2, reuse the existing scan in local gates:
  - Root `AGENTS.md` `## Verification`: add a bullet for the wording and rulebook scan, `node --test crates/workshop/ui/test/docs-claims.mjs`, saying it enforces the Engine wording rule, the rulebook Definitions, and the Plugin activation wording, needs no `npm ci`, and runs from any directory because the file resolves the repository root from its own path.
  - `.githooks/pre-push`: add a step that echoes `==> Running docs-claims wording scan...` and runs `node --test crates/workshop/ui/test/docs-claims.mjs`, placed before the clippy step so it fails fast. Run it unconditionally: the clippy step already builds UI crates through npm, so Node is present wherever the hook passes today.
- D1-12, crate-private renames in `promptforge-lua`, with every use site and test updated:
  - `ToolsAddEntry` becomes `OfferEntry`, and its `alias` field becomes `name` (`tools/decode.rs`, used in `tools.rs`).
  - `collect_tools_add_entries` becomes `collect_offer_entries` (`tools/decode.rs`, `tools.rs`, `tools/tests.rs`, `tools/tests-offering.rs`).
  - `add_local_params_schema` becomes `local_params_schema`, and its three tests in `tools/tests.rs` drop the `add_` prefix.
  - `tool_alias` becomes `tool_name` (`tools/decode.rs`, the `pub(crate) use` in `tools.rs`, `protocol/parse.rs`, `tools/tests.rs`).
  - `ToolCallCounts::aliases` becomes `ToolCallCounts::names` (`scope.rs`, its one caller in `tools.rs`), and the `alias` parameters and docs of `new`, `ensure`, `increment`, and `get` say `name`. The internal invariant message `tool call counts: alias {alias:?} was not pre-seeded` becomes `tool call counts: name {name:?} was not pre-seeded`; it is not author-facing or specified.
  - `install_tools`, `ToolBinding.alias` (the wire name), the effect and event `alias` fields, and every author-facing text keep their names.
- D1-17: `crates/harness-internal/runner/src/performers.rs` lines 79 and 80 say `alias` is the tool's wire name, or its canonical id when the run could not offer it, for the performer's own diagnostics. The parameter name and signature stay.
- No module ownership, dependency direction, trust boundary, wire format, or persisted format changes.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused (D1-1): add a sibling of `model_calling_an_offered_but_unscoped_tool_is_a_hard_error` in `crates/promptforge-internal/engine/src/execute/tests/tool_loop.rs` (around line 299), built from the same `FixtureTools` fixture, in which the model requests the canonical id of a tool the section advertised. Assert the round fails with `OutOfScopeToolCall`, the message ends with the in-scope list, and it does not contain "not offered in this section". This test fails at `70f7fd577` because `offered_binding` resolves the id and sets the suffix; if it passes before the change, the finding is false: keep the test and skip the code change.
- Regression (D1-1): `model_calling_an_offered_but_unscoped_tool_is_a_hard_error` (suffix present, line 344) and `model_calling_pure_unknown_tool_is_a_hard_error` (suffix absent, line 376) pass unchanged. The model-call dispatch tests that reject a model-supplied id keep passing, which shows the `wire_binding` swap in `tool_call.rs` changed no behavior.
- Fault injection (D1-2): temporarily add the word "Workshop" to a doc comment in an Engine crate test file, run `node --test crates/workshop/ui/test/docs-claims.mjs` from the repository root and confirm it fails naming that file, then revert and confirm it passes. Run `bash .githooks/pre-push` once on the clean tree and confirm the new step runs and passes.
- Renames (D1-12): no new test; the compiler proves every use site moved, and the existing `tools/tests.rs`, `tools/tests-offering.rs`, and `protocol` tests pass under their new names. The `git grep` in Success criteria finds none of the old names.
- Doc (D1-17): `cargo doc -p harness-runner --no-deps` with `RUSTDOCFLAGS="-D warnings"` builds clean.
- Exit checks: every command in the root `AGENTS.md` `## Verification` list, including the new docs-claims scan; `cargo nextest run --locked -p promptforge-engine -p promptforge-lua --all-features`; the facade surface check `cargo +nightly-2026-09-05 xtask api --check` shows no diff.

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences
  - D1-1: add one wire-name-only lookup, `ToolSet::wire_binding`, and use it from both model-call dispatch and the scope gate. Consequence: the two model-facing rules share one definition, which is the cause of D1-1; one new method on a type outside the facade.
  - D1-1: keep the error text unchanged and fix only when the suffix appears. Consequence: no spec text moves; a model that sends a canonical id gets the plain out-of-scope error, whose in-scope list already names the wire name to use.
  - D1-2: reuse the existing Node scan instead of porting it. Consequence: one rule table stays the single source; local gates need Node, which the clippy step already requires.
  - D1-2: run the scan unconditionally in the pre-push hook. Consequence: a machine without Node fails the hook, which it already does at the clippy step's UI build.
  - D1-12: name the decoded entry after what it is (`OfferEntry.name`, `collect_offer_entries`, `tool_name`, `local_params_schema`, `ToolCallCounts::names`). Consequence: about two dozen crate-private references and three test names move once; no facade or behavior change.
  - D1-17: fix the doc in place and keep the `alias` parameter name. Consequence: the trait signature is unchanged for every performer.
- User-resolved choices
  - The owner chose to include the exposed D1-2 in this plan.
  - The owner chose to include the rejected D1-12 and D1-17 in this plan.
- Rejected alternatives
  - D1-1, inline `.filter(|binding| binding.alias() == name)` in the scope gate: fixes the bug but keeps two copies of one rule, which is how D1-1 arose.
  - D1-1, a wire-name hint suffix ("call it as web_fetch"): better guidance, but it changes specified error text and is not needed to remove the contradiction.
  - D1-2, port the scan into `build-xtask` (`engine_guards.rs`): runs under Cargo alone, but duplicates the rule table, which must then stay in sync with the Node file. Revisit only if Node on developer machines stops being guaranteed.
  - D1-2, guard the hook step with `command -v node` like `cargo-deny`: would silently skip the check where it matters.
  - D1-12, keep the names: zero churn, but the names keep pointing readers at `tools.add` and `tools.add_local`, which no longer exist.
  - D1-17, rename the `alias` parameter on `ToolPerformer::call`: touches every performer for a doc-level problem.
- Assumptions and risks
  - D1-1 reachability depends on whether a serving backend lets a model emit `/` in a function name; many restrict names to `[A-Za-z0-9_-]`. The fix is correct either way.
  - CI run history was not inspected; the 15-commit red span for D1-2 rests on the workflow file, `git log`, and the reshape plan's own record.
  - Test files of `4dc463c64` were read only where a check needed them, so test-only debt there may be missed.

### Deferred and Out of Scope

- D1-3: declared-plugin tools can lose a wire-name collision, and an undeclared plugin whose every tool was left out disappears from `plugins.extras()`. The design record adopts the left-out-with-a-log-line rule, and its product and implementation contracts word `extras()` differently. Revisit if MCP server naming makes collisions common.
- D1-5, D1-14, and D1-15: the bare-string provider, the shipped prompt's wire-name prose, and Workshop rounds served by a different model than the handle describes. Revisit together with the deferred per-model tool-name rewrite.
- D1-13: `Requirements::merge`'s drop step no longer fires for any producer in this repository, but it is documented facade behavior. Revisit if the facade's `Requirements` is reshaped.
- D1-20: a declared plugin missing one expected tool fails at `tools.offer` instead of at prepare. Revisit if prompts need prepare to refuse a run over one missing tool.
- The `promptforge-docs` guide stays stale, as the reshape plan decided.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (the workspace's only default member, so plain `cargo build` builds just the gateway); the desktop app is an explicit `cargo build --locked -p workshop`. Rust build scripts bundle the UIs, so a fresh clone runs `npm ci --prefix crates/workshop` and `npm ci --prefix crates/gateway/config-ui/ui` first, as CI does.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-filter>`, adding `--all-features` for any crate other than `workshop`, `workshop-server`, and `workshop-server-api` to match the full suite. A JS test file runs with `node --test <file>` from its package directory.
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for non-workshop crates and `cargo nextest run --locked -p <crate>` for the three workshop crates above. JS packages: `npm test --workspace <ui|look|platform>` from `crates/workshop`, or `npm test` from `crates/gateway/config-ui/ui`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. The first run includes `build-xtask`, whose tests are the boundary and structural checks. UI suites: `npm test --workspaces --if-present` in `crates/workshop` and `npm test` in `crates/gateway/config-ui/ui`. Config lives in `.config/nextest.toml` (60 s slow timeout, terminate after 3 periods; STT crates run in a capped `heavy` group).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` and `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets`, both with `CARGO_BUILD_WARNINGS=deny` (in PowerShell, `$env:CARGO_BUILD_WARNINGS='deny'`). Also `cargo check -p gateway --no-default-features` for the headless build shape (the only allowed standalone `cargo check`), `npm run typecheck --workspaces --if-present` in `crates/workshop`, `npm run typecheck` in `crates/gateway/config-ui/ui`, and `cargo deny check` plus `cargo hakari verify` for dependency policy.
- Formatter check command: `cargo fmt --all --check` (stable toolchain, `style_edition = "2024"`; the pre-commit hook runs it).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`, plus `cargo doc -p promptforge --no-deps` with the same flags and default features. Facade surface: `cargo +nightly-2026-09-05 xtask api --check` against the committed `crates/promptforge/public-api.txt` (the pinned nightly is named in `crates/build-xtask/src/api/toolchain.rs`).
- Test placement and naming conventions: Unit tests live in a sibling file wired with `#[cfg(test)] #[path = "<stem>-tests.rs"] mod tests;` (for example `parse.rs` beside `parse-tests.rs`), or in a `tests/` subdirectory once a module has three or more test files (for example `engine/src/execute/tests/`). Integration tests compile as one binary per crate from `tests/it/main.rs` or `tests/suite/main.rs`, with shared helpers in `support.rs` and kebab-labelled siblings such as `prepare-files.rs`. Test function names are snake_case sentences describing the behavior (`each_test_call_has_a_filesystem_of_its_own`). Plugin crates test through `promptforge-plugin`'s `test-support` feature, enabled from `[dev-dependencies]` only. JS tests are `node:test` files in `test/*.mjs` (workshop `ui`, `look`, `platform`) or `src/**/*.test.mjs` (config UI).
- Directory map: `crates/` holds every crate. `promptforge` is the Engine facade, with private Engine crates in `promptforge-internal/` (`types`, `vfs`, `parser`, `lua`, `model-client`, `engine`). `promptforge-plugin` is the Plugin contract and `plugin-web`, `plugin-mcp`, `plugin-user-input` implement it. `harness` is the Harness facade, `harness-internal/runner` its private runner, and `harness-gateway-client` the gateway client. `gateway/` holds the gateway family (`app` binary, `config`, `config-ui`, `local`, `routing`, `protocol`, `cloud-providers`, `web-search`, `logging`, `progress`, and the `stt/` speech subsystem); `gateway-api-types` and `gateway-api-discovery` are its public pair. `workshop/` holds the Workshop Host (`desktop` Tauri app, `server`, `server-api`, tiered library crates, and the `ui`, `look`, `platform` npm workspaces). Also in `crates/`: `shared-*` utilities and `shared-ui` (a TypeScript and CSS package, not a crate), `build-*` tooling (`build-xtask` structural checks and `cargo xtask`, `build-workshop` behind `cargo workshop`, `build-ceiling` file-size check, `build-ui`, `build-user-guide`, `build-llama-cuda`), and `workspace-hack` (cargo-hakari). Top level: `guide/` (mdBook docs for language, gateway, and workshop, plus the site landing page), `prompts/` (sample prompts), `tools/` (Node sidecar staging and TTS scripts with tests), `.github/workflows/` (CI, release, nightly), `.githooks/` (pre-commit fmt, pre-push clippy and deny), `.cursor/rules/` (Workshop architecture and SPA rules), `vibe/` (past plans), `local/` (machine-local config and fixtures), `images/`.
- Component boundaries: `cargo test -p build-xtask` enforces these. Engine crates (`promptforge*`) depend on no gateway, workshop, or Harness crate. Inside the Engine, `types` and `vfs` are leaves, `model-client` depends on `types`, `parser` on `lua` and `types`, `lua` on `types`, `vfs`, `model-client`, and `engine` on all of them, and the `promptforge` facade re-exports over them all. Plugin crates depend only on `promptforge-plugin`, `shared-*`, and outside libraries, and `promptforge-plugin` depends on `types` and `vfs`. Harness crates name only `promptforge`, `promptforge-plugin`, and `workspace-hack` outside their family, except that `harness-gateway-client` may name `plugin-web` to implement its `SearchProvider`. Gateway crates depend on no Engine, Harness, or workshop crate. Workshop crates reach the gateway only through `gateway-api-types` and `gateway-api-discovery`, and reach the Engine and Harness only through the public crates (`promptforge`, `promptforge-plugin`, `harness`, `harness-gateway-client`). Containers (`promptforge-internal/`, `harness-internal/`, `gateway/`, `gateway/stt/`, `workshop/`) are private to their families: only `promptforge` and `promptforge-plugin` may enter `promptforge-internal/`, only `harness` may enter `harness-internal/`, and only `gateway-stt` is public within `gateway/stt/`. Workshop tiers run vocabulary (`protocol`, `registry`, `support`), then services (`gateway`, `menu`, `status`), then features (`agents`, `run-log`, `user-state`, `workspace`), then `workshop-server`, each depending only downward. The `workshop` desktop app depends only on `workshop-server-api`, which depends only on `workshop-server`. `shared-*` crates depend on no product crate.
- Conventions summary: The four defined terms (Engine, Harness, Host, Plugin) are capitalized with one meaning each, and Engine crates say "the caller" and never mention the Host (`crates/workshop/ui/test/docs-claims.mjs` scans for this). Every crate sets `publish = false`, inherits `version`, `edition`, `license`, and `repository` plus `[lints] workspace = true`, takes dependencies as `<dep>.workspace = true`, and depends on `workspace-hack`. Most crates call `build-ceiling` from a build script, which fails any Rust file over 500 lines. Manifests comment why each dependency or feature exists. Lints are strict: clippy `all` and `pedantic` deny, `unwrap_used` and `expect_used` deny, bare `#[allow]` is denied so suppressions use `#[expect(..., reason = "...")]`, `unsafe_code` is denied outside owned boundaries, and `missing_docs` and `unreachable_pub` warn (and fail under the gate). Crate docs carry a `## Invariants` section. Source directories stay flat until a group reaches three files, using kebab siblings with `#[path]` below that. Comments explain only non-obvious constraints and cite upstream issue URLs for workarounds. Error messages are written for model consumption, naming required versus actual. JSON that reaches a recorder round-trips exactly, with sorted keys and `float_roundtrip`. Behavior changes ship with tests in the same change. Cargo features gate only real build constraints. Changes to the facade surface update `public-api.txt`. Commits are one-line imperative summaries (`Expose the model provider on model handles`), and a plan closes with `Close plan: <slug>`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Run the docs-claims wording scan from local gates [completed]

- Component: Local wording gate (D1-2)
- Component placement: first. It depends on nothing, and once it lands, every later step's run of the root `AGENTS.md` verification commands includes the wording scan, so the Engine doc edits in Steps 2 and 3 are checked locally before push.
- Pieces: the `## Verification` entry and the pre-push hook step, built jointly in one commit because both run the same command and one fault-injection check covers both.
- Artifacts:
  - Root `AGENTS.md` `## Verification` list: add a bullet that runs `node --test crates/workshop/ui/test/docs-claims.mjs` and says it enforces the Engine wording rule, the rulebook Definitions, and the Plugin activation wording, needs no `npm ci`, and runs from any directory because the file resolves the repository root from its own path.
  - `.githooks/pre-push`: add a step that echoes `==> Running docs-claims wording scan...` and runs `node --test crates/workshop/ui/test/docs-claims.mjs`, placed before the `==> Running Clippy on workspace...` step so it fails fast. Run it unconditionally, with no `command -v node` guard.
- Tests: no committed test, since this is tooling. Run the Testing Plan fault injection: temporarily add the word "Workshop" to a doc comment in an Engine crate test file, run the scan from the repository root and confirm it fails naming that file, then revert and confirm it passes. The injected word is never committed. Then run `bash .githooks/pre-push` once on the clean tree and confirm the new step runs and passes.
- Commit: one commit with both files, for example `Run the docs-claims wording scan from local gates`.

</step-1>

<step-2>

### Step 2: Resolve model-supplied tool names by wire name only

- Component: Model-facing tool lookup (D1-1)
- Component placement: second. It is the one debt the reshape introduced and the only behavior change in the plan. Step 3 also edits `promptforge-lua`; landing this fix first keeps it a self-contained, bisectable commit and leaves the renames as a mechanical follow-on.
- Pieces: the lookup method on `ToolSet`, then its two engine callers and the error field doc. Built jointly in one commit: the method exists only so both callers share one definition, and the only test that observes the fix runs through the engine.
- Failing-first test: in `crates/promptforge-internal/engine/src/execute/tests/tool_loop.rs`, add a sibling of `model_calling_an_offered_but_unscoped_tool_is_a_hard_error` built from the same `FixtureTools` fixture, named as a snake_case behavior sentence (for example `model_calling_an_advertised_tool_by_canonical_id_gets_no_not_offered_suffix`). The model requests the canonical id (such as `web/fetch`) of a tool its section advertised under its wire name (such as `web_fetch`). Assert the round fails with `OutOfScopeToolCall`, the message ends with the in-scope list, and it does not contain "not offered in this section". Run it in the worktree before making any code change below, without checking out another revision, and confirm it fails. If it passes, the finding is false: keep the test, skip the code changes below, and report it.
- Code:
  - `crates/promptforge-internal/lua/src/handles.rs`: add `ToolSet::wire_binding(&self, name: &str) -> Option<&ToolBinding>`, returning the offered binding whose wire name (`ToolBinding.alias`) equals `name` and never matching by canonical id. Its doc says a model reaches tools only by the wire names a round advertised. `ToolSet::offered_binding` keeps its signature and behavior for script calls and Lua lookups.
  - `crates/promptforge-internal/engine/src/execute/scheduler/tool_call.rs`: in the model-call branch (`call_id.is_some()`), call `tool_set.wire_binding(alias)` in place of `offered_binding(alias).filter(|binding| binding.alias() == alias)`. The script branch (`ToolId::parse` with the `catalog_binding` fallback) is unchanged.
  - `crates/promptforge-internal/engine/src/execute/scheduler/chat.rs`: the `global_exists` closure calls `wire_binding(name).is_some()` in place of `offered_binding(name).is_some()`.
  - `crates/promptforge-internal/engine/src/error.rs`: the `OutOfScopeToolCall.global_exists` field doc says the name is the wire name of a tool the run offers. The `#[error]` text is unchanged.
- Regression: `model_calling_an_offered_but_unscoped_tool_is_a_hard_error` (suffix present) and `model_calling_pure_unknown_tool_is_a_hard_error` (suffix absent) pass unchanged, and the model-call dispatch tests that reject a model-supplied id keep passing, which shows the `tool_call.rs` swap changed no behavior.
- Verify: `cargo nextest run --locked -p promptforge-engine -p promptforge-lua --all-features`; `cargo +nightly-2026-09-05 xtask api --check` shows no diff, because `ToolSet` is outside the facade and `crates/promptforge/public-api.txt` does not change; every root `AGENTS.md` verification command, including the scan from Step 1.
- Commit: one commit with the test and the code, for example `Resolve model tool names by wire name only`.

</step-2>

<step-3>

### Step 3: Rename promptforge-lua internals after the offer API

- Component: Offer-API names (D1-12)
- Component placement: third. It also edits `promptforge-lua`, so it lands as its own commit on top of Step 2, rebased if Step 2 changed shared files. It adds no behavior, so it follows the behavior fix rather than competing with it for review.
- Pieces: the decode-side names in `tools/decode.rs` and the counter names in `scope.rs`. They do not depend on each other; built jointly in one commit because neither gets a new test and one set of checks (the compiler, the existing `promptforge-lua` and `promptforge-engine` tests, and the `git grep` below) covers both completely.
- Artifacts, all under `crates/promptforge-internal/lua/src`, with every use site and test updated:
  - `ToolsAddEntry` becomes `OfferEntry`, and its `alias` field becomes `name` (`tools/decode.rs`, used in `tools.rs`).
  - `collect_tools_add_entries` becomes `collect_offer_entries` (`tools/decode.rs`, `tools.rs`, `tools/tests.rs`, `tools/tests-offering.rs`).
  - `add_local_params_schema` becomes `local_params_schema` (`tools/decode.rs` and its callers), and its three tests in `tools/tests.rs` drop the `add_` prefix: `add_local_params_schema_builds_object_schema_with_required_fields`, `add_local_params_schema_sorts_required_so_the_schema_text_is_deterministic`, and `add_local_params_schema_rejects_an_unsupported_type`.
  - `tool_alias` becomes `tool_name` (`tools/decode.rs`, the `pub(crate) use` in `tools.rs`, `protocol/parse.rs`, `tools/tests.rs`). Its three tests in `tools/tests.rs` take the new prefix so the Success criteria `git grep` finds nothing: `tool_alias_accepts_a_bare_string`, `tool_alias_reads_the_id_off_a_tool_object`, and `tool_alias_rejects_tables_other_types_and_other_userdata` become `tool_name_accepts_a_bare_string`, `tool_name_reads_the_id_off_a_tool_object`, and `tool_name_rejects_tables_other_types_and_other_userdata`.
  - `ToolCallCounts::aliases` becomes `ToolCallCounts::names` (`scope.rs` and its one caller in `tools.rs`); the `alias` parameters and docs of `new`, `ensure`, `increment`, and `get` say `name`; the internal invariant message `tool call counts: alias {alias:?} was not pre-seeded` becomes `tool call counts: name {name:?} was not pre-seeded`.
  - Unchanged: `install_tools`, `ToolBinding.alias` (the wire name), the effect and event `alias` fields, every author-facing or specified text, and every facade item.
- Tests: no new test. The renamed tests and the existing `tools/tests.rs`, `tools/tests-offering.rs`, and `protocol` tests pass under their new names.
- Verify: `cargo nextest run --locked -p promptforge-lua -p promptforge-engine --all-features`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features` with `CARGO_BUILD_WARNINGS=deny` (in PowerShell, `$env:CARGO_BUILD_WARNINGS='deny'`); `cargo fmt --all --check`; and `git grep -nF -e ToolsAddEntry -e collect_tools_add_entries -e add_local_params_schema -e tool_alias -e "fn aliases(" -- crates/promptforge-internal/lua/src` prints nothing.
- Commit: one commit, for example `Rename promptforge-lua offer internals after the offer API`.

</step-3>

<step-4>

### Step 4: Correct the ToolPerformer::call alias doc

- Component: Harness doc (D1-17)
- Component placement: last. It depends on nothing and nothing depends on it, so any position works; last lets the plan's exit checks run once on the finished tree.
- Pieces: one piece, the doc comment, so no further division.
- Artifact: `crates/harness-internal/runner/src/performers.rs` lines 79 and 80. The `ToolPerformer::call` doc says `alias` is the tool's wire name, or its canonical id when the run could not offer the tool, for the performer's own diagnostics, and no longer says "prompt-local name". The `alias` parameter name and the trait signature stay.
- Tests: no new test. `cargo doc -p harness-runner --no-deps` with `RUSTDOCFLAGS="-D warnings"` (in PowerShell, `$env:RUSTDOCFLAGS='-D warnings'`) builds clean, and `rg -n "prompt-local name" crates/harness-internal/runner/src/performers.rs` prints nothing.
- Exit checks for the whole plan, after this commit: every command in the root `AGENTS.md` `## Verification` list, including the docs-claims scan from Step 1; `cargo nextest run --locked -p promptforge-engine -p promptforge-lua --all-features`; and `cargo +nightly-2026-09-05 xtask api --check` shows no diff.
- Commit: one commit, for example `Describe the ToolPerformer alias as the wire name`.

</step-4>

</execution-plan>
