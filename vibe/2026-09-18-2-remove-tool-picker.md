---
name: Remove tool picker
overview: Archive `promptforge-tool-picker` as a frozen non-compiling artifact in the sibling `promptforge-design` repository, then remove the crate and every feature that existed only for it (fuzzy tool slots, near-duplicate tool conflicts, the capability description lint, the dead `models.bind` resolver) from the PromptForge workspace, together with the candle/tokenizers/hf-hub dependency stack, the embedded embedding model, and the CI model cache.
todos:
  - id: archive
    content: Copy the crate to promptforge-design/study-tool-picker/crate/ with a README note and commit there
    status: completed
  - id: consumers
    content: Remove the picker, fuzzy slots, conflicts, the capability lint, and the models.bind resolver from model-client, lua, parser, runtime, and workshop-workspace
    status: pending
  - id: crate
    content: Delete the crate; prune root manifest members and the candle/tokenizers/hf-hub/safetensors/half pins; regenerate hakari; fix nextest filters; remove the CI model cache
    status: pending
  - id: ui-docs
    content: Remove fuzzy tool rendering from the Workshop SPA; update guide sources, crate READMEs, and dependency-surface.md with fresh measurements
    status: pending
  - id: verify
    content: Full gate list, cargo tree invariants, and reference sweep
    status: pending
isProject: false
---

# Remove the tool picker

All paths below are relative to the PromptForge repository root (`promptforge/` in the workspace) unless prefixed with `../promptforge-design/`, the sibling design-archive repository. Facts about the dependency graph come from `vibe/dependency-surface.md`, measured on 2026-09-18 at the commit landing the `toml 1` pin with cargo 1.98.0.

<product-contract>

## Product Requirements

The tool picker is a local sentence-embedding engine that maps prose capability descriptions to tools. It is switched off in every shipped build: the Workshop builds its session environment without a picker, so fuzzy slots already go unfilled and the near-duplicate scan already records nothing. Its only live use is an advisory lint inside the capability registry. Yet every binary that links the runtime still embeds the fp16 `BAAI/bge-small-en-v1.5` weights, compiles the 50-crate candle and tokenizers stack, needs network access to Hugging Face on a first build, and drags a model cache through four CI workflows. The picker is removed outright, archived as a frozen artifact in the design repository, and the three language and host features that existed only for it are removed with it.

- Problem and users: `crates/workshop/sessions/src/agents/environment.rs` builds `Environment::new().registry(registry)` with no `.picker(...)`, so the fuzzy fill and the conflict scan in `crates/promptforge-api-runtime/src/execute/fill.rs` are inert in production; the single live use is `CapabilityRegistry::new()` building `ToolPicker::empty` for a registration-time lint (`crates/promptforge-api-runtime/src/capabilities.rs`). The cost is paid by everyone who builds or ships the runtime or the Workshop: `crates/promptforge/tool-picker/build.rs` downloads and converts the model on first build and `src/assets.rs` embeds it with `include_bytes!`; the candle-core/candle-nn/candle-transformers/tokenizers set is 50 exclusive crates, the second-largest exclusive cost in the tree (`vibe/dependency-surface.md`, ranked table); `.github/actions/hf-model-cache/action.yml` is used eleven times across `ci.yml`, `nightly.yml`, `release-workshop.yml`, and `workshop-installer-smoke.yml`.
- Goals:
  - The crate `promptforge-tool-picker` and every reference to it leave the PromptForge repository: code, tests, benches, manifests, hakari output, nextest configuration, CI, crate READMEs, and the user guide.
  - The crate's source is preserved verbatim as a non-compiling artifact in `../promptforge-design/study-tool-picker/crate/`, beside the study that produced it.
  - Fuzzy tool slots (the frontmatter `tools:` map form with `want` and `optional`), near-duplicate tool conflicts (bind-time recording and the scope-time error), and the capability description lint are removed as features.
  - The dead `models.bind` semantic resolver in `crates/promptforge/model-client` and its error variants are removed, since that machinery is the model client's only reason to name the picker and `models.bind` itself is already gone from the language.
  - The workspace pins `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers`, `hf-hub`, `safetensors`, and `half` leave the root `Cargo.toml` when no remaining crate needs them.
  - `vibe/dependency-surface.md` is refreshed with post-removal measurements, as its own success bar requires when a ranked dependency is removed.
- Non-goals:
  - Reducing the `workspace-hack` floor (Finding 2 in `vibe/dependency-surface.md`) or replacing `turso` (Finding 3).
  - Touching `shared-progress` (the gateway and workshop use it), `promptforge_api_types::tools::ToolId` (the runtime and its catalog use it), `sha2` or `anyhow` (the gateway uses them), or `criterion` at the workspace level (`crates/promptforge/lua/benches/surface.rs` uses it).
  - Repairing `../wg21-paperflow/crates/papergate/Cargo.toml`, which path-depends on crate directories that stopped existing in the 2026-09-17 reorg and is already broken.
  - Renaming the Workshop UI's "model picker" widgets or the gateway's "quant picker" wording; those are unrelated uses of the word.
- Success criteria:
  - `cargo tree --workspace -e normal,build --target all -i candle-core` prints nothing, and likewise for `tokenizers`, `hf-hub`, and `safetensors`.
  - A search of `crates/`, `.github/`, `.config/`, `guide/src/`, `Cargo.toml`, and `AGENTS.md` for `tool-picker`, `tool_picker`, `ToolPicker`, `FuzzySlot`, `NearDuplicate`, `hf-model-cache`, and `HF_TOKEN` returns nothing. History under `vibe/` is exempt, and so are the gateway's own `HF_TOKEN` reads under `crates/gateway/` (its Hugging Face download token for local model artifacts, unrelated to the picker).
  - `../promptforge-design/study-tool-picker/crate/` holds the 20 files of the crate as they were at the removal commit, and `../promptforge-design/study-tool-picker/README.md` names it.
  - Every verification gate in the Testing Plan passes; `cargo hakari verify` is clean; `cargo test -p build-xtask` passes.
- Constraints:
  - Workspace lints (`Cargo.toml`, `[workspace.lints]`): `unsafe_code` forbidden; `unwrap_used`, `expect_used`, clippy `all` and `pedantic` denied; `missing_docs` and `unreachable_pub` warn. Removal must not leave dead imports, unused fields, or orphaned doc links (`rustdoc::broken_intra_doc_links` is denied).
  - The product-boundary matrix and structural harness (`crates/build-xtask/src/product.rs`, `crates/build-xtask/src/tidy.rs`) run as `cargo test -p build-xtask` and must pass after the crate leaves the member list.
  - Repository policy admits no new structural check without explicit user approval; this plan adds none.
  - `ToolSlot` in `crates/promptforge/parser/src/contract.rs` keeps `#[non_exhaustive]`: the deferred open host-offered posture is still planned to join it.
  - Files stay under the 500-line ceiling enforced on the workshop crates carrying the invariants marker (`AGENTS.md`).
  - The `../promptforge-design` worktree carries unrelated uncommitted changes; the archive commit stages only the copied crate files and the README edit.
- Open questions: None

## Functional Specification

No shipped behavior changes for the Workshop's chat sessions, which never had a picker. Three declared surfaces narrow: the prompt frontmatter accepts only exact tool paths, tool bindings carry no conflict records, and the runtime's error vocabulary loses the variants nothing has produced since `models.bind` was removed.

- Actors and workflows:
  - Prompt author: `tools:` in frontmatter maps an alias to a bare string global tool path (`namespace/pack/name`). The map form `{ want: "...", optional: true }` is no longer a slot shape and fails at parse. `tools.always` and `tools.add` in Lua are unchanged.
  - Host developer embedding `promptforge-api-runtime`: `Environment` has no `picker()` builder; `CapabilityRegistry::register` performs id uniqueness and normalization-collision checks only, with no description lint. `ToolBinding` (from `promptforge-lua`, re-exported through the runtime) has no `conflicts` field or accessor.
  - Workshop user: the run panel's tool rows render exact slots only.
  - Developer building the workspace: no first-build network fetch, no `HF_TOKEN` in CI, no model cache step in CI. The gateway's runtime `HF_TOKEN` read for local model downloads is unchanged.
- Inputs and outputs: prompt frontmatter `tools:` values are strings; the runtime's prepare journal records exact fills only. Workshop's prompt-contract JSON (`crates/workshop/workspace/src/handlers-prompts.rs`) emits tool entries of kind `exact` only.
- States and validation: `ToolSlotVisitor` accepts `visit_str` only; a map value is a deserialization error whose message names the expected shape ("an exact tool path string").
- Errors and recovery: `Error::NearDuplicateTools` and the five `ModelBind`, `ModelBindQuery`, `ModelAbsent`, `ModelDuplicate`, `ModelAmbiguous` variants are removed from `crates/promptforge/model-client/src/error.rs`, `crates/promptforge/lua/src/error.rs`, and `crates/promptforge-api-runtime/src/error.rs` and their kind and mapping tables. No runtime path produced them.
- Security and privacy behavior: the runtime no longer performs a build-time download from Hugging Face; no other change.
- Acceptance criteria: the Success criteria above, plus the full gate list in the Testing Plan.

</product-contract>
<implementation-contract>

## Technical Design

The change is a removal across eight crates, the workspace manifests, CI, the Workshop SPA, and documentation, plus one copy into the sibling repository. No new abstractions are introduced. Exact tool slots, the capability registry, and the run's tool catalog keep their shapes minus the picker-dependent members.

- Architecture: unchanged product topology. `promptforge-api-runtime` remains the one door; `crates/promptforge/` loses one private member. Dependency directions are unaffected: `promptforge-model-client` stops depending on the picker, and nothing else gains an edge.
- Modules and interfaces:
  - `crates/promptforge/parser/src/contract.rs`: `ToolSlot` keeps `Exact(ToolId)` and `#[non_exhaustive]`; `FuzzySlot`, `ToolSlot::Fuzzy`, and `ToolSlotVisitor::visit_map` are removed. `crates/promptforge/parser/src/lib.rs` stops re-exporting `FuzzySlot`. Doc lines in `crates/promptforge/parser/src/build.rs` (near 81 and 224) describe exact slots only.
  - `crates/promptforge/lua/src/handles.rs`: `Conflict`, the `conflicts` field on `ToolBinding`, its `PartialEq`/`Debug` participation, and `conflicts()` are removed; `crates/promptforge/lua/src/lib.rs` (near line 132) stops exporting `Conflict`.
  - `crates/promptforge/model-client/src/model.rs`: remove `mod resolver`, `pub use resolver::PickerModelResolver`, `picker_catalog_from`, `model_to_picker_id`, `model_from_picker_id`, `escape_segment`, `unescape_segment`, `PICKER_MODEL_LABEL`, `ModelCatalogFiltered`, `ModelResolver`, `ResolvedModel`, `satisfies_constraints`, and the `promptforge_tool_picker` import; delete `src/model/resolver.rs`; remove `ModelBindOpts` and `impl From<&ModelBindOpts> for ModelInvocation` from `src/model/options.rs` and its re-export; drop the two picker-id round-trip tests in `src/model/tests.rs`.
  - `crates/promptforge-api-runtime/src/capabilities.rs`: `CapabilityRegistry` loses the `lint: ToolPicker` field, `LINT_KEY_SEGMENT`, the post-insert lint, and the picker import; the `ToolId` import, if still needed, comes from `promptforge_api_types::tools::ToolId` rather than the picker's re-export. `capabilities-tests.rs` loses the lint tests.
  - `crates/promptforge-api-runtime/src/execute/fill.rs`: `fill_tool_bindings` loses its `picker` parameter and the `ToolSlot::Fuzzy` arm; `fuzzy_fill_picker`, `fill_fuzzy_slot`, and `record_near_duplicate_conflicts` are deleted with the picker imports.
  - `crates/promptforge-api-runtime/src/execute/environment.rs`: `Environment` loses the `picker` field, the `picker()` builder, the `Debug` field, the import, and the picker and fuzzy sentences in its docs; `prepare` passes no picker.
  - `crates/promptforge-api-runtime/src/execute/bindings.rs`: `ToolBindings` loses `conflicts`, `record_conflict`, `conflicts()`, the `Conflict` type use, `bound_ids` if it has no remaining caller, and the symmetric-conflict test.
  - `crates/promptforge-api-runtime/src/execute/scope.rs`: the conflict check over in-scope bindings is removed. `src/tools.rs`: `NearDuplicateDiagnostic` is removed; `src/lib.rs` (near line 91) stops re-exporting it. `src/error.rs` and `src/execute/error.rs`: `Error::NearDuplicateTools` and the `ModelBind*` variants and their mapping arms (near 290-320, 579-600, 680-700, and the test near 847) are removed. Doc mentions at `src/lib.rs` lines 46 and 59, `src/execute.rs` line 35, and `README.md` line 22 are corrected.
  - `crates/promptforge-api-runtime/benches/models_loop.rs` and the `[[bench]]` block in `Cargo.toml` are deleted; `criterion` leaves this crate's dev-dependencies.
  - `crates/workshop/workspace/src/handlers-prompts.rs`: the `Fuzzy` variant of the contract tool enum and its match arm are removed; `handlers-prompts-tests.rs` loses the two fuzzy fixtures (near lines 104 and 169-175).
  - `crates/promptforge-api-types/src/capabilities.rs` (near line 290): the doc sentence about the registration-time near-duplicate lint is removed.
  - Workshop SPA: `crates/workshop/server/ui/src/services/run-api.ts` loses `RunContractToolFuzzy`, the `kind === "fuzzy"` parse branch, and collapses `RunContractTool` to the exact shape; `crates/workshop/server/ui/src/ui/run/run-rows.ts` (near 146-149) renders exact rows only; `crates/workshop/server/ui/test/run-panel.mjs` loses the fuzzy fixture (near 114) and the "exact and fuzzy tools render" check (near 359).
- File and public API changes:
  - Deleted: `crates/promptforge/tool-picker/` (20 files), `crates/promptforge-api-runtime/benches/models_loop.rs`, `crates/promptforge/model-client/src/model/resolver.rs`, `.github/actions/hf-model-cache/`.
  - Root `Cargo.toml`: remove `"crates/promptforge/tool-picker"` from `members`; remove `promptforge-tool-picker` from `[workspace.dependencies]`; remove `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers` (and its unification comment), `hf-hub`, `safetensors`, and `half`, each only after `cargo tree --workspace -e normal,build --target all -i <crate>` confirms no remaining puller.
  - `crates/promptforge-api-runtime/Cargo.toml`: remove both `promptforge-tool-picker` lines (normal and `test-fixtures` dev). `crates/promptforge/model-client/Cargo.toml`: remove `promptforge-tool-picker.workspace = true`.
  - `crates/workspace-hack/Cargo.toml`: regenerated by `cargo hakari generate` and `cargo hakari manage-deps` (`.config/hakari.toml`); `cargo hakari verify` clean. `Cargo.lock` is committed.
  - `.config/nextest.toml`: `package(promptforge-tool-picker)` leaves both `heavy` group filters (lines 23 and 28).
  - CI: every `uses: ./.github/actions/hf-model-cache` step and its `name: Cache the embedding model` line are removed (`ci.yml` five, `nightly.yml` three, `release-workshop.yml` one, `workshop-installer-smoke.yml` one); the `HF_TOKEN: ${{ secrets.HF_TOKEN }}` env entries in `ci.yml`, `nightly.yml`, and `workshop-installer-smoke.yml` are removed; the explanatory comments at `ci.yml` lines 12-18, `nightly.yml` lines 20-22, and `release-workshop.yml` lines 125-126 are removed or rewritten to drop the picker.
  - Documentation: `crates/README.md` (the `promptforge-api-runtime` paragraph no longer lists `tool-picker`); `crates/promptforge/README.md` (the `promptforge-tool-picker` section is removed and the `promptforge-model-client` paragraph no longer names it); `crates/promptforge/model-client/README.md` if it names the resolver; `crates/workshop/server/README.md` if its one "picker" mention is the tool picker rather than the model picker; guide sources `guide/src/language/01-frontmatter-and-structure.md` (line 38), `02-the-run.md` (line 7), `07-tools.md` (lines 31, 97, 124), then the assembled `guide/promptforge-language-guide.md` regenerated with the `build-user-guide` crate. The `guide/src/workshop/*` "picker" hits are the UI model picker and stay.
  - `vibe/dependency-surface.md`: remove the candle row from the ranked table and the picker note in measurement trap 4; remove the `base64 0.13` (via `spm_precompiled`) and `rand 0.8` (via candle) entries from the duplicate-versions list if they left; add a row to "What was replaced, and why" (removed: the picker crate and its embedding stack; replacement: none, exact slots only, with `cargo tree -i candle-core` output as evidence); rerun the shipped-shape counts and update the header commit and date per that file's "Reproducing the measurements" section.
  - Archive: `../promptforge-design/study-tool-picker/crate/` receives a verbatim copy of `crates/promptforge/tool-picker/` (all 20 files: `Cargo.toml`, `build.rs`, `README.md`, `src/{assets,catalog,config,embed,error,lib,model,picker,picker-tests,policy,policy-tests,rank,selected}.rs`, `tests/it/{main,behavior,public_api}.rs`, `tests/fixtures/mixed-servers.json`); `../promptforge-design/study-tool-picker/README.md` gains a paragraph naming the directory as the frozen crate source at the PromptForge removal commit, non-compiling because `version.workspace`, `edition.workspace`, `[lints] workspace`, and `promptforge-api-types.workspace` have no workspace to inherit from there. No manifest in the design repository is edited.
- Data, persistence, failure, security, and privacy constraints:
  - No persisted data or wire format changes. The Workshop's prompt-contract JSON loses an enum case that was never produced by shipped prompts that parse.
  - `Cargo.lock` changes and is committed; `--locked` gates run against the updated lock.
  - Removing the `build.rs` removes the only build-time network access in the workspace's runtime and Workshop closure.

</implementation-contract>
<verification-contract>

## Testing Plan

Verification is the workspace's existing gate list plus dependency-graph invariants and a reference sweep. One new parser test replaces the fuzzy-slot tests; everything else is pruning tests whose subject no longer exists.

- Unit:
  - `crates/promptforge/parser/src/contract/tests.rs`: the two fuzzy-slot tests are removed; one new test asserts that a `tools:` map value fails to parse with a message naming the exact-path expectation.
  - `crates/promptforge-api-runtime`: prune picker and conflict tests in `src/execute/tests/{mod,exec_flow,live_infer,tool_scoping,observations}.rs`, `src/capabilities-tests.rs`, `src/execute/bindings.rs`, `src/error.rs`, and `tests/suite/{prepare,support}.rs` (including `picker_model`, `shared_test_model`, `filled_slots_record_near_duplicate_conflicts_symmetrically`, `bound_with_tools`'s near-duplicate parameter, and the three conflict tests near `tool_scoping.rs` lines 198-290). Remaining exact-slot fill tests keep passing.
  - `crates/promptforge/model-client/src/model/tests.rs`: the picker-id round-trip tests are removed.
  - `crates/workshop/workspace/src/handlers-prompts-tests.rs`: fuzzy fixtures removed; the exact-kind assertions remain.
- Integration and end-to-end: the Workshop sessions end-to-end chat test in `crates/workshop/sessions/src/agents/environment.rs` (`a_chat_session_activates_the_web_capability_and_calls_search_end_to_end`) runs unchanged, proving exact-slot advertising survives. `npm test` in `crates/workshop/server/ui` passes after the run-panel fixture change.
- Regression, security, and performance:
  - `cargo tree --workspace -e normal,build --target all -i candle-core` prints nothing; likewise for `tokenizers`, `hf-hub`, `safetensors`.
  - `cargo hakari verify` clean.
  - Reference sweep: `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache|HF_TOKEN" crates .github .config guide/src Cargo.toml AGENTS.md --glob '!crates/gateway/**'` returns nothing, and `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache" crates/gateway` returns nothing (the gateway's own `HF_TOKEN` reads stay).
  - `cargo deny check` clean.
- Exit criteria: the workspace's verification commands as recorded in `AGENTS.md` (Verification section) all pass:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `cargo test -p build-xtask`
  - `cargo deny check`
  - `npm test` in `crates/workshop/server/ui`

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Remove the picker entirely rather than relocate it. Rationale: it is inert in every shipped build, its only live use is an advisory lint, and its cost (50 exclusive crates, embedded fp16 weights in every runtime-linking binary, a first-build network fetch, eleven CI cache steps) is paid for nothing. User's words: "I want to eliminate the tool picker completely."
  - Archive the crate source in `../promptforge-design/study-tool-picker/crate/` as a non-compiling artifact. Rationale: the design repository already holds the study (`study-tool-picker/`) and spike (`spike-tool-picker/`) the crate came from, so the frozen source sits beside its evidence; no workspace there means it cannot compile, which is the intent. User's words: "Put it in promptforge-design as a non-compiling artifact."
  - Remove fuzzy tool slots, near-duplicate tool conflicts, and the capability description lint as features rather than stub them. Rationale: each exists only to consume picker output; a grammar that can never fill and a conflict record nothing can produce are traps for authors and hosts. User's words: "I agree with your recommend removing all three (Fuzzy tool slots, near-dup tool conflicts, capability description lint)."
  - Remove the `models.bind` resolver machinery in `promptforge-model-client` (`PickerModelResolver`, `ModelBindOpts`, `ModelResolver`, `ResolvedModel`, the id escape helpers, the `ModelBind*` error variants). Rationale: `models.bind` was already removed from the language (`crates/promptforge-api-runtime/src/execute/tests/exec_flow.rs` near line 2392 asserts calling it fails), nothing constructs the resolver, and it is the model client's only edge to the picker. Falls under "remove all code that references it."
  - Keep `#[non_exhaustive]` on `ToolSlot`. Rationale: the deferred open host-offered posture is documented on the enum as a future member.
  - Keep `criterion` in the workspace, `shared-progress`, `ToolId` in `promptforge-api-types`, `sha2`, and `anyhow`. Rationale: each has a remaining consumer named in Non-goals.
  - Keep the step count low and run a single full verification at the end. User's words: "keep the number of steps low and only do a full verify at the end".
- Rejected alternatives:
  - Move the crate to `crates/promptforge-tool-picker/` at the workspace root. Reason: the root is the documented public layer (`crates/README.md`, `AGENTS.md`), and a private substrate crate there would be the first exception; it also removes nothing. Revisit: never, superseded by removal.
  - Move the crate to `../promptforge-design/crates/` as a compiling path dependency. Reason: it depends on `promptforge_api_types::tools::ToolId` and `shared_progress::ProgressHandle`, so the two repositories would depend on each other through path edges that `actions/checkout` breaks, the crate would leave the product-boundary matrix (`crates/build-xtask/src/product.rs` binds workspace members only), and the design repository has no CI. Revisit: never, superseded by removal.
  - Cut only the `model-client` edge and leave the crate consumed by the runtime alone. Reason: still ships the weights and the stack for a feature that is off. Revisit: if fuzzy slot resolution is wanted again, restore from the archive as a new design.
  - Shrink build cost through a hakari `[traversal-excludes]` entry instead of removal. Reason: keeps the crate and its build script; the floor question (Finding 2) is independent and stays deferred.
- Assumptions, risks, and notes:
  - `cargo-hakari` must be installed locally to regenerate `crates/workspace-hack/Cargo.toml`; `.config/hakari.toml` governs it.
  - `crates/promptforge-api-runtime/src/capabilities.rs` (line 66) imports `ToolId` through the picker's re-export (`crates/promptforge/tool-picker/src/catalog.rs` line 18, `pub use promptforge_api_types::tools::ToolId`); after removal that import comes from `promptforge_api_types::tools::ToolId` directly.
  - `half` appears in `crates/workspace-hack/Cargo.toml` with features that may be requested by another transitive puller; the pin is removed from the root manifest only if `cargo tree -i half` shows no remaining normal or build edge from a workspace member. The same check governs each of the seven pins.
  - The `HF_TOKEN` repository secret lives in GitHub settings, outside the repository; retiring it is a manual follow-up after CI stops referencing it.
  - The `../promptforge-design` worktree has five unrelated uncommitted paths; the archive commit stages only `study-tool-picker/crate/**` and `study-tool-picker/README.md`.
  - The archive lands in a different repository from the one the run operates on, so it is committed there directly by the session rather than through the per-step review cycle; the PromptForge-side deletion is what the run reviews.
  - Post-removal closure counts are projections until measured: roughly 51 fewer packages in the `promptforge-api-runtime`, `workshop`, `promptforge-lua`, `promptforge-parser`, and `promptforge-model-client` closures, low single digits for `gateway`, and about 67 MB of embedded weights out of the Workshop binary. The `dependency-surface.md` refresh records the measured numbers.
  - The `guide/promptforge-language-guide.md` assembled export is regenerated from `guide/src/` by the `build-user-guide` crate; edit sources, not the export.

### Deferred and Out of Scope

- Deferred: the `workspace-hack` floor (Finding 2 in `vibe/dependency-surface.md`); revisit after this removal changes the unified set.
- Deferred: `turso` at 59 exclusive crates (Finding 3); revisit when the product decides whether sync and replication are used.
- Out of scope: `../wg21-paperflow/crates/papergate/Cargo.toml` (already broken by the 2026-09-17 reorg); retiring the `HF_TOKEN` GitHub secret; the Workshop and gateway UI "model picker" and "quant picker" wording.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (default member; `default-members = ["crates/gateway/app"]`). Desktop app: `cargo build --locked -p workshop`. TypeScript UIs: `npm ci && npm run build` inside `crates/workshop/server/ui` and `crates/gateway/config-ui/ui`.
- Focused test command pattern: `cargo nextest run --locked -p <crate> <filter>`; single integration target `cargo test -p <crate> --test it <name>` (e.g. `cargo test -p gateway-stt --test it architecture`).
- Component test command pattern: `cargo nextest run --locked -p <crate>` for one crate; `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` for the workshop family; `cargo nextest run --locked -p workshop-server --features headless` for the headless shape; `cargo test -p build-xtask` for the boundary and structural harness; `npm run typecheck && npm test` inside a UI directory.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then the workshop family command above plus `cargo test --doc -p workshop -p workshop-server -p workshop-server-api`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`, then `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`, plus the feature-combination gate `cargo check -p gateway --no-default-features`. Never run standalone `cargo check --workspace`. Supply chain: `cargo deny check`, `cargo audit`.
- Formatter check command: `cargo fmt --all --check` (rustfmt `style_edition = "2024"`; also the pre-commit hook).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api` with `RUSTDOCFLAGS="-D warnings"`; user guide `mdbook build guide`.
- Test placement and naming conventions: integration tests live in `<crate>/tests/it/main.rs` with one module per concern (`tests/it/<topic>.rs`, nested `tests/it/<topic>/<case>.rs`), invoked as `--test it`; some crates use flat `tests/<name>.rs`. Unit tests are `#[cfg(test)]` modules (317 files) either inline or as kebab siblings `foo-tests.rs` wired via `#[path = "foo-tests.rs"]` (e.g. `tool-picker/src/picker-tests.rs`, `policy-tests.rs`); when three or more, they form a `src/<module>/tests/` directory (`mod.rs` plus per-topic files). Fixtures go in `tests/fixtures/`. Nextest config in `.config/nextest.toml` puts `promptforge-tool-picker`, `gateway-stt-backend-whisper`, and `gateway-stt` in a `heavy` test group (`threads-required = 4`, `max-threads = 8`). Clippy allows `unwrap`/`expect` in tests only. UI tests are `node --test` over `test/**/*.mjs` and `src/**/*.test.mjs`.
- Directory map: `Cargo.toml` (workspace, resolver 3, edition 2024, explicit member list including every family container crate, `exclude` for the manifestless containers and `crates/shared-ui`); `crates/` root layer: `promptforge-api-runtime`, `promptforge-api-types` (PromptForge public surface), `gateway-api`, `gateway-api-discovery` (Gateway public surface), `shared-vfs`, `shared-progress`, `shared-loopback`, `shared-ui` (TypeScript+CSS package, not a Rust crate), `workspace-hack` (cargo-hakari), `build-ui`, `build-workshop`, `build-xtask`, `build-user-guide`, `build-llama-cuda`; `crates/promptforge/` (private family: `lua`, `parser`, `store`, `vfs`, `model-client`, `tool-picker`, `web`, `webfetch`, `web-search`); `crates/gateway/` (private family: `app`, `cloud-providers`, `config`, `config-ui`, `local`, `logging`, `protocol`, `routing`, `web-search`, `stt/{api,engine,backend-whisper,whisper-ffi}`); `crates/workshop/` (private family: `shell` = package `workshop`, `server`, `server-api`, `gateway`, `menu`, `protocol`, `registry`, `sessions`, `status`, `support`, `user-state`, `workspace`); `guide/` (mdbook source and built book); `prompts/`, `local/` (local prompts, profiles, stores, STT fixtures); `tools/` (Node staging scripts and their `.test.mjs`); `vibe/` (archdoc plus dated plan history under `2026-07`, `2026-08`, `2026-09`); `.github/workflows/` (ci, nightly, release, guide, stt-miri, installer smoke); `.githooks/` (pre-commit fmt, pre-push headless check + clippy + deny); `.cargo/config.toml` (rust-lld on Windows, `cargo workshop` and `cargo xtask` aliases); `.config/nextest.toml`, `.config/hakari.toml`; `deny.toml`, `clippy.toml`, `rustfmt.toml`, `rust-toolchain.toml` (stable), `dist-workspace.toml`; `AGENTS.md`.
- Component boundaries: three products, PromptForge, Gateway, Workshop, each a private container of crates whose only public surface sits at the `crates/` root. Dependency direction: workshop-* -> {promptforge-api-runtime, promptforge-api-types, gateway-api, gateway-api-discovery, shared-*}; promptforge-api-runtime -> crates/promptforge/* (the one door into the family); crates/promptforge/* -> siblings and shared-*; crates/gateway/* -> siblings and shared-*; shared-* -> nothing product-level. Workshop must not depend on gateway family crates; gateway must not depend on promptforge or workshop crates; promptforge must not depend on gateway or workshop crates; the `workshop` shell depends on `workshop-server-api`, never `workshop-server`. Within workshop: shell -> features -> services -> vocabulary. `promptforge-tool-picker` is a crates/promptforge family crate consumed by `promptforge-api-runtime`; it is a workspace member, a `[workspace.dependencies]` entry, a nextest heavy-group filter entry, and has its own `build.rs`, `README.md`, and `tests/it/` suite. Rules bind normal, dev, build, and target-specific dependencies alike; `cargo test -p build-xtask` enforces the matrix.
- Conventions summary: Rust 2024 edition on stable, `#![forbid(unsafe_code)]` workspace-wide with `missing_docs`, `unreachable_pub`, clippy `all` + `pedantic` denied, `unwrap_used`/`expect_used` denied outside tests, rustdoc broken links denied. No file exceeds 500 lines (split first). Source dirs are flat: one or two child modules are kebab siblings `foo-bar.rs` with `#[path]`; three or more become a `foo/` directory. Every workshop-* `lib.rs` opens with a `//!` doc carrying `## Invariants`. Behavior changes ship with tests in the same change; structural checks require explicit user approval. Cargo features gate real constraints (toolchain, native build), not product shape; runtime and serve paths never compile native deps or exit the process. Long-running work reports through `shared-progress`. Comments explain non-obvious constraints and cite upstream issue URLs for workarounds. Error messages are concise and model-consumable (required versus actual). Dependency pins carry a comment explaining the version choice. `Cargo.lock` is committed and `--locked` is used everywhere. `workspace-hack` (hakari) is inherited by every member. SPA side: CSS beside its TypeScript, `--ws-*` tokens only, no `localStorage`, esbuild via `node build.mjs`, `tsc --noEmit` typecheck.

</project-survey>
<execution-plan>

## Execution Instructions

The objective is one removal: the tool picker leaves PromptForge, its source is frozen in the design repository, and the three features and the dependency stack that existed only for it leave with it. The archive already landed: `../promptforge-design/study-tool-picker/crate/` holds a verbatim copy of the 20 crate files and `../promptforge-design/study-tool-picker/README.md` names it, committed in that repository as `c973a42` (PromptForge `HEAD` at copy time was `bf8cb465`). The PromptForge work divides into three components, each one shippable slice, each one piece, each one step with one commit. The operator asked for few steps and one full verification at the end, so no component is divided further.

- Component order and reasons:
  - `rust-consumers` first. The five consuming crates stop naming the picker while the crate is still a workspace member, so the commit builds on its own and the crate becomes an orphan with no in-workspace edge.
  - `workspace` second. Deleting a crate that is still imported does not build, so this depends on `rust-consumers`. The crate deletion, its manifest entries, the seven third-party pins, hakari regeneration, the nextest filters, CI, and the crate READMEs land in one commit so `--locked` gates stay coherent at that commit.
  - `ui-docs` last. The SPA change is independent of the Rust work, but the `vibe/dependency-surface.md` measurements require the crate deletion and hakari regeneration to have landed, and the operator asked for a single full verification at the end.
- Steps 1 and 2 run their focused tests through the coding cycle and skip the standalone Verify dispatch. Step 3 runs `FULL` verification: the Testing Plan's gate list, the `cargo tree` invariants, and the reference sweep.

<step-1>

### Step 1: Remove the picker's consumers from the Rust crates [completed]

- Component: `rust-consumers`
- Parser (`crates/promptforge/parser`): `src/contract.rs` loses `FuzzySlot`, `ToolSlot::Fuzzy`, and `ToolSlotVisitor::visit_map`; `ToolSlot` keeps `Exact(ToolId)` and `#[non_exhaustive]`; `visit_str` is the only accepted form and the visitor's `expecting` text reads "an exact tool path string". `src/lib.rs` stops re-exporting `FuzzySlot`. The doc lines in `src/build.rs` (near 81 and 224) describe exact slots only. `src/contract/tests.rs` loses the two fuzzy-slot tests and gains one test asserting that a `tools:` map value (`{ want: "...", optional: true }`) fails to parse with a message naming the exact-path expectation.
- Lua (`crates/promptforge/lua`): `src/handles.rs` loses `Conflict`, the `conflicts` field on `ToolBinding`, its `PartialEq` and `Debug` participation, and `conflicts()`; `src/lib.rs` (near 132) stops exporting `Conflict`; `src/error.rs` loses the `ModelBind`, `ModelBindQuery`, `ModelAbsent`, `ModelDuplicate`, and `ModelAmbiguous` variants and their kind and mapping arms.
- Model client (`crates/promptforge/model-client`): `src/model.rs` loses `mod resolver`, `pub use resolver::PickerModelResolver`, `picker_catalog_from`, `model_to_picker_id`, `model_from_picker_id`, `escape_segment`, `unescape_segment`, `PICKER_MODEL_LABEL`, `ModelCatalogFiltered`, `ModelResolver`, `ResolvedModel`, `satisfies_constraints`, and the `promptforge_tool_picker` import; `src/model/resolver.rs` is deleted; `src/model/options.rs` loses `ModelBindOpts` and `impl From<&ModelBindOpts> for ModelInvocation` and their re-export; `src/error.rs` loses the five `ModelBind*` variants and their kind and mapping tables; `src/model/tests.rs` loses the two picker-id round-trip tests; `Cargo.toml` loses `promptforge-tool-picker.workspace = true`; `README.md` loses any resolver mention.
- Runtime (`crates/promptforge-api-runtime`): `src/capabilities.rs` loses the `lint: ToolPicker` field, `LINT_KEY_SEGMENT`, the post-insert lint, and the picker import, and imports `ToolId` from `promptforge_api_types::tools::ToolId`; `src/capabilities-tests.rs` loses the lint tests. `src/execute/fill.rs`: `fill_tool_bindings` loses its `picker` parameter and `ToolSlot::Fuzzy` arm; `fuzzy_fill_picker`, `fill_fuzzy_slot`, and `record_near_duplicate_conflicts` are deleted with their imports. `src/execute/environment.rs`: `Environment` loses the `picker` field, the `picker()` builder, the `Debug` field, the import, and the picker and fuzzy doc sentences; `prepare` passes no picker. `src/execute/bindings.rs`: `ToolBindings` loses `conflicts`, `record_conflict`, `conflicts()`, the `Conflict` use, `bound_ids` when no caller remains, and the symmetric-conflict test. `src/execute/scope.rs` loses the conflict check over in-scope bindings. `src/tools.rs` loses `NearDuplicateDiagnostic`; `src/lib.rs` (near 91) stops re-exporting it. `src/error.rs` and `src/execute/error.rs` lose `Error::NearDuplicateTools` and the `ModelBind*` variants with their mapping arms (near 290-320, 579-600, 680-700) and the test near 847. Doc mentions at `src/lib.rs` 46 and 59, `src/execute.rs` 35, and `README.md` 22 are corrected. `benches/models_loop.rs` and the `[[bench]]` block in `Cargo.toml` are deleted and `criterion` leaves this crate's dev-dependencies; both `promptforge-tool-picker` lines (normal and `test-fixtures` dev) leave `Cargo.toml`. Tests pruned in `src/execute/tests/{mod,exec_flow,live_infer,tool_scoping,observations}.rs` and `tests/suite/{prepare,support}.rs`: `picker_model`, `shared_test_model`, `filled_slots_record_near_duplicate_conflicts_symmetrically`, the near-duplicate parameter of `bound_with_tools`, and the three conflict tests near `tool_scoping.rs` 198-290. Remaining exact-slot fill tests keep passing unchanged.
- API types (`crates/promptforge-api-types/src/capabilities.rs`, near 290): the doc sentence about the registration-time near-duplicate lint is removed.
- Workshop workspace (`crates/workshop/workspace`): `src/handlers-prompts.rs` loses the `Fuzzy` variant of the contract tool enum and its match arm so the prompt-contract JSON emits kind `exact` only; `src/handlers-prompts-tests.rs` loses the two fuzzy fixtures (near 104 and 169-175) and keeps the exact-kind assertions.
- Manifests: `Cargo.lock` is updated (the runtime and model-client dependency lists shrink) and committed. `promptforge-tool-picker` remains a workspace member and a `[workspace.dependencies]` entry until Step 2. Run `cargo hakari verify`; if it reports drift, run `cargo hakari generate` and commit `crates/workspace-hack/Cargo.toml` in the same commit.
- Tests: `cargo nextest run --locked -p promptforge-parser -p promptforge-lua -p promptforge-model-client -p promptforge-api-runtime -p workshop-workspace`; `cargo test --doc -p promptforge-api-runtime -p promptforge-model-client`; `cargo clippy -p promptforge-parser -p promptforge-lua -p promptforge-model-client -p promptforge-api-runtime -p workshop-workspace --all-targets --all-features -- -D warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps -p promptforge-api-runtime -p promptforge-model-client -p promptforge-lua -p promptforge-parser`; `rg -n "tool_picker|ToolPicker|FuzzySlot|NearDuplicate|PickerModelResolver|ModelBindOpts" crates --glob '!crates/promptforge/tool-picker/**' --glob '!*.md'` returns nothing.
- Commit: one commit in PromptForge containing the code, the manifest edits, `Cargo.lock`, and the tests.

</step-1>

<step-2>

### Step 2: Delete the crate and prune the workspace, nextest, CI, and crate READMEs [completed]

- Component: `workspace`
- Crate: `git rm -r crates/promptforge/tool-picker` (the 20 files already archived in `../promptforge-design` at `c973a42`). This removes the only `build.rs` in the runtime and Workshop closure that reaches the network, and the `include_bytes!` of the fp16 `BAAI/bge-small-en-v1.5` weights.
- Root `Cargo.toml`: remove `"crates/promptforge/tool-picker"` from `members`; remove `promptforge-tool-picker` from `[workspace.dependencies]`; then, for each of `candle-core`, `candle-nn`, `candle-transformers`, `tokenizers`, `hf-hub`, `safetensors`, and `half`, run `cargo tree --workspace -e normal,build --target all -i <crate>` and remove the pin (with the `tokenizers` unification comment) only when the command prints nothing. A pin that still has a puller stays and is named in the step's return.
- Hakari: `cargo hakari generate`, then `cargo hakari manage-deps`, then `cargo hakari verify` clean; `crates/workspace-hack/Cargo.toml` and `Cargo.lock` are committed.
- Nextest: `.config/nextest.toml` loses `package(promptforge-tool-picker)` from both `heavy` group filters (lines 23 and 28); the `heavy` group itself stays for `gateway-stt-backend-whisper` and `gateway-stt`.
- CI: delete `.github/actions/hf-model-cache/action.yml` and its directory; remove every `uses: ./.github/actions/hf-model-cache` step with its `name: Cache the embedding model` line (`ci.yml` five, `nightly.yml` three, `release-workshop.yml` one, `workshop-installer-smoke.yml` one); remove the `HF_TOKEN: ${{ secrets.HF_TOKEN }}` env entries in `ci.yml`, `nightly.yml`, and `workshop-installer-smoke.yml`; remove or rewrite the explanatory comments at `ci.yml` 12-18, `nightly.yml` 20-22, and `release-workshop.yml` 125-126 so none mentions the picker or the model cache.
- Crate READMEs: `crates/README.md` (the `promptforge-api-runtime` paragraph no longer lists `tool-picker`); `crates/promptforge/README.md` (the `promptforge-tool-picker` section is removed and the `promptforge-model-client` paragraph no longer names it); `AGENTS.md` if it names the crate or the model cache.
- Tests: `cargo tree --workspace -e normal,build --target all -i candle-core` prints nothing, and likewise for `tokenizers`, `hf-hub`, and `safetensors`; `cargo hakari verify` clean; `cargo test -p build-xtask` passes with the shrunken member list; `cargo check -p gateway --no-default-features`; `cargo deny check` clean; `cargo nextest run --locked -p promptforge-api-runtime` (proves the `--locked` lock is coherent); `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache|HF_TOKEN" crates .github .config Cargo.toml AGENTS.md --glob '!crates/gateway/**'` returns nothing and `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache" crates/gateway` returns nothing (`guide/src` is Step 3; the gateway's own `HF_TOKEN` reads stay).
- Commit: one commit in PromptForge containing the deletion, both manifests, `Cargo.lock`, `crates/workspace-hack/Cargo.toml`, `.config/nextest.toml`, the four workflows, the deleted action, and the READMEs.

</step-2>

<step-3>

### Step 3: Collapse the Workshop SPA to exact tools, refresh the guide and dependency surface, and verify

- Component: `ui-docs`
- Workshop SPA (`crates/workshop/server/ui`): `src/services/run-api.ts` loses `RunContractToolFuzzy` and the `kind === "fuzzy"` parse branch and collapses `RunContractTool` to the exact shape; `src/ui/run/run-rows.ts` (near 146-149) renders exact rows only; `test/run-panel.mjs` loses the fuzzy fixture (near 114) and the "exact and fuzzy tools render" check (near 359), keeping an exact-only render assertion.
- Guide: edit the sources `guide/src/language/01-frontmatter-and-structure.md` (line 38), `guide/src/language/02-the-run.md` (line 7), and `guide/src/language/07-tools.md` (lines 31, 97, 124) so `tools:` is documented as alias-to-exact-path strings only, then regenerate `guide/promptforge-language-guide.md` with the `build-user-guide` crate (never edit the export by hand). The `guide/src/workshop/*` "picker" hits are the UI model picker and stay.
- Other READMEs: `crates/workshop/server/README.md` only if its one "picker" mention is the tool picker rather than the model picker.
- `vibe/dependency-surface.md`: remove the candle row from the ranked table and the picker note in measurement trap 4; remove the `base64 0.13` (via `spm_precompiled`) and `rand 0.8` (via candle) entries from the duplicate-versions list when `cargo tree -i` shows they left; add a row to "What was replaced, and why" (removed: `promptforge-tool-picker` and its candle, tokenizers, hf-hub, safetensors stack; replacement: none, exact slots only; evidence: the empty `cargo tree --workspace -e normal,build --target all -i candle-core` output); rerun the shipped-shape closure counts for `promptforge-api-runtime`, `workshop`, `promptforge-lua`, `promptforge-parser`, `promptforge-model-client`, and `gateway` per the file's "Reproducing the measurements" section, replacing the projected figures (about 51 packages and about 67 MB of embedded weights) with measured ones; update the header commit hash and date.
- Tests: `npm run typecheck && npm test` in `crates/workshop/server/ui`; the reference sweep `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache|HF_TOKEN" crates .github .config guide/src Cargo.toml AGENTS.md --glob '!crates/gateway/**'` returns nothing and `rg -n "tool-picker|tool_picker|ToolPicker|FuzzySlot|NearDuplicate|hf-model-cache" crates/gateway` returns nothing (history under `vibe/` exempt; the gateway's own `HF_TOKEN` reads stay); `../promptforge-design/study-tool-picker/crate/` still holds 20 files and `../promptforge-design/study-tool-picker/README.md` names it.
- Verification (`FULL`, the only standalone Verify dispatch in this plan), all of which must pass:
  - `cargo fmt --all --check`
  - `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings`
  - `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`
  - `cargo check -p gateway --no-default-features`
  - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`
  - `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`
  - `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api` (includes `a_chat_session_activates_the_web_capability_and_calls_search_end_to_end` in `crates/workshop/sessions/src/agents/environment.rs`, unchanged)
  - `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`
  - `cargo test -p build-xtask`
  - `cargo deny check`
  - `cargo hakari verify`
  - `cargo tree --workspace -e normal,build --target all -i <crate>` prints nothing for `candle-core`, `tokenizers`, `hf-hub`, and `safetensors`
  - `npm test` in `crates/workshop/server/ui`
- Commit: one commit in PromptForge containing the SPA change and its test, the guide sources and regenerated export, the README edit if any, and `vibe/dependency-surface.md`.

</step-3>

</execution-plan>
