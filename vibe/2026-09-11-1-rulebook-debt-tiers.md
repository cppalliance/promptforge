---
name: Fix tier 1 and 2 rulebook deviations
overview: Fix all Tier 1 (correctness/operational) and Tier 2 (hygiene) rust-rulebook deviations verified at promptforge HEAD da1456b, as a series of focused commits following the repo's vibe/ plan conventions.
todos:
  - id: ws-b
    content: "B: CI doctest step + aggregate job + drop unused tracing-subscriber dev-dep"
    status: pending
  - id: ws-a1
    content: "A: fix blocking/fallible Drop impls (RecoveryCandidate, GatewaySupervisor, SttEngine/Transcriber)"
    status: pending
  - id: ws-a2
    content: "A: restore discarded error causes (SessionError::Inference, AudioError::InvalidBase64)"
    status: pending
  - id: ws-a3
    content: "A: bound the session_agents supervisor event channel"
    status: pending
  - id: ws-d1
    content: "D: add #[non_exhaustive] to 7 public items/variants"
    status: pending
  - id: ws-d2
    content: "D: convert 43 #[allow] sites to #[expect] with reasons"
    status: pending
  - id: ws-e
    content: "E: convert ~5 in-process async tests to paused time"
    status: pending
  - id: ws-c1
    content: "C1: FixtureError for the fixture API + typed sources in the take/finalization pipeline (~26 sites)"
    status: pending
  - id: ws-c2
    content: "C2: anyhow for test-only code and build tools + folded typed errors in remaining production (~32 sites)"
    status: pending
isProject: false
---

# Fix Tier 1 + Tier 2 rust-rulebook deviations in promptforge

<product-contract>

## Product Requirements

A 150-commit audit of the promptforge repository against the workspace Rust rulebook found 78 commits introducing deviations. The maintainers who run this repository need the correctness-relevant subset of that debt eliminated without a style-churn campaign, because the audit also showed that the repository holds several deliberate counter-conventions where the rulebook, not the code, is what should change.

- Goals: fix every verified Tier 1 finding (blocking or fallible destructors, discarded error causes, a CI doctest coverage gap, an unused dev-dependency, an unbounded channel) and every verified Tier 2 finding (stringly `Result<_, String>` errors, missing `#[non_exhaustive]` on public items, `#[allow]` suppressions that should be `#[expect]`, real-time sleeps in in-process async tests) at HEAD `da1456b`.
- Non-goals: the entire Deferred Workstream cataloged under Execution Instructions - documentation-example convergence, oversized-file splits, import regrouping, bool-flag elimination, test-layout consolidation, the toolchain-pin policy question, and the real-network test sleeps.
- Success criteria: every finding verified at HEAD is either fixed with the full verification gate green or explicitly recorded as deferred.
- Constraints, from the repository's own policy:
  - Behavior changes ship with tests in the same change.
  - No structural enforcement may be introduced.
  - The four cross-product dependency rules hold.
  - The local toolchain is pinned to MSRV 1.89 while CI lints on a newer stable, so import and doc-comment changes must be checked with the newer toolchain before pushing.
  - Work lands as focused commits following the repository's vibe/ plan and ledger conventions.
- Open questions: the bounded capacity for the supervisor event channel in workstream A, delegated to the executor with a justification requirement.

## Functional Specification

The actor is a maintainer (or agent) executing the workstreams as a series of commits; CI is the enforcing counterparty. The inputs are the HEAD-verified inventories recorded under Project Survey, and the outputs are six commits, one per step, plus this plan committed to the repository. Each commit is a state transition that must leave the full verification gate green; a commit that cannot pass the gate does not land.

Two recovery rules govern the work itself:

- Where a fix changes runtime behavior (the destructor and channel work), tests proving the new behavior ship in the same commit.
- Where an `#[allow]` suppression turns out to be stale during conversion, it is deleted rather than converted.

On security and privacy, restoring error sources changes what printed error chains contain; the added sources are internal typed errors carrying no credential material, and the existing log redaction in gateway-logging must remain intact and is verified by its existing tests.

Acceptance is observable:

- No `Drop` impl in the touched types performs blocking joins or network I/O.
- The two discarded error causes are carried as sources.
- No `unbounded_channel` remains in the session-agents supervisor pipe.
- Workshop doctests run in CI under an aggregate required-status job.
- The unused dev-dependency is gone.
- No `Result<_, String>` remains in the surveyed sites.
- The seven public items carry `#[non_exhaustive]`.
- No surveyed `#[allow]` remains without conversion or deletion.
- The in-process async tests run on paused time.

</product-contract>

<implementation-contract>

## Technical Design

The design is a set of independent local repairs against one workspace; there is no cross-module architecture change.

Every edit conforms to these rules, stated here in full so execution needs no external reference: library errors are concrete thiserror types while test, build-script, and build-tool code uses anyhow; error messages are lowercase noun phrases with no trailing period and no `failed to` prefix; every wrapped error carries its cause in a `#[source]` field; public error enums and their data-carrying variants are `#[non_exhaustive]`; error types are `Send + Sync + 'static`; lint suppressions use `#[expect(lint, reason = "...")]`, and a suppression found stale is deleted rather than converted; new public items are documented, with `# Errors` where they return `Result`; destructors stay infallible and non-blocking, with fallible or blocking work exposed through explicit `shutdown()` methods; and every behavior change ships its tests in the same commit.

**Destructors.** Blocking `Drop` bodies are replaced with explicit fallible shutdown methods plus non-blocking drops. `RecoveryCandidate` in [crates/workshop/src/gateway/supervisor.rs:93](promptforge/crates/workshop/src/gateway/supervisor.rs) gains an explicit `shutdown(self) -> Result<...>` owning the `request_shutdown_before` call; its `Drop` becomes a non-blocking best-effort signal rather than a no-op, so a missed explicit `shutdown()` still signals the unpublished gateway process. `GatewaySupervisor` (same file, line 381) and `SttEngine` ([crates/gateway-stt-engine/src/engine.rs:187](promptforge/crates/gateway-stt-engine/src/engine.rs), with the same pattern in `Transcriber` at worker.rs:138) already expose `shutdown()` and need only their `Drop` bodies reduced to signal-and-detach, with doc comments stating that the explicit method is the blocking, error-reporting path. Two checks gate all of this: before any detach, verify the workers hold only owned or `Arc` state and borrow nothing from the dropped object, and audit every drop site of all three types so no caller silently relies on the old blocking drop.

**Error causes.** Two unit variants gain source fields: `SessionError::Inference` in [crates/gateway-stt/src/realtime/session/state.rs:44](promptforge/crates/gateway-stt/src/realtime/session/state.rs) and `AudioError::InvalidBase64` in [crates/gateway-stt/src/audio.rs:171](promptforge/crates/gateway-stt/src/audio.rs), eliminating the `map_err(|_| ...)` discards at their call sites. Both variants become data-carrying, so each also gains variant-level `#[non_exhaustive]`, and their messages stay lowercase noun phrases.

**Supervisor channel.** The event pipe in [crates/workshop-server/src/session_agents.rs:251](promptforge/crates/workshop-server/src/session_agents.rs) becomes a bounded `mpsc::channel(N)`. This is not semantics-preserving: today's unbounded sends fail only when the receiver is gone, while a bounded queue can also drop events under load, and these are lifecycle events whose loss can hang a state transition. Before choosing N, the executor must classify which `SupervisorEvent` kinds are loss-tolerant, route loss-intolerant events over a guaranteed path (a blocking send where the call site allows it, or a separate unbounded side channel), and justify N with headroom evidence for the remainder.

**Stringly errors.** The elimination introduces one new public error type, a thiserror `FixtureError` for the feature-gated fixture API in [crates/gateway-stt/src/test_fixtures.rs](promptforge/crates/gateway-stt/src/test_fixtures.rs), with `#[non_exhaustive]` on the enum and separately on every data-carrying variant, operation-named variants, lowercase noun-phrase messages with no trailing period, and `Send + Sync + 'static`. The work reuses `SessionError`/`TranscribeError` where they are the genuine source in the take/finalization pipeline, folds private production helpers into their crates' existing error enums, and uses `anyhow::Result` in test-only code, the build scripts, and the build-tool binaries. New public items introduced anywhere in this work carry full documentation with `# Errors` sections where they return `Result`.

**Attributes.** Mechanical work: `#[non_exhaustive]` on seven public items and `#[allow]` to `#[expect(..., reason = "...")]` across 43 surveyed sites.

**Paused time.** Roughly five in-process async tests convert to `#[tokio::test(start_paused = true)]` with explicit clock advance, contingent on the `test-util` feature being enabled for those crates.

The public API surface changes are additive attributes and new error types only; all affected crates are `publish = false`, so no semver or deprecation staging applies.

### Verified inventories at HEAD

The repository is a multi-crate Rust workspace at HEAD `da1456b` with a clean tree, edition 2024, resolver 3, MSRV 1.89 pinned in `rust-toolchain.toml`, and all product crates `publish = false`. The verified inventories that bound the work:

- Four blocking or fallible destructor sites: supervisor.rs:93 and :381 in workshop, engine.rs:187 and worker.rs:138 in gateway-stt-engine.
- Two discarded error causes: route.rs:109 and audio.rs:171 in gateway-stt.
- One unbounded channel in the session-agents supervisor pipe (session_agents.rs:251, with senders in lifecycle.rs and receivers in supervisor/events.rs); the other two audited unbounded channels were already removed.
- A CI workflow at [.github/workflows/ci.yml](promptforge/.github/workflows/ci.yml) whose `check-workshop` job runs nextest for workshop and workshop-server while the only doctest step explicitly excludes both crates, and which has no aggregate `needs:` job.
- An unused `tracing-subscriber` dev-dependency at [crates/gateway-stt/Cargo.toml:35](promptforge/crates/gateway-stt/Cargo.toml), verified unreferenced in the crate.
- 58 stringly `Result<_, String>` sites: ~20 in the feature-gated fixture API of gateway-stt `test_fixtures.rs`, ~6 in the production take/finalization pipeline, ~12 in test-only code, and ~24 across remaining production code and build tools.
- 6 public items missing `#[non_exhaustive]` - `GatewayStartup` and its `OwnerTimeout` variant in gateway `relaunch.rs`, `DecodeMode` and `DecodeRequest` in gateway-stt-engine `decoder.rs`, `EnginePolicy` in `policy.rs`, `ValidatedConnection` in shared-sidecar `validated.rs` - plus the `GatewayPublicationError::Build` variant in workshop-server `gateway_binding.rs` to align.
- 43 `#[allow]` suppressions, of which 26 already carry reasons and 17 need reasons written.
- 32 `tokio::time::sleep` calls in test code, of which exactly one already uses `start_paused` and roughly six are in-process and pause-friendly: promptforge-core `execute/tests/input.rs:315`, promptforge-lua `dispatch.rs:282`, promptforge-core `execute/tests/scheduler.rs:3089` and `:3121`, gateway-stt-engine `test_fixtures/tests/scenario_cleanup/decode.rs:89`.

Whether `anyhow` is already in `[workspace.dependencies]` and whether the affected crates enable tokio's `test-util` feature are both to be confirmed at execution time.

</implementation-contract>

<verification-contract>

## Testing Plan

Unit coverage comes from the rule that every behavior change ships its tests in the same commit: the destructor changes gain tests proving drop no longer blocks and explicit shutdown still reports failures, the channel change gains tests proving loss-intolerant events are guaranteed delivery and a full queue never grows without bound, and the error-source changes gain assertions on the restored chains. Integration coverage is the existing suite, which must pass unmodified except where a surveyed signature changed; the paused-time conversions are themselves test changes and must be proven deterministic by running the affected tests repeatedly.

Verification is deliberately light per step and heavy once at the end. During coding, only the step's focused tests run. Each step ends with a component-scope check: the build, `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and the touched crates' tests. The full gate runs once, on the final step:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings` (additionally with the newer CI stable whenever imports or doc comments were touched)
- `cargo test --locked --workspace --all-features` plus `cargo nextest run -p workshop -p workshop-server`
- `cargo test --doc`
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`

The exit criterion is that gate green on the final commit with every acceptance behavior under Functional Specification observable.

</verification-contract>

<decision-record>

## Decision Record

The governing decisions were made in conversation and are recorded with the user's words.

- Scope: fix the correctness and hygiene tiers only - "lets fix everything in tiers 1 and 2." Rejects a fix-everything campaign, because 78 of 150 commits deviating from a rule indicates the rule rather than the code is wrong for this repository, and because style churn costs blame history and review load for no behavioral gain. Revisit when a maintainer decides to enforce the rulebook strictly, which should happen through a rulebook addendum first.
- Async tests: convert only the in-process tests to paused time - the user selected "Convert only the ~6 in-process tests to paused time; leave real-network tests as-is." The remaining ~26 tests drive real websocket, TCP, or HTTP peers whose wall-clock behavior paused time cannot model. Revisit if a fake-transport seam appears for those suites.
- Stringly errors: the full cleanup - the user selected "Fix all 58: anyhow in test code, concrete thiserror types in production and the fixture API." Rejects the production-only and tests-only subsets because partial conversions leave the fixture API, the largest cluster, stringly.
- Deferred workstream: the user asked to "add as Deferred Workstream everything else," keeping every remaining finding cataloged rather than silently dropped.
- Plan form: the user asked for "the mandated sections but ... expository paragraphs instead of rigid bullets," softened to prose-plus-bullets after review, and because "I will want this plan committed to the repo," the plan is written to stand alone and lands in the repository under the vibe/ plan convention.
- Execution style: strictly serial, one commit at a time - "I do not want parallel execution." Rejects concurrent subagent workstreams despite their disjoint file sets; no revisit condition was requested.
- Step shape: six steps - the five consolidated workstreams with C split at its design seam - after the user asked "Would you prefer 6, or even 7 steps? I'm open to it - use your judgement." C1 (fixture API plus the take/finalization pipeline) carries the design content, the new `FixtureError` type; C2 (test-only anyhow conversions and remaining production helpers) is the mechanical sweep. D was not split because D1 is seven one-line attributes whose review overhead would exceed its risk. Rejects finer decomposition into per-crate or per-cluster commits to keep per-step review and verification overhead proportionate.
- Verification cadence: light per step, full gate once at the end - "I don't want to have to do the full verify at every step." Rejects the plan's earlier gate-per-commit wording; the accepted risk is that a cross-crate regression surfaces at the final step rather than at its originating commit.

Assumptions, risks, and notes:

- The audit judged commits historically, so the HEAD-verified inventory under Project Survey governs and any site that drifted since verification is re-checked before editing.
- The destructor changes alter shutdown timing and carry the highest behavioral risk in the plan.
- Adding `#[source]` changes printed error chains, which is the intent.
- Several audit findings self-healed before this plan (the `SpeechReplacement` rollback drop, the unbounded relay and decode channels, `SpeechError::Rollback`, and the source-compiled CI tool installs), which is why historical findings alone never justify an edit.
- A pre-execution review corrected the plan itself: the channel work originally claimed bounded `try_send` preserves today's semantics exactly (false - a full queue drops lifecycle events that today are only lost when the receiver is gone), the detach work lacked a check that workers hold no borrows from the dropped object, two either/or instructions were unresolved, and the C/D ordering contradicted itself. All were repaired before handoff.
- A final conformance pass inlined the governing rules into Technical Design so execution needs no external reference: it added variant-level `#[non_exhaustive]` for the error variants that become data-carrying, the style requirements for the new `FixtureError` (operation-named variants, lowercase noun-phrase messages, `Send + Sync + 'static`), and the documentation requirement for new public items.

</decision-record>

<project-survey>

## Project Survey

- Status: complete
- Build: `cargo build --locked -p gateway` (gateway is the sole default member; the desktop app is an explicit `cargo build --locked -p workshop`). UI bundles require `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui` first; crate build scripts bundle the UIs into `OUT_DIR`.
- Focused test command pattern: `cargo test -p <crate> <name-filter>` (used by CI for process-ownership races, e.g. `cargo test --locked -p shared-sidecar a_process_lifetime_lease_recovers_after_its_owner_is_terminated`).
- Component test command pattern: `cargo nextest run --locked -p <crate>` (workshop job runs `cargo nextest run --locked -p workshop -p workshop-server`); gateway integration suites run via `cargo test -p <crate> --test it [filter]` (e.g. `cargo test -p gateway-stt --test it architecture`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, plus doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; workshop crates lint separately with `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`. CI lints on stable, newer than the pinned MSRV.
- Formatter check command: `cargo fmt --all --check`.
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS: -D warnings`. The user guide builds with `mdbook build guide`; its generated indexes regenerate via `cargo run -p build-user-guide`.
- Test placement and naming conventions: unit tests live in `src` modules; integration tests live under `crates/<crate>/tests/` as a multi-file suite behind a `main.rs` harness with one sibling module per area (promptforge-core: `tests/suite/{main,execution,fanout,parsing,shipped,support}.rs`; gateway: `tests/it/{main,boot,cache,chat,cuda,local,profiles,queue,sidecar,...}.rs`). Fixtures sit beside suites (e.g. `tests/prompts/`). Criterion benches live in `crates/promptforge-core/benches/`. Test names are descriptive snake_case sentences (`simultaneous_direct_launches_leave_one_owner_and_one_clean_handoff`).
- Directory map: `crates/` holds all 36 Rust workspace members plus `crates/shared-ui` (TypeScript+CSS package, excluded from the Cargo workspace); `guide/` is the mdBook user guide with four audience sets (`src/workshop/`, `src/gateway/`, `src/language/`, `src/agent/`) and a `scratch/` regeneration cache; `tools/` holds harness tool files and node scripts (`stage-gateway-sidecar.mjs`); `prompts/` holds prompt files; `vibe/` holds `archdoc.md` and dated design notes; `images/` holds README assets; `.github/workflows/` holds CI (`ci.yml`, `guide.yml`, release and nightly workflows); `target/` and `target-msrv/` are build output.
- Component boundaries (per `vibe/archdoc.md`): the executor (promptforge-core, promptforge-parser, promptforge-lua, promptforge-agent) parses and runs pipelines and depends on gateway, store, Lua VM boundary, and shared substrate; the gateway (gateway plus gateway-* crates) owns model routing, provider credentials, and local inference and depends only on shared substrate; the CLI (`promptforge` crate) is a thin shell adapter over executor, gateway, and store; the workshop UI (workshop, workshop-server) hosts the executor in-process and attaches over the gateway protocol; the store (promptforge-store) is a run-scoped virtual filesystem with no dependencies; the shared substrate (shared-loopback, shared-progress, shared-protocol, shared-sidecar) has no dependencies. AGENTS.md binds four cross-product rules: gateway crates cannot depend on workshop crates, promptforge product crates cannot depend on gateway or workshop product crates, gateway product crates cannot depend on promptforge product crates, and workshop product crates cannot depend on gateway product crates.
- Conventions summary: Rust edition 2024, MSRV 1.89 pinned in `rust-toolchain.toml` while CI lints on newer stable; workspace lints forbid `unsafe_code`, deny clippy `all` plus `unwrap_used`/`expect_used`, and warn on `missing_docs` and pedantic; behavior changes ship with tests in the same change and structural enforcement checks require explicit user approval; library and serve paths return failures instead of exiting or installing process-global state; long-running work reports through `shared-progress`; comments explain non-obvious constraints and cite upstream issue URLs for platform workarounds; Cargo features gate real constraints (toolchain, native builds), not product shape; build steps must never dirty the git tree (CI enforces a clean-tree check); docs prose bans em-dashes and opens code fences with four backticks.

</project-survey>

<execution-plan>

## Execution Instructions

The work decomposes into five components landing as six steps, each step one commit, executed strictly serially in the order below per the recorded decision against parallel execution. Component placement rationale: `ci-gate-hygiene` first so the stricter CI gate guards every later commit; `runtime-hazard-repairs` next because it carries the highest behavioral risk and benefits from the tightened gate; `api-attributes` and `paused-time-tests` follow as independent mechanical components with no dependencies on each other or on the error work; `stringly-error-elimination` last because it is largest. That component splits into two sequential pieces: the `FixtureError` design content (Step 5) before the mechanical sweep (Step 6), because Step 5 introduces the `FixtureError` type and typed pipeline sources the remaining work folds into. Where a file is in both the attribute component's and the error component's scope, the attribute step skips it and that file's conversions fold into the corresponding error commit after the semantic edits.

Per-step verification is deliberately light: the step's focused tests, then a component-scope check of the build, `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, and the touched crates' tests. The full verification gate from the Testing Plan runs once, on the final step. Every behavior change ships its tests in the same commit.

The run is seeded by copying this plan verbatim to `vibe/YYYY-MM-DD-N-words.md`, pointing `vibe/ACTIVE` at that path, and committing both with subject `[WIP] Plan: <plan title>`; progress is marked in the repository copy as steps complete.

<step-1>

### Step 1: CI doctest coverage and dependency hygiene [completed]

- Component: ci-gate-hygiene

Add a `cargo test --doc -p workshop -p workshop-server` step to the `check-workshop` job in `.github/workflows/ci.yml`, closing the gap where the only doctest step explicitly excludes both crates. Add an aggregate required-status job with `needs:` on all existing jobs and an `if: always()` failure check. Delete the unused `tracing-subscriber` dev-dependency at `crates/gateway-stt/Cargo.toml:35`, verified unreferenced in the crate.

Verification: the workflow lints clean, `gateway-stt` builds and its tests pass without the dev-dependency, and the component-scope check is green.

</step-1>

<step-2>

### Step 2: Non-blocking destructors, restored error causes, bounded supervisor channel

- Component: runtime-hazard-repairs

Destructors: give `RecoveryCandidate` (`crates/workshop/src/gateway/supervisor.rs:93`) an explicit fallible `shutdown(self)` owning the `request_shutdown_before` call, and reduce its `Drop` to a non-blocking best-effort signal so a missed explicit `shutdown()` still signals the unpublished gateway process. Reduce the `Drop` bodies of `GatewaySupervisor` (same file, line 381), `SttEngine` (`crates/gateway-stt-engine/src/engine.rs:187`), and `Transcriber` (`crates/gateway-stt-engine/src/worker.rs:138`) to signal-and-detach, with doc comments stating that the explicit `shutdown()` is the blocking, error-reporting path. Before any detach, verify the workers hold only owned or `Arc` state and borrow nothing from the dropped object, and audit every drop site of all three types so no caller silently relies on the old blocking drop.

Error causes: add `#[source]` fields to `SessionError::Inference` (`crates/gateway-stt/src/realtime/session/state.rs:44`) and `AudioError::InvalidBase64` (`crates/gateway-stt/src/audio.rs:171`), making both variants data-carrying with variant-level `#[non_exhaustive]` and lowercase noun-phrase messages, and delete the two `map_err(|_| ...)` discards at their call sites (`route.rs:109` and `audio.rs:171`).

Channel: convert the event pipe at `crates/workshop-server/src/session_agents.rs:251` (senders in `lifecycle.rs`, receivers in `supervisor/events.rs`) to a bounded `mpsc::channel(N)` after classifying which `SupervisorEvent` kinds are loss-tolerant, routing loss-intolerant events over a guaranteed path (a blocking send where the call site allows it, or a separate unbounded side channel), and justifying N with headroom evidence for the remainder.

Tests in the same commit prove: drop no longer blocks and explicit shutdown still reports failures; the restored error chains carry their sources; loss-intolerant events get guaranteed delivery and a full queue never grows without bound. The component-scope check follows.

</step-2>

<step-3>

### Step 3: `#[non_exhaustive]` attributes and `#[expect]` conversions

- Component: api-attributes

Apply `#[non_exhaustive]` to the seven surveyed public items: `GatewayStartup` and its `OwnerTimeout` variant (gateway `relaunch.rs`), `DecodeMode` and `DecodeRequest` (gateway-stt-engine `decoder.rs`), `EnginePolicy` (gateway-stt-engine `policy.rs`), `ValidatedConnection` (shared-sidecar `validated.rs`), and the `GatewayPublicationError::Build` variant (workshop-server `gateway_binding.rs`). Convert the 43 surveyed `#[allow]` suppressions to `#[expect(lint, reason = "...")]`, writing reasons for the 17 that lack them and deleting any suppression found stale rather than converting it. Skip files in the stringly-error component's scope; their attribute conversions fold into Steps 5 and 6 after the semantic edits.

This is a mechanical change: the existing suite unmodified plus the component-scope check is the verification.

</step-3>

<step-4>

### Step 4: Paused-time conversion for in-process async tests

- Component: paused-time-tests

After confirming tokio's `test-util` feature is enabled for the affected crates, convert the roughly five in-process async tests to `#[tokio::test(start_paused = true)]` with explicit clock advance: promptforge-core `execute/tests/input.rs:315`, promptforge-lua `dispatch.rs:282`, promptforge-core `execute/tests/scheduler.rs:3089` and `:3121`, and gateway-stt-engine `test_fixtures/tests/scenario_cleanup/decode.rs:89`. Leave the ~26 real-network test sleeps as-is per the recorded decision.

Prove the conversions deterministic by running the affected tests repeatedly; the component-scope check follows.

</step-4>

<step-5>

### Step 5: `FixtureError` and typed sources in the take/finalization pipeline

- Component: stringly-error-elimination

Eliminate the ~26 stringly `Result<_, String>` sites in the fixture API and the production take/finalization pipeline. Introduce a public thiserror `FixtureError` for the feature-gated fixture API in `crates/gateway-stt/src/test_fixtures.rs`: `#[non_exhaustive]` on the enum and separately on every data-carrying variant, operation-named variants, lowercase noun-phrase messages with no trailing period and no `failed to` prefix, `#[source]` where wrapping, `Send + Sync + 'static`, and full documentation with `# Errors` on new public items returning `Result`. Include the future bound in `realtime/session/items.rs`. Give the pipeline sites in `realtime/item.rs`, `take/state.rs`, `take/finalization.rs`, and `realtime/session/state.rs` typed sources, reusing `SessionError`/`TranscribeError` where they are the genuine source. Attribute conversions for touched files fold into this commit after the semantic edits.

Tests asserting the new error chains ship in the same commit; the component-scope check follows.

</step-5>

<step-6>

### Step 6: anyhow for test and build code, typed errors in remaining production

- Component: stringly-error-elimination

Eliminate the remaining ~32 stringly sites. Use `anyhow::Result` in test-only code (the `realtime/wire/server.rs` validators, `take/state/alignment_tests/adversaries.rs`, `tests/it/realtime_session.rs`, gateway `test_support.rs`, and the test-fixture-gated `main.rs` rendezvous), the build scripts, and the build-tool binaries, confirming first whether `anyhow` is already in `[workspace.dependencies]`. Fold typed errors into the crates' existing error enums for the remaining private production helpers: gateway `config_write.rs` and `dialect.rs`, gateway-local `confine.rs`, workshop-server `session/menu.rs`, promptforge-tool-picker `rank.rs`, build-ui `lib.rs`, and the two promptforge-lua LazyLock statics. Attribute conversions for touched files fold into this commit after the semantic edits.

This is the final step, so the full verification gate from the Testing Plan runs here: `cargo fmt --all --check`; `cargo clippy --all-targets --all-features -- -D warnings`, additionally on the newer CI stable because imports and doc comments were touched; `cargo test --locked --workspace --all-features` plus `cargo nextest run -p workshop -p workshop-server`; `cargo test --doc`; and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`. The commit lands only with the gate green and every acceptance behavior under the Functional Specification observable.

</step-6>

### Deferred Workstream

Five catalogs of findings deliberately not executed in this pass; each must be re-verified at the then-current HEAD before scheduling, because the audit judged commits historically and some entries have already self-healed.

#### F1. Documentation gaps

- Missing `# Examples` doctests on public items across promptforge-core `execute/config.rs`, promptforge-lua `vm.rs`, promptforge-tool-picker `model.rs`, gateway-stt `service.rs` and the test-fixture modules, gateway-stt-engine `decoder.rs` and `test_fixtures/native.rs`, shared-sidecar `shutdown.rs`/`stale.rs`/`health.rs`/`lock.rs`/`validated.rs`, workshop-server `gateway_binding.rs`/`serve.rs`/`menu.rs`/`session_agents.rs`, gateway `diagnostics.rs`, shared-loopback `lib.rs`, and gateway-config `config/stt.rs` with `accessors.rs`.
- Missing or variant-less `# Errors` on workshop-server `fixtures.rs` and `lib.rs` and gateway-stt `service.rs`.
- Third-person summary style in shared-sidecar `cancellation.rs` among others.

#### F2. Module and file structure

- Missing `//!` module docs across the gateway-stt `take/` and `realtime/` subtrees, gateway-stt `audio.rs`, gateway-stt-engine `test_fixtures/scenarios.rs`, and several workshop-server and gateway test modules.
- Fifteen files past the 500-line split rule, largest being gateway `src/lib.rs` at ~4858 lines, `profile_switch.rs` at 1997, gateway-logging `worker.rs` at ~1455, and build-workshop `main.rs` at 1319.
- Logic in the gateway-logging, gateway, and shared-loopback crate roots.
- `include!`-assembled test splits in workshop-server `tests/it/chat_gate.rs` and `realtime_relay.rs` and gateway `tests/it/realtime_stt.rs`.
- Mixed `mod.rs` versus `foo.rs` module style within gateway-stt.

#### F3. API design

- Bool flag parameters in gateway `commands.rs`, gateway-stt `segment.rs` and `segment/boundary.rs`, workshop `menu.rs`, shared-sidecar `health.rs`, gateway-logging `queue.rs`, gateway-stt `model.rs` and `realtime/wire/client.rs`, and the engine worker/model loaders.
- Clone-returning getters in workshop-server `app.rs` and `serve.rs`.
- Missing compile-time Send/Sync assertions in promptforge-core `input.rs` and gateway-logging `runtime.rs`/`writer.rs`.
- Missing `#[must_use]` on shared-sidecar's `GatewayInstanceLease`.
- Over-long combinator chains in promptforge-lua `protocol.rs` and gateway-logging `redact.rs`; the bool-producing match in workshop-server `catalog/chat.rs`.
- The `Cow` candidate in gateway-logging `redact.rs`; `{}` printing of anyhow errors in workshop `gateway.rs`.

#### F4. Test layout and hygiene

- Integration tests outside the single `tests/it/main.rs` binary in product-integration-tests, build-workshop, workshop, gateway-stt-engine, gateway-stt-backend-whisper, and gateway-transcribe.
- Unit tests in sibling `tests.rs` files across promptforge-lua, gateway-stt, and workshop-server.
- Bare `#[test]` functions at module scope in workshop-server `test_gateway/process.rs` and gateway-stt `test_fixtures/native.rs`.
- Hand-rolled temp dirs in gateway-logging `runtime.rs` and `worker.rs`; blocking `std::fs` and thread joins inside async tests in gateway-stt `tests/common/mod.rs`.
- Operation-style `expect` messages in gateway-transcribe `tests/native_whisper.rs`.
- The ~26 real-network async-test sleeps deferred by the recorded decision.
- Duplicated fixture helpers left behind when the `test-fixtures` feature was deleted.

#### F5. Tooling, CI, and policy

- The `rust-toolchain.toml` MSRV pin codified in AGENTS.md - a policy decision to resolve by unpinning or by amending the rulebook for application repos.
- `tokio::spawn` inside gateway-stt library code at `realtime/registry.rs` and `realtime/session/items.rs`.
- Import grouping outside the three-block rule across promptforge-lua, gateway, gateway-stt, gateway-logging, and workshop-server files; the free-function import in gateway-stt `realtime/wire/server.rs`.
- The `failed to` Display prefix in gateway `config_write.rs`.
- Build-script hygiene in workshop `build.rs` (boxed error instead of anyhow) and gateway `build.rs` (untested manifest-generation logic).

</execution-plan>
