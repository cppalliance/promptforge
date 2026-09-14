---
name: "Debt removal: capabilities follow-ups"
overview: "Remove the three accepted debts from the capabilities/global-naming range (upstream/master..eeca0a16) - re-type ToolId::capability() to CapabilityId, reject punctuation-twin capability ids at registration, delete the dead-on-arrival 8192 fallback model descriptor in Workshop - plus the exposed pre-existing PF-EXP-01: bound-tool dispatch failures become model-readable tool results instead of aborting the model loop."
todos:
  - id: retype-capability-accessor
    content: Re-type ToolId::capability() to CapabilityId, delete the fill.rs string round-trip, migrate GlobalName-comparing consumers (PF-DEBT-01)
    status: pending
  - id: registry-twin-rejection
    content: Reject punctuation-normalized capability id twins at registry registration with RegistryErrorKind::NormalizationCollision (PF-DEBT-02)
    status: pending
  - id: remove-fallback-descriptor
    content: Delete the 8192 fallback descriptor; report catalog fetch failure as the chat launch error (PF-DEBT-03)
    status: pending
  - id: tool-errors-as-results
    content: Convert bound-tool dispatch failures into model-readable tool results in the model loop (PF-EXP-01)
    status: pending
isProject: false
---

# Debt Removal: Capabilities Follow-ups

<product-contract>

## Product Requirements

A debt-collector pass over the capabilities/global-naming range (`upstream/master` `e615c2ea`..`eeca0a16`, 22 commits) accepted three introduced debts, and the operator added one exposed pre-existing debt to scope. All four are small, local, and independently shippable.

- Problem and users: four debts from the capabilities work now tax the system: a type split between `ToolId` and `CapabilityId` bridged by a string round-trip; a registry that admits punctuation-twin capability ids into model-facing text; a fallback model descriptor whose documented purpose is unreachable; and a model loop that aborts on any bound-tool error, hiding the tool's model-facing message from both the model and the operator. Users are capability authors, hosts (Workshop), and models reading tool output.
- Goals:
  - PF-DEBT-01: `ToolId::capability()` returns `CapabilityId`; no string round-trip or untyped `GlobalName` comparison remains at any seam.
  - PF-DEBT-02: `CapabilityRegistry::register` rejects a capability id that differs from an existing registration only by `-`/`_`/`.` punctuation, with `RegistryErrorKind::NormalizationCollision` naming both ids.
  - PF-DEBT-03: the 8192-token fallback descriptor in `crates/workshop-sessions/src/agents/environment.rs` is deleted; a catalog fetch failure at chat launch is reported as the launch error naming the fetch as cause.
  - PF-EXP-01: a bound tool's own `ToolError` becomes the call's tool result - untrusted-wrapped, model-readable - and the run continues; cancellation, quota, determinism, internal, out-of-scope, and local-tool errors stay fatal.
- Non-goals: no `GlobalName::parse` change (collision is relational, not a parse property); no `Observation` enum payload (the existing tool-result channel already carries the error text to journal and SPA); no `OutOfScopeToolCall` conversion (enforced hiding is a separate security-adjacent decision); no local-tool or Lua `tools.call` error-semantics change (author bugs keep raising); no charset narrowing; no structural ratchets; no change to `min_context` values.
- Success criteria: the four target states above hold; the full workspace suite, doctests, clippy `-D warnings`, and `cargo fmt --all --check` are green.
- Constraints: behavior changes ship with their tests in the same change; user-facing strings are model-facing strings (concise, factual, self-contained) per the root `AGENTS.md` Principles rule; the software is pre-release with first-party hosts only.
- Open questions: none.

## Functional Specification

Each work item changes one observable behavior; everything else is invariant.

- Actors and workflows: capability authors register packs into `CapabilityRegistry`; Workshop launches chat sessions and resolves the dropdown's model per run; models call bound tools through `models.loop`.
- Inputs and outputs:
  - `ToolId::capability() -> CapabilityId` (was `GlobalName`).
  - `register` returns `RegistryError` with kind `NormalizationCollision` for punctuation twins; the message names both ids.
  - `current_model` in `crates/workshop-sessions/src/agents/environment.rs` reports its failure causes (catalog fetch failed; selection absent from the fetched catalog) instead of binding a fallback descriptor; the caller in `crates/workshop-sessions/src/agents/supervisor/effects.rs` (~160-171) turns them into a launch-time error report.
  - In `crates/promptforge-api/src/execute/tool_loop.rs`, the `DispatchTarget::Bound` arm converts `Error::Tool` into the call's result record; all other error classes propagate unchanged.
- States and validation: punctuation normalization maps `-`/`_`/`.` to one canonical byte per segment; case needs no handling (the charset is lowercase-only at parse). The registry's exact-duplicate rejection (`DuplicateId`) is unchanged.
- Errors and recovery: a failed tool call produces a tool result whose content is the `ToolError` message, nonce-wrapped as untrusted (it embeds upstream content); the model adapts, retries, or reports. A model that keeps calling the failing tool exits at `max_tool_iterations` as today.
- Security and privacy behavior: error text is wrapped untrusted like any third-party-embedding content; unadvertised-alias calls stay fatal (enforced hiding unchanged).
- Acceptance criteria:
  - Grep finds no `capability().to_string()` and no `GlobalName`-typed capability comparison.
  - Registering `acme/web-search` then `acme/web_search` (or `acme/web.search`) fails with `NormalizationCollision` naming both; distinct non-twin names register fine.
  - A chat launch whose catalog fetch fails reports the fetch failure; no 8192 descriptor is ever bound.
  - A failing bound tool yields an untrusted tool result carrying its message, the loop continues to a terminal reply, and the `ToolCallFailed` observation still fires.

</product-contract>
<implementation-contract>

## Technical Design

Four independent, local changes. Only PF-DEBT-01 touches a public signature; it completes the return type the capabilities plan's live declaration (`vibe/2026-09-13-1-capabilities-global-naming.md`) always specified.

- Architecture:
  - PF-DEBT-01: one nominal type for capability identity. `CapabilityId` gains a crate-internal constructor from a prefix known 2-segment; `ToolId::capability()` builds on it directly, with no re-parse.
  - PF-DEBT-02: rejection lives at `CapabilityRegistry::register` (`crates/promptforge-api/src/capabilities.rs`), never at parse. The capabilities plan's deferred declarations anticipated exactly this variant ("`GlobalNameError` and `RegistryError` each gain NormalizationCollision"). The existing description near-duplicate lint stays advisory and unchanged.
  - PF-DEBT-03: `current_model` returns the descriptor or a reported cause (a `Result` with a small error enum, or an enum of outcomes); the 8192 `FALLBACK_CONTEXT` constant and the fallback arm are deleted. The boot-window path (no selection yet, first catalog model) keeps working when the fetch succeeds.
  - PF-EXP-01: the Lua `tools.call` arm already delivers dispatch failure as a catchable value (`crates/promptforge-api/src/execute/scheduler.rs`, `dispatch_tool_call` ~1576-1587); the model loop is the only abort-on-tool-error path, and this change extends the existing mechanism to it. `dispatch_tool` (`crates/promptforge-lua/src/dispatch.rs` ~87-147) is unchanged: it fires `TOOL_CALL_FAILED` before returning, so the SPA failure frame from `232204cf` keeps working, and the loop's existing `on_tool_result` records the error text as the result content.
- Modules and interfaces: `shared-promptforge-api` (`ToolId::capability()` re-typed; `CapabilityId` constructor), `promptforge-api` (`RegistryErrorKind::NormalizationCollision`; the `tool_loop.rs` Bound arm), `workshop-sessions` (`current_model` signature and its caller).
- File and public API changes: `crates/shared-promptforge-api/src/tools/ids.rs` and `capabilities.rs`; `crates/promptforge-api/src/capabilities.rs`, `execute/fill.rs` (the ~118 round-trip deleted), `execute/tool_loop.rs`; consumers in `crates/promptforge-model-client/src/model.rs` and `crates/promptforge-tool-picker/src/policy.rs` migrate to the typed form; `crates/workshop-sessions/src/agents/environment.rs` and `agents/supervisor/effects.rs`; `guide/src/language/07-tools.md` documents tool failures arriving as tool results, and the assembled guide is regenerated.
- Data, persistence, failure, security, and privacy constraints: the empty-reply exit counter in `tool_loop.rs` (`successful_tool_calls`, ~165) counts any call that received a result record, error included - rename to match; the round's atomic append (assistant calls plus one result record per call) is preserved; no journal or replay consumer exists to re-teach (the durable tier is out of scope for the product).

</implementation-contract>
<verification-contract>

## Testing Plan

Each item lands with its behavior tests in the same change; the abort-to-result migration re-pins the one test that asserted the old semantics.

- Unit:
  - PF-DEBT-01: existing accessor and containment tests updated to the typed return; the four touched crates compile as the primary proof.
  - PF-DEBT-02: new registry tests - `acme/web-search` then `acme/web_search` fails with `NormalizationCollision` naming both; same for `acme/web.search`; exact duplicate still `DuplicateId`; punctuation-distinct non-twins (`acme/web-search` vs `acme/web-search-extra`) register fine; the description-lint tests are unaffected.
  - PF-DEBT-03: replace `a_failed_catalog_fetch_binds_the_fallback_descriptor` with a test pinning that a fetch failure produces the reported launch error naming the fetch as cause; keep `no_selection_and_no_catalog_means_no_model`.
  - PF-EXP-01: the loop's abort-pinning test (`crates/promptforge-api/src/execute/tests/tool_loop.rs` ~319) migrates to pin error-as-result (result record carries the message, untrusted-wrapped, loop continues to a terminal reply). New tests: cancellation still aborts mid-dispatch; a quota (counts) failure still aborts; a local-tool handler error still aborts; `TOOL_CALL_FAILED` fires alongside the error result; repeated calls to the failing tool exit at `max_tool_iterations`.
- Integration and end-to-end: workshop-sessions suite and the workshop-server chat gates stay green through PF-DEBT-03; the chat end-to-end test (`a_chat_session_activates_the_web_capability_and_calls_search_end_to_end`) is unaffected by PF-EXP-01 because its mock search succeeds.
- Regression, security, and performance: the Lua `tools.call` and local-tool suites are unchanged and green; the guide builds (`mdbook build guide`) with the regenerated assembled guide.
- Exit criteria: full workspace nextest, doctests, clippy `-D warnings`, `cargo fmt --all --check` all green.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - PF-DEBT-01 remedy is the re-type, not a crate-internal conversion shim: the capabilities plan's live declaration always specified `pub fn capability(&self) -> CapabilityId`; the `GlobalName` return was interim with a falsifier that fired when `CapabilityId` landed (`1d0c2a5f`). Consequence: one-pass consumer migration.
  - PF-DEBT-02 remedy is rejection at registration: user-resolved 2026-09-14 ("do actual rejection"), superseding the capabilities plan's deferral - "registry scale" arrived when the registry and the first-party pack landed in-range. Rejection lives at `register`, never at `GlobalName::parse`, because collision is relational.
  - PF-DEBT-03 remedy is deletion: user-resolved 2026-09-14 ("do deletion"). The fallback caused two corrective episodes in one night (`090fc747`, `eeca0a16`) and zero successful degradations; its only consumer now refuses it by construction (`min_context: 32768` in `crates/workshop-sessions/agents/chat.md`).
  - PF-EXP-01 remedy is error-as-result at the model-dispatch boundary: user-resolved 2026-09-14 (scope expanded after the user asked "can we fix this reliably?"). Two same-night outages (an invisible 404, an invisible context refusal) would have been self-explaining to the model under this behavior.
- Rejected alternatives:
  - Advisory lint only for name twins: weaker than the approved remedy; the description lint already covers the advisory channel. Revisit never.
  - Charset narrowing to one separator: breaks the shipped `web-search`/`web_fetch` wire names' descendants and dotted reverse-DNS namespaces. Revisit never.
  - Raising `FALLBACK_CONTEXT` above 32768: fabricates a window the model may not have and moves the failure provider-side. Revisit never.
  - Mark-degraded binding: still binds unreliable metadata. Revisit never.
  - Payload on `Observation::ToolCallFailed`: redundant once the error text flows through the existing tool-result channel to the journal and SPA. Revisit never.
  - Converting `OutOfScopeToolCall` to a result: enforced hiding is a separate security-adjacent decision. Revisit when the deferred discovery capability lands.
  - Converting local-tool handler errors: author bugs should abort. Revisit never.
- Assumptions, risks, and notes:
  - Pre-release with first-party hosts only: the `capability()` signature change and the loop's behavior change have no external consumers to defend.
  - `RunErrorKind::Tool` no longer covers bound-tool failures from the model loop; it remains for the Lua arm and local tools.
  - The debt-collector findings and challenge (2026-09-14, disposition ref `eeca0a16`) upheld all four items; 21 other candidates were rejected (11 residual-but-acceptable, 5 weak/speculative, 5 false at the disposition ref) and are deliberately not in scope.

### Deferred and Out of Scope

- Deferred: nothing new; every deferral from the capabilities plan (the prompt-pack, the open toolset, the `prompt` global, bridge capabilities, versioning) stands untouched.
- Out of scope: the residual-but-acceptable candidates from the debt pass - they are deferrals with named landing points or reviewed policies, not ripe debt.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (default-members builds only the gateway, which compiles on a fresh clone with no CUDA or Tauri system packages; the desktop app is an explicit `cargo build -p workshop`)
- Focused test command pattern: `cargo nextest run --locked -p <crate> <test-name-substring>`
- Component test command pattern: `cargo nextest run --locked -p <crate>` (workshop crates: `-p workshop -p workshop-server`)
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`)
- Formatter check command: `cargo fmt --all --check`
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`
- Test placement and naming conventions: unit tests live inline in `src/` files under `#[cfg(test)]` modules; integration tests live in `crates/<crate>/tests/`, most commonly as a single `it` target (`tests/it/main.rs` with submodules), with some crates using named targets (e.g. `engine_contract.rs`, `interruption.rs`); shared helpers and data live in `tests/common/` and `tests/fixtures/`; boundary and structural harness runs via `cargo test -p build-xtask`; nextest config in `.config/nextest.toml` defines a `heavy` test group (max-threads 2) for promptforge-tool-picker, gateway-stt-backend-whisper, and gateway-stt
- Directory map: `crates/` holds all workspace members (Cargo glob `crates/*`, excluding `crates/shared-ui`, a TypeScript+CSS package); `guide/` is the mdbook user guide; `prompts/` holds PromptForge prompt files; `tools/` holds repo tooling; `vibe/` holds session logs and `archdoc.md`; `images/` holds assets; `local/` and `target*/` are build/local output; `.config/` holds nextest config, `.githooks/` git hooks, `.github/` CI
- Component boundaries (per `vibe/archdoc.md` and AGENTS.md): three products - PromptForge (executor/runtime, `promptforge-*` crates), Gateway (inference service, `gateway-*` crates), Workshop (Tauri desktop, `workshop-*` crates) - plus `shared-*` substrate crates and `build-*` output builders; dependency rules: workshop-* never depends on gateway-*, gateway-* never depends on promptforge-*/workshop-*, promptforge-* never depends on gateway-*/workshop-*, shared-* depends on no product crates, and crates outside promptforge-* may depend only on promptforge-api (the one-door rule); executor depends on gateway, store, Lua VM boundary, and shared substrate; VFS layer (`shared-vfs` plus the `promptforge-vfs` policy gate) depends on nothing
- Conventions summary: Rust edition 2024, stable toolchain (`rust-toolchain.toml`); workspace lints forbid unsafe_code and deny clippy `all`, `unwrap_used`, `expect_used`; no file exceeds 500 lines (enforced by build-xtask); every workshop-* crate's lib.rs opens with a `## Invariants` doc marker listing allowed dependencies; dependencies flow shell -> features -> services -> vocabulary; behavior changes ship with tests in the same change; error messages are written for model consumption (concise, factual, self-contained); Cargo features gate real constraints (toolchain, native builds), not product shape; CSS lives beside its TypeScript in self-contained feature directories using `--ws-*` design tokens; comments cite upstream issue URLs for workarounds

</project-survey>
<execution-plan>

## Execution Instructions

Components in dependency order:

1. `capability-id-retype` (PF-DEBT-01) - first: it changes the shared vocabulary crate (`shared-promptforge-api`) that the other PromptForge components build against, and it completes the public signature the capabilities plan always declared.
2. `registry-twin-rejection` (PF-DEBT-02) - second: it edits the same file (`crates/promptforge-api/src/capabilities.rs`) as the re-type's containment-check migration; landing adjacent avoids same-file rebase churn.
3. `fallback-descriptor-removal` (PF-DEBT-03) - third: isolated to `workshop-sessions`; no coupling to the PromptForge components.
4. `tool-errors-as-results` (PF-EXP-01) - last: widest test surface (one migrated abort-pinning test plus five new behavior tests) and a guide regeneration; landing last puts the exit gate immediately after the largest change.

<step-1>

### Step 1: re-type ToolId::capability() to CapabilityId [completed]

- Component: capability-id-retype
- Piece: typed-accessor-and-consumers (single piece, joint construction: the signature change and the consumer migrations must compile together in one commit)
- Change: in `crates/shared-promptforge-api/src/capabilities.rs`, add a crate-internal `CapabilityId` constructor from a prefix known 2-segment; in `crates/shared-promptforge-api/src/tools/ids.rs`, re-type `ToolId::capability()` to return `CapabilityId` built on that constructor with no re-parse; delete the string round-trip in `crates/promptforge-api/src/execute/fill.rs` (~118); migrate the containment check in `crates/promptforge-api/src/capabilities.rs` and the `GlobalName`-comparing consumers in `crates/promptforge-model-client/src/model.rs` and `crates/promptforge-tool-picker/src/policy.rs` to the typed form.
- Tests: update the existing accessor and containment tests to the typed return; the four touched crates compiling is the primary proof.
- Verify: `cargo nextest run --locked -p shared-promptforge-api -p promptforge-api -p promptforge-tool-picker -p promptforge-model-client`; clippy on the same crates with `-D warnings`; grep finds no `capability().to_string()` and no `GlobalName`-typed capability comparison.

</step-1>

<step-2>

### Step 2: reject punctuation-twin capability ids at registration

- Component: registry-twin-rejection
- Piece: normalization-collision-rejection (single piece: one error kind, one register check, one test set)
- Change: in `crates/promptforge-api/src/capabilities.rs`, add `RegistryErrorKind::NormalizationCollision`; at `CapabilityRegistry::register`, reject a capability id that differs from an existing registration only by `-`/`_`/`.` punctuation (normalize each segment to one canonical byte per separator; case needs no handling, the charset is lowercase-only at parse), with a model-facing message naming both ids; leave `GlobalName::parse`, the exact-duplicate `DuplicateId` path, and the advisory description near-duplicate lint unchanged.
- Tests: new registry tests - `acme/web-search` then `acme/web_search` fails with `NormalizationCollision` naming both; same for `acme/web.search`; exact duplicate still `DuplicateId`; punctuation-distinct non-twins (`acme/web-search` vs `acme/web-search-extra`) register fine; the description-lint tests are unaffected.
- Verify: `cargo nextest run --locked -p promptforge-api`.

</step-2>

<step-3>

### Step 3: delete the 8192 fallback model descriptor

- Component: fallback-descriptor-removal
- Piece: catalog-failure-reporting (single piece, joint construction: the `current_model` signature change, the fallback deletion, and the caller's error reporting compile together)
- Change: in `crates/workshop-sessions/src/agents/environment.rs`, delete the `FALLBACK_CONTEXT` constant and the fallback arm of `current_model`; change `current_model` to return the descriptor or a reported cause (a `Result` with a small error enum, or an enum of outcomes) covering catalog-fetch-failed and selection-absent-from-the-fetched-catalog; in `crates/workshop-sessions/src/agents/supervisor/effects.rs` (~160-171), turn those causes into the chat launch error naming the fetch as cause; keep the boot-window path (no selection yet, first catalog model) working when the fetch succeeds; no `min_context` values change.
- Tests: replace `a_failed_catalog_fetch_binds_the_fallback_descriptor` with a test pinning that a fetch failure produces the reported launch error naming the fetch as cause; keep `no_selection_and_no_catalog_means_no_model`.
- Verify: `cargo nextest run --locked -p workshop-sessions`; the workshop-server chat gates stay green (`cargo nextest run --locked -p workshop -p workshop-server`).

</step-3>

<step-4>

### Step 4: convert bound-tool dispatch failures into tool results

- Component: tool-errors-as-results
- Piece: bound-arm-error-as-result (sequential before the guide piece: behavior lands first, docs describe landed behavior)
- Change: in `crates/promptforge-api/src/execute/tool_loop.rs`, convert the `DispatchTarget::Bound` arm so a tool's own `Error::Tool` becomes the call's result record - content is the `ToolError` message, nonce-wrapped as untrusted - and the run continues; cancellation, quota, determinism, internal, out-of-scope, and local-tool errors propagate unchanged; rename the `successful_tool_calls` counter (~165) to match its answered-call semantics (any call that received a result record, error included); preserve the round's atomic append (assistant calls plus one result record per call); leave `dispatch_tool` (`crates/promptforge-lua/src/dispatch.rs` ~87-147) unchanged so `TOOL_CALL_FAILED` still fires before return and the loop's existing `on_tool_result` records the error text as the result content.
- Tests: migrate the abort-pinning test (`crates/promptforge-api/src/execute/tests/tool_loop.rs` ~319) to pin error-as-result (result record carries the message, untrusted-wrapped, loop continues to a terminal reply); new tests - cancellation still aborts mid-dispatch; a quota (counts) failure still aborts; a local-tool handler error still aborts; `TOOL_CALL_FAILED` fires alongside the error result; repeated calls to the failing tool exit at `max_tool_iterations`.
- Verify: `cargo nextest run --locked -p promptforge-api`; the Lua `tools.call` and local-tool suites are unchanged and green.

</step-4>

<step-5>

### Step 5: document tool failures as tool results in the guide

- Component: tool-errors-as-results
- Piece: guide-documentation (sequential after the bound-arm piece)
- Change: update `guide/src/language/07-tools.md` to document that a bound tool's own failure arrives as an untrusted-wrapped, model-readable tool result and the run continues; regenerate the assembled guide.
- Tests: none; docs-only step.
- Verify: `mdbook build guide` exits 0.

</step-5>

Exit gate after the last step: full workspace nextest (`cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`), doctests (`cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`), workshop crates (`cargo nextest run --locked -p workshop -p workshop-server`), clippy `-D warnings` (workspace and workshop invocations per the Project Survey), `cargo fmt --all --check`.

</execution-plan>
