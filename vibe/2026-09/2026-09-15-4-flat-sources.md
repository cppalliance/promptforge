---
name: Flat Sources Rule
overview: Add a flat-sources rule to promptforge/AGENTS.md (source subdirs need 3+ files, with defined flatten/rehydrate conversions in both directions), then flatten all 19 non-compliant crates workspace-wide.
todos:
  - id: create-rule
    content: Add flat-sources bullet to Structural Rules in promptforge/AGENTS.md
    status: pending
  - id: flatten-crates
    content: Flatten all 19 non-compliant crates per the audit table and mechanics spec
    status: pending
  - id: verify
    content: Run the full workspace verification gate
    status: pending
isProject: false
---

# Flat Sources Rule

<product-contract>

## Product Requirements

The promptforge workspace has accumulated source subdirectories holding only one or two files each, which the repository owner wants eliminated and prevented from recurring. The work is a written convention in promptforge/AGENTS.md plus a one-time mechanical flattening of every violating directory in the workspace.

- Problem and users: 45 source subdirectories across 19 of 43 crates hold only 1-2 files each (full scan of promptforge/crates/, excluding target/, taken 2026-09-15). The repository owner is the user; the affected users are every contributor and agent navigating the tree.
- Goals: a durable written rule covering both directions of conversion, and a workspace tree that already complies with it when the work is done.
- Non-goals: rehydrating anything (zero dash-named .rs files exist in promptforge/crates/, so no flat group qualifies); changing top-level tests/ or benches/ trees; changing any module name, public API, or behavior; adding structural enforcement tooling.
- Success criteria: the rule text is present under ## Structural Rules in promptforge/AGENTS.md; no src/ subdirectory in any crate holds fewer than three .rs files; the full verification gate passes with zero test modifications.
- Constraints: file moves use git mv to preserve history; module names and call sites stay unchanged via #[path] attributes; repository policy (promptforge/AGENTS.md, ## Engineering) forbids introducing structural enforcement without explicit user approval, so the rule is documentation only.
- Open questions: None

## Functional Specification

The convention governs how contributors and coding agents lay out Rust modules. A module group smaller than three files lives as flat kebab-labeled siblings; a group of three or more may occupy a subdirectory; touching a group on the wrong side of the line triggers conversion.

- Actors and workflows: contributors and coding agents adding or editing Rust modules in promptforge. When creating, growing, shrinking, or touching a module group, they place it in the valid form for its size.
- Inputs and outputs: input is the existing source tree plus any new module work; output is a tree where every src/ subdirectory holds at least three .rs files and every smaller group is a set of dash-labeled sibling files.
- States and validation: a module group is either flat (`foo.rs` beside `foo-bar.rs`, wired with `#[path = "foo-bar.rs"] mod bar;`) or rehydrated (`foo.rs` beside `foo/bar.rs`, standard layout, no path attributes). Validity is determined by direct .rs file count, with three as the line in both directions.
- Errors and recovery: name collisions with existing siblings are checked before each move; a miswired #[path] surfaces as a cargo check failure for that crate and is corrected before moving on.
- Security and privacy behavior: None
- Acceptance criteria: every row of the audit table in Technical Design is applied; the verification gate in Testing Plan is green.

</product-contract>
<implementation-contract>

## Technical Design

This is a file-layout convention change with no runtime architecture impact. All design content is the rule text, the move mechanics, and the per-crate audit of required moves.

- Architecture: no change. Module names, visibility, and call sites are preserved exactly; only file paths change.
- Modules and interfaces: no public interface changes. Each flattened submodule keeps its module name through an explicit path attribute on its declaration in the parent module, e.g. `#[path = "anthropic-taxonomy.rs"] pub(crate) mod taxonomy;` in promptforge/crates/shared-cloud-providers/src/providers/anthropic.rs:16. A #[path] attribute in a file module resolves relative to that file's own directory, so sibling filenames work without call-site changes.
- File and public API changes:
  - promptforge/AGENTS.md gains one bullet under ## Structural Rules, verbatim: "Source directories are flat by default. A subdirectory of source files must contain at least three files; one or two files belong beside the parent module as `foo-bar.rs` (parent stem, dash, kebab label), wired with an explicit path attribute so the module name stays clean: `#[path = "foo-bar.rs"] mod bar;`. The two forms are convertible in both directions: when a `foo-*.rs` sibling group grows to three files, rehydrate it into a `foo/` subdirectory in standard module layout (`foo/bar.rs` beside `foo.rs`) and drop the path attributes; when a subdirectory shrinks below three files, flatten it back to kebab siblings. Apply whichever conversion applies when you touch files in a group on the wrong side of the line. Top-level `tests/` and `benches/` trees are exempt; they follow Cargo target conventions."
  - Move mechanics for every row below: git mv each file; parent stem stays verbatim even when snake_case (`boot_load-tests.rs`); only the appended label is kebab (`logging_tests` becomes `logging-tests`); a mod.rs in a flattened directory promotes to the parent-stem file (`compactors/mod.rs` becomes `compactors.rs`); nested violating directories flatten recursively with chained labels (`take/agreement/final_overlap/tests.rs` becomes `take/agreement-final-overlap-tests.rs`); emptied directories are removed; collisions with existing siblings are checked before each move.
  - Audit of all 43 crates (source: directory scan of promptforge/crates/ excluding target/, 2026-09-15). Compliant, untouched (24): build-llama-cuda, build-ui, build-user-guide, build-workshop, build-xtask, gateway-config-ui, gateway-logging, gateway-protocol, gateway-stt-backend-whisper, gateway-web-search, gateway-whisper-ffi, promptforge-model-client, promptforge-parser, promptforge-vfs, promptforge-web, promptforge-webfetch, shared-gateway-api, shared-loopback, shared-progress, shared-sidecar, shared-vfs, workshop, workshop-protocol, workshop-support.
  - Non-compliant (19), with directories to flatten and resulting filenames:

| Crate | Dirs to flatten | Resulting files |
|---|---|---|
| shared-cloud-providers | providers/anthropic (1), providers/bedrock (2), providers/foundry (1), providers/mistral (1), providers/openrouter (1) | anthropic-taxonomy.rs, bedrock-sigv4.rs, bedrock-taxonomy.rs, foundry-taxonomy.rs, mistral-taxonomy.rs, openrouter-taxonomy.rs in src/providers/ |
| gateway | src/boot_load (tests.rs), src/main (logging_tests.rs) | boot_load-tests.rs, main-logging-tests.rs |
| gateway-config | src/profile (name.rs), src/shadow (content.rs, tests.rs) | profile-name.rs, shadow-content.rs, shadow-tests.rs |
| gateway-local | src/gguf (tests.rs), src/launch_templates (tests.rs), src/server (support.rs, tests.rs), src/artifacts/staging (tests.rs) | gguf-tests.rs, launch_templates-tests.rs, server-support.rs, server-tests.rs, artifacts/staging-tests.rs |
| gateway-routing | src/queue (tests.rs) | queue-tests.rs |
| gateway-stt | src/batch (2), src/generation (lease.rs, snapshot.rs), src/segment (boundary.rs), src/realtime/session/items (1), src/realtime/session/route (1), src/realtime/wire/server (events.rs), src/take/agreement (2 plus nested final_overlap/tests.rs), src/take/pcm (1), src/take/state (2 plus nested alignment_tests/adversaries.rs and tests/live_prefix.rs), src/take/window/tests (live_prefix.rs) | batch-tests.rs, batch-native-tests.rs, generation-lease.rs, generation-snapshot.rs, segment-boundary.rs, realtime/session/items-tests.rs, realtime/session/route-tests.rs, realtime/wire/server-events.rs, take/agreement-final-overlap.rs, take/agreement-projection.rs, take/agreement-final-overlap-tests.rs, take/pcm-tests.rs, take/state-alignment-tests.rs, take/state-tests.rs, take/state-alignment-tests-adversaries.rs, take/state-tests-live-prefix.rs, take/window-tests-live-prefix.rs |
| gateway-stt-engine | src/test_fixtures/tests (scenario_cleanup.rs plus nested scenario_cleanup/ with construction.rs, decode.rs) | test_fixtures/tests-scenario-cleanup.rs, tests-scenario-cleanup-construction.rs, tests-scenario-cleanup-decode.rs |
| promptforge-api | src/capabilities (tests.rs), src/lua (coro_tests.rs), src/tools (tests.rs) | capabilities-tests.rs, lua-coro-tests.rs, tools-tests.rs |
| promptforge-lua | src/compactors, src/messages, src/projection (mod.rs plus tests.rs each) | compactors.rs, compactors-tests.rs, messages.rs, messages-tests.rs, projection.rs, projection-tests.rs |
| promptforge-tool-picker | src/picker (tests.rs), src/policy (tests.rs) | picker-tests.rs, policy-tests.rs |
| promptforge-web-search | src/web_search (tests.rs) | web_search-tests.rs |
| shared-promptforge-api | src/capabilities (tests.rs), src/names (tests.rs), src/untrusted (inventory.rs) | capabilities-tests.rs, names-tests.rs, untrusted-inventory.rs |
| workshop-gateway | src/gateway_progress (tests.rs plus nested tests/ with lifecycle.rs, recovery.rs), src/heartbeat (refresh.rs, tests.rs), src/observer (tests.rs), src/resolve (tests.rs), src/test_gateway (process.rs) | gateway_progress-tests.rs, gateway_progress-tests-lifecycle.rs, gateway_progress-tests-recovery.rs, heartbeat-refresh.rs, heartbeat-tests.rs, observer-tests.rs, resolve-tests.rs, test_gateway-process.rs |
| workshop-menu | src/catalog (chat.rs, tests.rs), src/menu (memory.rs, tests.rs plus nested tests/memory.rs) | catalog-chat.rs, catalog-tests.rs, menu-memory.rs, menu-tests.rs, menu-tests-memory.rs |
| workshop-registry | src/push (tests.rs) | push-tests.rs |
| workshop-server | src/app (fixtures.rs, tests.rs), src/serve (tests.rs), src/routes/assets (headless_tests.rs, tests.rs), src/routes/gateway_config (tests.rs plus nested tests/recovery.rs) | app-fixtures.rs, app-tests.rs, serve-tests.rs, routes/assets-headless-tests.rs, routes/assets-tests.rs, routes/gateway_config-tests.rs, routes/gateway_config-tests-recovery.rs |
| workshop-sessions | src/input (tests.rs, tool.rs), src/relay (tests.rs), src/session (log.rs, menu.rs), src/agents/supervisor/transition (tests.rs) | input-tests.rs, input-tool.rs, relay-tests.rs, session-log.rs, session-menu.rs, agents/supervisor/transition-tests.rs |
| workshop-status | src/progress (tests.rs) | progress-tests.rs |
| workshop-workspace | src/error (tests.rs), src/handlers (tests.rs), src/workspace (tests.rs) | error-tests.rs, handlers-tests.rs, workspace-tests.rs |

- Data, persistence, failure, security, and privacy constraints: None

</implementation-contract>
<verification-contract>

## Testing Plan

Every move is behavior-preserving, so the entire existing suite must pass unmodified. The gate is the workspace's documented verification set (command list source: promptforge/AGENTS.md, ## Verification).

- Unit: all existing unit tests, including the moved tests.rs and *_tests.rs files, pass unmodified under their unchanged module names.
- Integration and end-to-end: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`; doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates via `cargo nextest run --locked -p workshop -p workshop-server`.
- Regression, security, and performance: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with RUSTDOCFLAGS="-D warnings"; structural harness `cargo test -p build-xtask`.
- Exit criteria: every command above exits zero with no test content modified; each crate additionally passes `cargo check -p <crate> --all-features` immediately after its own moves.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Rule lives in promptforge/AGENTS.md rather than a Cursor rule file. Rationale: the user stated the .mdc form does not work for this purpose. User's words: "it cant be .mdc that wont work. use AGENTS.md".
  - Three-file threshold in both directions. Rationale: some crates legitimately need subdirectories, so an absolute ban was wrong; the user picked the hard count over a judgment test. User's words: "some of them need subdirs", then selecting "Subdir needs 3+ files to exist; 1-2 files must flatten to kebab siblings".
  - The rule is bidirectional with a defined rehydration path. Rationale: a growing flat group needs a defined route back to a directory. User's words: "but there also needs a way to take the flattened files and rehydrate".
  - Flat naming is parent stem, dash, kebab label. User's words: "use the root filename and put a dash, and then give it a, a, a subname. Give it a kebab label."
  - Fix all 19 non-compliant crates in this change rather than only shared-cloud-providers. User selected "Fix all 19 crates now (~60 file moves, one big mechanical pass)" after reviewing the full audit.
  - The rule covers test-only subdirectories inside src/ but exempts top-level tests/ and benches/ trees. User selected "Applies to all of src/ including test-only subdirs; top-level tests/ and benches/ are exempt (Cargo target conventions)".
- Rejected alternatives:
  - A .mdc rule file under .cursor/rules/: rejected by the user; no revisit condition stated.
  - An absolute ban on source subdirectories: rejected because some crates need them; revisit only if the 3-file threshold proves unworkable in practice.
  - A 2-file threshold and a judgment-based "genuine subsystem" test: not selected; revisit if the hard count produces disputes.
  - Flattening shared-cloud-providers only and migrating the rest on touch: superseded by the fix-everything-now decision.
- Assumptions, risks, and notes:
  - The audit reflects the tree as of 2026-09-15; any drift since requires rechecking the affected crate before its moves.
  - #[path] resolution relative to the declaring file's directory is assumed; cargo check per crate is the backstop if a nested case resolves differently.
  - Most violating directories hold only split-out test files (tests.rs or *_tests.rs), so most moves touch test code, not product code.
  - Crate-root src/ directories holding a single lib.rs or main.rs involve no subdirectory and are not violations.
  - Commit decomposition and ordering are left to execution.

### Deferred and Out of Scope

- Deferred: rehydration of flat groups into directories. No group qualifies today; revisit whenever a foo-*.rs sibling group reaches three files.
- Out of scope: top-level tests/ and benches/ trees; the 24 compliant crates; any structural enforcement harness; changes to module names, visibility, or behavior.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (default member is `gateway` only; desktop app is explicit: `cargo build -p workshop`; UI bundles build through crate build scripts after `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui`)
- Focused test command pattern: `cargo nextest run -p <crate> <test-name-substring>` (CI uses `cargo test --locked -p <crate> <full-test-name>` for named race tests)
- Component test command pattern: `cargo nextest run -p <crate>`; gateway integration target: `cargo nextest run -p gateway --test it` (some scenarios need `--no-default-features --features test-fixtures`); workshop-server headless: `cargo nextest run --locked -p workshop-server --features headless`
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server` plus `cargo test --doc -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`)
- Formatter check command: `cargo fmt --all --check`
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`
- Test placement and naming conventions: unit tests live in `src/tests.rs` pulled in by `#[cfg(test)] mod tests;` at the foot of `lib.rs`, with shared fixtures in `src/test_support.rs`; integration tests are a directory target `tests/it/main.rs` with per-area modules (gateway, gateway-stt) or single-file `tests/<name>.rs`; test names are sentence-style snake_case (e.g. `a_process_lifetime_lease_recovers_after_its_owner_is_terminated`); structural/boundary harness runs via `cargo test -p build-xtask`
- Directory map: `crates/` holds all 44 workspace members (`members = ["crates/*"]`, `crates/shared-ui` excluded as a TypeScript+CSS package); `guide/` is the mdbook user guide; `prompts/` holds example PromptForge pipeline files; `tools/` holds Node helper scripts (sidecar staging, TTS live checks); `local/` holds local run configuration and fixtures; `vibe/` holds plan files and `archdoc.md`; `.config/nextest.toml` configures nextest; `.github/workflows/` holds CI (`ci.yml` plus release and nightly workflows); `target/` and `target-msrv/` are build outputs
- Component boundaries: three products - PromptForge runtime (`promptforge-*` crates, executor + parser + Lua VM boundary + store/VFS), Gateway (`gateway-*` crates, independent inference server), Workshop (`workshop-*` crates, Tauri desktop + in-process server) - over `shared-*` substrate crates and `build-*` build tooling; dependency rules: workshop never depends on gateway, gateway never on promptforge/workshop, promptforge never on gateway/workshop, shared never on products, and the one-door rule: outside crates may depend only on `promptforge-api`, never on internal `promptforge-*` substrate crates; SPA dependency direction is shell -> features -> services -> vocabulary
- Conventions summary: Rust edition 2024 with workspace lints (`unsafe_code = "forbid"`, clippy `all` deny, `unwrap_used`/`expect_used` deny, rustdoc link lints deny); no file exceeds 500 lines (enforced by `build-xtask`); every `workshop-*` lib.rs opens with a `## Invariants` doc marker; behavior changes ship with tests in the same change; error messages are written for model consumption (concise, factual, required-vs-actual); Cargo features gate real constraints, not product shape; CSS lives beside its TypeScript using `--ws-*` tokens, never raw values; comments cite upstream issue URLs for workarounds; CI requires a clean git tree after builds

</project-survey>
<execution-plan>

## Execution Instructions

Components in dependency order, with placement reasons:

1. `rule` - the flat-sources convention text in promptforge/AGENTS.md. Placed first because it defines the rule the flattening implements; documentation-only with no code dependency.
2. `shared-crates` - shared-cloud-providers, shared-promptforge-api. Substrate layer per archdoc; flattened before product crates so later per-crate checks run against final paths.
3. `gateway-crates` - gateway, gateway-config, gateway-local, gateway-routing, gateway-stt, gateway-stt-engine. Independent of the promptforge and workshop groups; ordered here by archdoc layering.
4. `promptforge-crates` - promptforge-api, promptforge-lua, promptforge-tool-picker, promptforge-web-search. Independent of the gateway and workshop groups.
5. `workshop-crates` - workshop-gateway, workshop-menu, workshop-registry, workshop-server, workshop-sessions, workshop-status, workshop-workspace. Independent of the gateway and promptforge groups.
6. `verification` - the full Testing Plan gate. Placed last because it depends on every move.

Within each crate component, each crate is one piece built as one step. The crates are mutually independent (module names and call sites are unchanged), so the pieces have no inter-dependencies; they are sequenced one per step only to keep each commit crate-scoped and reviewable.

Every crate step applies the move mechanics in Technical Design: git mv each file; parent stem stays verbatim even when snake_case; only the appended label is kebab; a mod.rs promotes to the parent-stem file; nested violating directories flatten recursively with chained labels; check collisions with existing siblings before each move; remove emptied directories. Rewire each parent module declaration with an explicit #[path] attribute in the same pass as its moves (module names and call sites stay unchanged), then run `cargo check -p <crate> --all-features`. Each step lands as one commit containing its moves and rewiring.

<step-1>

### Step 1: Add flat-sources rule to AGENTS.md [completed]

- Component: rule
- Add the rule bullet verbatim (full text in Technical Design, File and public API changes) under ## Structural Rules in promptforge/AGENTS.md.
- Documentation only; no tests. One commit.

</step-1>

<step-2>

### Step 2: Flatten shared-cloud-providers [completed]

- Component: shared-crates
- Apply the shared-cloud-providers row of the audit table in Technical Design. Dirs to flatten: providers/anthropic (1), providers/bedrock (2), providers/foundry (1), providers/mistral (1), providers/openrouter (1). Resulting files in src/providers/: anthropic-taxonomy.rs, bedrock-sigv4.rs, bedrock-taxonomy.rs, foundry-taxonomy.rs, mistral-taxonomy.rs, openrouter-taxonomy.rs.
- Verify: `cargo check -p shared-cloud-providers --all-features`.

</step-2>

<step-3>

### Step 3: Flatten shared-promptforge-api [completed]

- Component: shared-crates
- Apply the shared-promptforge-api row of the audit table. Dirs to flatten: src/capabilities (tests.rs), src/names (tests.rs), src/untrusted (inventory.rs). Resulting files: capabilities-tests.rs, names-tests.rs, untrusted-inventory.rs.
- Verify: `cargo check -p shared-promptforge-api --all-features`.

</step-3>

<step-4>

### Step 4: Flatten gateway [completed]

- Component: gateway-crates
- Apply the gateway row of the audit table. Dirs to flatten: src/boot_load (tests.rs), src/main (logging_tests.rs). Resulting files: boot_load-tests.rs, main-logging-tests.rs.
- Verify: `cargo check -p gateway --all-features`.

</step-4>

<step-5>

### Step 5: Flatten gateway-config [completed]

- Component: gateway-crates
- Apply the gateway-config row of the audit table. Dirs to flatten: src/profile (name.rs), src/shadow (content.rs, tests.rs). Resulting files: profile-name.rs, shadow-content.rs, shadow-tests.rs.
- Verify: `cargo check -p gateway-config --all-features`.

</step-5>

<step-6>

### Step 6: Flatten gateway-local [completed]

- Component: gateway-crates
- Apply the gateway-local row of the audit table. Dirs to flatten: src/gguf (tests.rs), src/launch_templates (tests.rs), src/server (support.rs, tests.rs), src/artifacts/staging (tests.rs). Resulting files: gguf-tests.rs, launch_templates-tests.rs, server-support.rs, server-tests.rs, artifacts/staging-tests.rs.
- Verify: `cargo check -p gateway-local --all-features`.

</step-6>

<step-7>

### Step 7: Flatten gateway-routing [completed]

- Component: gateway-crates
- Apply the gateway-routing row of the audit table. Dirs to flatten: src/queue (tests.rs). Resulting files: queue-tests.rs.
- Verify: `cargo check -p gateway-routing --all-features`.

</step-7>

<step-8>

### Step 8: Flatten gateway-stt [completed]

- Component: gateway-crates
- Apply the gateway-stt row of the audit table, including recursive nested flattening with chained labels: take/agreement/final_overlap/tests.rs becomes take/agreement-final-overlap-tests.rs; take/state nested alignment_tests/adversaries.rs and tests/live_prefix.rs become take/state-alignment-tests-adversaries.rs and take/state-tests-live-prefix.rs; take/window/tests/live_prefix.rs becomes take/window-tests-live-prefix.rs. Resulting files (17): batch-tests.rs, batch-native-tests.rs, generation-lease.rs, generation-snapshot.rs, segment-boundary.rs, realtime/session/items-tests.rs, realtime/session/route-tests.rs, realtime/wire/server-events.rs, take/agreement-final-overlap.rs, take/agreement-projection.rs, take/agreement-final-overlap-tests.rs, take/pcm-tests.rs, take/state-alignment-tests.rs, take/state-tests.rs, take/state-alignment-tests-adversaries.rs, take/state-tests-live-prefix.rs, take/window-tests-live-prefix.rs.
- Verify: `cargo check -p gateway-stt --all-features`.

</step-8>

<step-9>

### Step 9: Flatten gateway-stt-engine [completed]

- Component: gateway-crates
- Apply the gateway-stt-engine row of the audit table. Dirs to flatten: src/test_fixtures/tests (scenario_cleanup.rs plus nested scenario_cleanup/ with construction.rs, decode.rs). Resulting files: test_fixtures/tests-scenario-cleanup.rs, tests-scenario-cleanup-construction.rs, tests-scenario-cleanup-decode.rs.
- Verify: `cargo check -p gateway-stt-engine --all-features`.

</step-9>

<step-10>

### Step 10: Flatten promptforge-api [completed]

- Component: promptforge-crates
- Apply the promptforge-api row of the audit table. Dirs to flatten: src/capabilities (tests.rs), src/lua (coro_tests.rs), src/tools (tests.rs). Resulting files: capabilities-tests.rs, lua-coro-tests.rs, tools-tests.rs.
- Verify: `cargo check -p promptforge-api --all-features`.

</step-10>

<step-11>

### Step 11: Flatten promptforge-lua [completed]

- Component: promptforge-crates
- Apply the promptforge-lua row of the audit table. Dirs to flatten: src/compactors, src/messages, src/projection (mod.rs plus tests.rs each; each mod.rs promotes to the parent-stem file). Resulting files: compactors.rs, compactors-tests.rs, messages.rs, messages-tests.rs, projection.rs, projection-tests.rs.
- Verify: `cargo check -p promptforge-lua --all-features`.

</step-11>

<step-12>

### Step 12: Flatten promptforge-tool-picker [completed]

- Component: promptforge-crates
- Apply the promptforge-tool-picker row of the audit table. Dirs to flatten: src/picker (tests.rs), src/policy (tests.rs). Resulting files: picker-tests.rs, policy-tests.rs.
- Verify: `cargo check -p promptforge-tool-picker --all-features`.

</step-12>

<step-13>

### Step 13: Flatten promptforge-web-search [completed]

- Component: promptforge-crates
- Apply the promptforge-web-search row of the audit table. Dirs to flatten: src/web_search (tests.rs). Resulting files: web_search-tests.rs.
- Verify: `cargo check -p promptforge-web-search --all-features`.

</step-13>

<step-14>

### Step 14: Flatten workshop-gateway [completed]

- Component: workshop-crates
- Apply the workshop-gateway row of the audit table, including nested flattening of gateway_progress/tests/ with chained labels. Resulting files: gateway_progress-tests.rs, gateway_progress-tests-lifecycle.rs, gateway_progress-tests-recovery.rs, heartbeat-refresh.rs, heartbeat-tests.rs, observer-tests.rs, resolve-tests.rs, test_gateway-process.rs.
- Verify: `cargo check -p workshop-gateway --all-features`.

</step-14>

<step-15>

### Step 15: Flatten workshop-menu [completed]

- Component: workshop-crates
- Apply the workshop-menu row of the audit table, including nested flattening of menu/tests/memory.rs with a chained label. Resulting files: catalog-chat.rs, catalog-tests.rs, menu-memory.rs, menu-tests.rs, menu-tests-memory.rs.
- Verify: `cargo check -p workshop-menu --all-features`.

</step-15>

<step-16>

### Step 16: Flatten workshop-registry [completed]

- Component: workshop-crates
- Apply the workshop-registry row of the audit table. Dirs to flatten: src/push (tests.rs). Resulting files: push-tests.rs.
- Verify: `cargo check -p workshop-registry --all-features`.

</step-16>

<step-17>

### Step 17: Flatten workshop-server [completed]

- Component: workshop-crates
- Apply the workshop-server row of the audit table, including nested flattening of routes/gateway_config/tests/recovery.rs with a chained label. Resulting files: app-fixtures.rs, app-tests.rs, serve-tests.rs, routes/assets-headless-tests.rs, routes/assets-tests.rs, routes/gateway_config-tests.rs, routes/gateway_config-tests-recovery.rs.
- Verify: `cargo check -p workshop-server --all-features`.

</step-17>

<step-18>

### Step 18: Flatten workshop-sessions [completed]

- Component: workshop-crates
- Apply the workshop-sessions row of the audit table. Resulting files: input-tests.rs, input-tool.rs, relay-tests.rs, session-log.rs, session-menu.rs, agents/supervisor/transition-tests.rs.
- Verify: `cargo check -p workshop-sessions --all-features`.

</step-18>

<step-19>

### Step 19: Flatten workshop-status [completed]

- Component: workshop-crates
- Apply the workshop-status row of the audit table. Dirs to flatten: src/progress (tests.rs). Resulting files: progress-tests.rs.
- Verify: `cargo check -p workshop-status --all-features`.

</step-19>

<step-20>

### Step 20: Flatten workshop-workspace [completed]

- Component: workshop-crates
- Apply the workshop-workspace row of the audit table. Dirs to flatten: src/error (tests.rs), src/handlers (tests.rs), src/workspace (tests.rs). Resulting files: error-tests.rs, handlers-tests.rs, workspace-tests.rs.
- Verify: `cargo check -p workshop-workspace --all-features`.

</step-20>

<step-21>

### Step 21: Run the full verification gate [completed]

- Component: verification
- Run the entire Testing Plan gate with zero test content modified: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`; doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; `cargo nextest run --locked -p workshop -p workshop-server`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with RUSTDOCFLAGS="-D warnings"; `cargo test -p build-xtask`.
- Exit criteria: every command exits zero. No commit expected; if a failure traces to a crate's moves, fix it in that crate's step before re-running the gate.

</step-21>

</execution-plan>
