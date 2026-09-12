---
name: Dependency rules, PR 35 code fixes, and the VFS observation hook
overview: Land the AGENTS.md dependency-rule revisions, rename shared-protocol to gateway-protocol, extend the architecture test to enforce the shared-* rule, fix the two code failures CI exposed on PR
todos:
  - id: land-rules
    content: Commit the AGENTS.md rule revisions together with the two design/ deletions
    status: pending
  - id: rename-crate
    content: Rename shared-protocol to gateway-protocol (crate, manifests, imports, AGENTS.md, lockfile)
    status: pending
  - id: extend-arch-test
    content: "Extend architecture.rs: Shared set + fifth rule, workshop prefix fix, fixture coverage"
    status: pending
  - id: fix-unix-clippy
    content: "Add #[expect(unnecessary_wraps)] to the cfg(unix) mode_of in shared-vfs host.rs"
    status: pending
  - id: fix-writerace
    content: "dispatch_store: drop the access clone before sending the answer"
    status: pending
  - id: vfs-hook
    content: "shared-vfs: op-observation sink on the handle plus mandatory acquire identity label"
    status: pending
  - id: verify
    content: "Verify: local gates plus PR #35 CI green across clippy, test, native-whisper"
    status: pending
isProject: false
---

# Dependency rules, PR #35 code fixes, and the VFS observation hook

<product-contract>

## Product Requirements

The promptforge repository's AGENTS.md gained a dependency-rule block whose shared-* rule one crate violates today and no check enforces. The first CI run on PR #35 (cppalliance/promptforge, head vinniefalco:master) then exposed three failures: a unix-only clippy lint invisible on Windows, a store claim-release race exposed by Linux scheduling, and a self-hosted runner environment fault on native-whisper (repaired on the machine and green since). This plan lands the rule revisions, renames the violating crate, extends the existing architecture test to enforce the shared-* rule, fixes the two code failures, and adds the VFS operation-observation hook to shared-vfs (the seam only - the event log, Lua query, and enrichment policies are deferred).

- Problem and users: `shared-protocol` depends on `gateway-config`, violating the new AGENTS.md rule "Shared crates must not depend on any product crates" and the crate's own AGENTS.md rule; the architecture test's Workshop classification is a hardcoded pair while the rule says `workshop-*`. Users are the maintainers and every CI consumer of the PR.
- Goals: the written dependency rules are true of the code and enforced by CI.
- Non-goals: no merge of `shared-protocol` into another crate; no new shared types crate; no `dtolnay/rust-toolchain` workflow step (the runner repair holds; the operator removed it); no VFS event log, Lua exposure, or enrichment policy (deferred - only the observation hook itself lands); no edits to historical `vibe/*.md` records.
- Success criteria: no `shared-*` crate depends on a product crate; `cargo test -p gateway-stt --test it architecture` enforces all five rules including the Shared rule and prefix-based Workshop classification; the unix clippy lint is expected-out; the WriteRace test passes a 100-iteration local stress loop; a sink installed on a VFS handle receives op, canonical path, and identity label for every admitted operation and never for a denied one; PR #35 CI is green across `clippy`, `test`, and `native-whisper`.
- Constraints: the initial commit contains exactly the `AGENTS.md` revisions and the two `design/` deletions and nothing else; the `Cargo.lock` regeneration after the rename is minimal (rename only, no version changes); the workspace lint policy wants `#[expect(..., reason = "...")]`, never `#[allow(...)]`.
- Open questions: None

## Functional Specification

The work is rule landing, one crate rename, one test extension, two targeted code fixes, and one new observation seam. Two behavior-adjacent changes: store claims release inside `run()` before it returns instead of at the blocking pool's leisure, and a VFS handle with an installed sink reports every admitted operation to it. Nothing changes what any operation does.

- Actors and workflows: the executing agent applies the work items; CI on PR #35 validates the result.
- Inputs and outputs: inputs are the working tree (which carries the uncommitted AGENTS.md revisions and the two `design/` deletions), the `shared-protocol` crate and its four consumers, and the architecture test; outputs are the same trees corrected, plus the initial commit.
- States and validation: after the rename, the workspace has no `shared-protocol` references outside `vibe/`; the architecture test's rule table holds five rules.
- Errors and recovery: the `dispatch_store` fix changes when claims release, never whether an operation succeeds; the clippy fix changes no behavior; the observation sink is fire-and-forget and never consulted for a decision.
- Security and privacy behavior: the WriteRace fix tightens the determinism boundary by bounding claim release to the run's lifetime; the observation sink sees op kind, canonical path, and identity label - no content - and is installed only by the host that owns the handle.
- Acceptance criteria: the success criteria above, with per-item verification in the Testing Plan.

</product-contract>
<implementation-contract>

## Technical Design

The cross-module design is the crate rename, which changes no dependency direction - `gateway-protocol` sits where `shared-protocol` sat, consumed by the same four gateway crates - and the observation hook, which extends the shared-vfs public surface. The architecture test extension adds a package set and a rule row to an existing approved structural check. The two CI code fixes are local: a cfg-gated lint expectation and an ordering constraint in one closure.

- Architecture: `shared-protocol` becomes `gateway-protocol`; its four consumers (`gateway`, `gateway-local`, `gateway-routing`, `gateway-web-search`) are all gateway crates, so no product boundary moves. The accepted consequence: the wire vocabulary is gateway-owned, so `promptforge-model-client`'s duplicate `ThinkingMode` can never converge onto it (promptforge crates may not depend on gateway crates) - the duplication is permanent by design. The observation hook lives in `shared-vfs` at the capability layer, not the backend layer, because backends cannot see identity: `acquire` mints the `ExecId` and now also carries the caller-supplied label.
- Modules and interfaces: `crates/shared-protocol/` moves to `crates/gateway-protocol/` with manifest, import, and lockfile renames; `crates/gateway-stt/tests/it/architecture.rs` gains `PackageSet::Shared` (prefix `shared-`), a combined `AnyProduct` forbidden set, a fifth rule (Shared cannot depend on any product), and prefix-based Workshop classification; `crates/shared-vfs/src/host.rs`'s `#[cfg(unix)]` `mode_of` gains an `#[expect]`; `crates/promptforge-core/src/execute/scheduler.rs`'s `dispatch_store` blocking closure drops its `Arc<Access>` clone before sending the answer; `crates/shared-vfs/src/handle.rs` gains the op sink and the labeled acquisition.
- File and public API changes: `VfsRef::acquire` and `Access::spawn` gain a mandatory caller-supplied `Origin` (a `#[non_exhaustive]` struct: `label`, `file`, `line`, all mandatory - `Origin::new(label)` is `#[track_caller]` and stamps the Rust call site via `Location::caller()`, `Origin::at(label, file, line)` sets an explicit position, which the executor and agent use to substitute the prompt's position for the Rust one, so every event's position is present and is the most precise thing the caller knows; claims still key on the internal `ExecId`; the `Origin` is for observability); `VfsRef::builder()` gains the op-sink installation. The sink fires on every admitted operation - after policy and claims pass, before the backend executes - with the op kind, the canonical path, and the origin; fire-and-forget, no outcome, and a policy-denied operation never fires. Reads, writes, and enumeration (list, glob) all fire. The sink is `Send + Sync` and must be cheap: store ops fire it from the blocking pool. Module docs name the deferred consumers (the bounded event log, the Lua pull query, enrichment policies).
- Data, persistence, failure, security, and privacy constraints: no persisted or wire format changes; the claims-release ordering is a lifecycle constraint - the drop must precede the send, and the fix carries a comment naming that constraint per the repository comment rule.

</implementation-contract>
<verification-contract>

## Testing Plan

Each work item carries a focused check; the WriteRace fix adds a stress loop; the final gate is PR #35 CI. No new product behavior means no new product tests beyond the architecture fixtures and the stress regression.

- Unit: the architecture test's adversarial fixtures extended to trigger the new Shared rule and a `workshop-`-prefixed violator; the existing fixture tests keep passing. New shared-vfs tests: a sink receives events in order with op, path, and label; no sink means no events; a policy-denied operation never fires the sink.
- Integration and end-to-end: `cargo test -p gateway-stt --test it architecture` against the post-rename workspace; builds and test suites of the five rename-touched crates; the workspace builds with the new `acquire` signature (every call site labels itself).
- Regression, security, and performance: `cargo nextest run -p promptforge-core jump_inside_a_fanout_arm_to_a_silent_chain_returns_empty_text` in a 100-iteration loop, every iteration green; `rustup target add x86_64-unknown-linux-gnu` then `cargo clippy -p shared-vfs --target x86_64-unknown-linux-gnu --all-targets -- -D warnings` to see the unix-only lint without CI.
- Exit criteria: `git show --stat` of the initial commit lists exactly `AGENTS.md` and the two `design/` deletions; a repo grep for `shared[-_]protocol` outside `vibe/` finds nothing; local build, clippy, and test gates green for the touched crates; PR #35 CI green across `clippy`, `test`, and `native-whisper`.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Rename `shared-protocol` to `gateway-protocol` rather than merging: all four consumers are gateway crates, and merge has no viable home (`gateway` would cycle through its optional `gateway-local` feature edge; `gateway-routing` would force an unrelated `gateway-web-search` edge; `gateway-config`'s lean charter forbids the reqwest/tokio upstream client). The user's words: "if gateway is the only product consuming shared-protocol then shared-protocol needs to be either renamed to gateway-{something} or merged into one of the gateway-* crates".
  - The two `design/` deletions ride the initial commit with the AGENTS.md revisions. The user's words: "the plan must merge the design deletions into the intiial commit".
  - The PR #35 CI failures split by runner: `native-whisper` (self-hosted) was runner configuration - repaired on the machine and green since. `clippy` and `test` run on GitHub-hosted `ubuntu-latest` (`ci.yml`), so they are repo code problems: a unix-only clippy lint in `shared-vfs/src/host.rs` and a store claim-release race in `dispatch_store`. Both fixes are in scope. The user's call, after the runs-on evidence: restore both fixes.
  - The VFS observation hook lands, but only the hook: the seam, the identity label, and its documentation are in scope now; the event log, the Lua query, and all enrichment policy are deferred. The user's words: "I want the hook in place but I do not want to build out the rest of the Rust and Lua".
  - The AGENTS.md rule revisions themselves (Principles, Roles, Structure, Engineering sections, including the shared-* dependency rule and the all-kinds binding) are the operator's own edits; this plan lands and enforces them.
- Rejected alternatives:
  - A new lean shared types crate holding `Capabilities`, `ModelKind`, `ThinkingMode`, and `Secret`: correct layering but a new facility for one edge. Revisit if a consumer outside the gateway product ever needs the wire vocabulary.
  - Splitting `shared-protocol` (wire types stay shared, upstream client moves to a gateway crate): the most faithful to the crate's original name, but a much larger reshuffle than the rulebreak requires. Revisit if the upstream abstraction gains a non-gateway consumer.
  - Merging into `gateway`, `gateway-routing`, or `gateway-config`: rejected for the cycle, the unrelated edge, and the lean charter respectively, as above.
- Assumptions, risks, and notes:
  - The runner repair is already applied to the self-hosted Windows runner (the operator's machine) and proven: the native-whisper job went green on the re-run after the fix. What was applied: NETWORK SERVICE has read-and-execute on `C:\Users\Vinnie\.cargo\bin` and the stable toolchain directory; both runner roots (`D:\actions-runner`, `C:\actions-runner`) carry a `.env` file whose `PATH=` line is the full current machine PATH plus the service account's WindowsApps entry, which the runner's listener reads at startup (`LoadAndSetEnv`); both runner services were restarted after each change. Windows services inherit the SCM's boot-time environment, so the `.env` bootstrap - not a service restart - is what refreshes a runner's PATH.
  - rust-cache's `rustc -vV` validation failure logs `##[error]` but does not fail its step (its `reportError` only annotates), so a green cache step never proved the toolchain resolved; only the run steps did.
  - The WriteRace mechanism is grounded in code reading of `dispatch_store`: the `spawn_blocking` closure's `Arc<Access>` clone outlives the answer send, and claims release only when the last clone drops. The fix bounds release to chain teardown inside `run()`.
  - The unix clippy lint is invisible on Windows by `cfg`; the Linux-target clippy check in the Testing Plan is the local proxy.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build: `cargo build --locked` (default member is the gateway only); desktop app is explicit: `cargo build --locked -p workshop`. UI bundles need `npm ci --prefix crates/workshop-server/ui` and `npm ci --prefix crates/gateway-config-ui/ui` first.
- Focused test: `cargo nextest run -p <crate> <filter>` or `cargo test -p <crate> <filter>`; single integration target: `cargo test -p <crate> --test it <filter>`.
- Component test: `cargo nextest run -p <crate> --all-features` (doctests separately: `cargo test -p <crate> --doc`).
- Full suite: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates run on Windows: `cargo nextest run --locked -p workshop -p workshop-server` plus `cargo test --doc -p workshop -p workshop-server`.
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`. Supply chain: `cargo deny check`, `cargo audit`.
- Formatter check: `cargo fmt --all --check`.
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS: -D warnings`; user guide: `mdbook build guide`.
- Test placement and naming: unit tests live in `src` beside the code; integration tests live in per-crate `tests/`, most often as a single `it` target (`tests/it/`) with shared `common`/`fixtures` helpers; cross-product end-to-end tests live in the `product-integration-tests` crate. Test names are snake_case sentences describing behavior (e.g. `a_process_lifetime_lease_recovers_after_its_owner_is_terminated`). Nextest is configured in `.config/nextest.toml` with a `heavy` test group (max 2 threads) for `promptforge-tool-picker`, `gateway-stt`, and `gateway-stt-backend-whisper`.
- Directory map: `crates/` holds all workspace members (`crates/*` glob; `shared-ui` excluded, it is a TypeScript+CSS package); `guide/` is the mdBook user guide; `prompts/` prompt pipelines; `tools/` Node helper scripts (sidecar staging, TTS live checks); `vibe/` architecture docs (`archdoc.md`); `images/` assets; `local/` local config; `.config/` nextest config; `.github/workflows/` CI; `target/` and `target-msrv/` build output.
- Component boundaries: three products plus shared substrate, per `AGENTS.md` and `vibe/archdoc.md`. `promptforge-*` crates (executor, parser, Lua boundary, tools, store, vfs policy) must not depend on gateway or workshop crates; `gateway-*` crates (routing, config, STT, local inference) must not depend on promptforge or workshop crates; `workshop-*` crates (Tauri shell, in-process server) must not depend on gateway crates; `shared-*` crates (protocol, vfs, loopback, progress, sidecar, ui) hold the public API surface and depend on no product crates; `build-*` crates build specific outputs. Dependency rules bind normal, dev, build, and target-specific dependencies.
- Conventions summary: Rust 2024 edition workspace, BSL-1.0; `unsafe_code` forbidden at workspace level (explicitly owned FFI boundaries excepted, e.g. `gateway-whisper-ffi`), `unwrap_used`/`expect_used` denied, clippy `all` denied and `pedantic` warn; behavior tests ship with behavior changes in the same change; structural enforcement (parsers, snapshots, allowlists, topology checks) requires explicit user approval; features gate real constraints (toolchain, native builds), not product shape; library/serve paths return failures instead of exiting; long-running work reports through `shared-progress`; comments cite upstream issue URLs for workarounds; no build step may write into the repository (CI enforces a clean tree).

</project-survey>
<execution-plan>

## Execution Instructions

Components in dependency order: rule-landing first (the initial-commit constraint requires it to precede every other commit); rename-and-enforcement next (the architecture test's new Shared rule only passes against the post-rename workspace, so the rename and the test extension land as one coupled commit); ci-code-fixes and vfs-observation-hook after (independent of each other, fixes first so the hook lands on a clean tree). Per-step verification below is the gating; the run's close-out confirms PR #35 CI green across `clippy`, `test`, and `native-whisper`.

<step-1>

### Step 1: Land the dependency-rule revisions [completed]

- Component: rule-landing

Commit exactly the working tree's `AGENTS.md` revisions together with the two `design/` deletions (`design/design-gateway-tts-phase-1.md`, `design/note-gateway-tts-phase-1-verification.md`, the operator's file move to `promptforge-design/`) and nothing else. Verification: `git show --stat` of the commit lists exactly those three paths.

</step-1>

<step-2>

### Step 2: Rename shared-protocol to gateway-protocol and enforce the Shared rule [completed]

- Component: rename-and-enforcement

One coupled commit - the new Shared rule fails against the pre-rename workspace, so the rename and the test extension land together. Rename: `git mv crates/shared-protocol crates/gateway-protocol`; rename the package in `crates/gateway-protocol/Cargo.toml` (drop the product qualifier from the description, keep `publish = false`); rename the root `Cargo.toml` `workspace.dependencies` entry and the four consumer manifests (`gateway`, `gateway-local`, `gateway-routing`, `gateway-web-search`); change `shared_protocol::` to `gateway_protocol::` in about 15 source files (`gateway/src/{profile_switch,lib,error,hf}.rs`, `gateway-routing/src/model.rs`, `gateway-local/src/{runtime,upstream,lib,dialect}.rs` plus `server/tests.rs` and `server/support.rs`, `gateway-web-search/src/{brave,error,service}.rs`); reword the crate's `AGENTS.md` (remove the "no dependency points back into Gateway code" rule, restate the purpose as a gateway crate) and its `README.md` name reference; check `tools/document.md`'s mention; regenerate `Cargo.lock` minimally (rename only, no version changes). Enforcement: in `crates/gateway-stt/tests/it/architecture.rs` (the test behind CI's "Check product dependency boundaries" step), make `PackageSet::Workshop` prefix-based (`package == "workshop" || package.starts_with("workshop-")`); add `PackageSet::Shared` (`starts_with("shared-")`); add a fifth rule forbidding Shared from depending on any product set (a combined `AnyProduct` forbidden set or three rule rows); extend the adversarial fixtures to trigger the new Shared rule and a `workshop-`-prefixed violator. Verification: build, clippy, and test green for the five rename-touched crates; a repo grep for `shared[-_]protocol` outside `vibe/` finds nothing; the fixture tests pass and `cargo test -p gateway-stt --test it architecture` passes against the post-rename workspace.

</step-2>

<step-3>

### Step 3: Fix the two PR #35 code failures [completed]

- Component: ci-code-fixes

Both fixes are tiny, share the PR #35 CI provenance (the GitHub-hosted `ubuntu-latest` `clippy` and `test` jobs), and land as one commit. In `crates/shared-vfs/src/host.rs`, add `#[expect(clippy::unnecessary_wraps, reason = "the not(unix) variant returns None; the Option unifies the platform signatures")]` to the `#[cfg(unix)]` variant of `mode_of` (around line 252). In `crates/promptforge-core/src/execute/scheduler.rs`'s `dispatch_store` (around line 1780), drop the blocking closure's `Arc<Access>` clone after the op and its observation and before `tx.send(...)`, with a comment naming the ordering constraint per the repository comment rule; the fix changes when claims release, never whether an operation succeeds. Verification: `rustup target add x86_64-unknown-linux-gnu`, then `cargo clippy -p shared-vfs --target x86_64-unknown-linux-gnu --all-targets -- -D warnings` (check-only, no linker needed); `cargo nextest run -p promptforge-core jump_inside_a_fanout_arm_to_a_silent_chain_returns_empty_text` in a 100-iteration loop, every iteration green. These are the two fixes behind PR #35's `clippy` and `test` job failures.

</step-3>

<step-4>

### Step 4: Add the VFS operation-observation hook [completed]

- Component: vfs-observation-hook

One seam, one commit. In `crates/shared-vfs/`: add `Origin` (a `#[non_exhaustive]` struct: `label`, `file`, `line`, all mandatory - `Origin::new(label)` is `#[track_caller]` and stamps the Rust call site via `Location::caller()`, `Origin::at(label, file, line)` sets an explicit position); `VfsRef::acquire` and `Access::spawn` gain a mandatory caller-supplied `Origin` (claims still key on the internal `ExecId`; the `Origin` is for observability); `VfsRef::builder()` gains the op-sink installation; the handle (`crates/shared-vfs/src/handle.rs`) fires the sink on every admitted operation - after policy and claims pass, before the backend executes - with the op kind, the canonical path, and the origin; fire-and-forget, no outcome, and a policy-denied operation never fires. Reads, writes, and enumeration (list, glob) all fire. The sink is `Send + Sync` and must be cheap: store ops fire it from the blocking pool. Update every call site: the executor and agent call `Origin::at` with the section name and the prompt's source position (all inputs already exist - the section VM is tagged with the section name, every compiled chunk carries its absolute `source_line`, sections carry spans, and the prompt carries its name); host code like the mount probe and tests call `Origin::new` and get the Rust call site for free. Module docs document the seam and name the deferred consumers (the bounded event log, the Lua pull query, enrichment policies); doc comments on `Origin::new` and `Origin::at` carry the most-specific-label guidance at the point of use (a section name for a chain, a tool id for a tool, a fixture name for a test - never a generic label when a specific one exists), and `crates/shared-vfs/AGENTS.md` carries it as a rule so agents working in the crate get it as instructions, not just docs. Verification: new shared-vfs tests - a sink receives events in order with op, path, and label; `Origin::new` stamps the caller's file and line; no sink means no events; a policy-denied operation never fires the sink - plus the shared-vfs and promptforge-vfs suites green and the workspace building with the new `acquire` signature.

</step-4>

Deferred and out of scope: historical `vibe/*.md` references to `shared-protocol` (dated records); the PR #35 runner configuration repair (operator's machine, already applied and proven green). Deferred above the VFS observation hook (which lands in this plan): the bounded run-scoped event log, the pull-based Lua query (the agent asks; the host never pushes), and every enrichment policy - deferred because context enrichment is per-host policy (a UI coding agent wants open windows and touched files; a headless agent has no windows; a report run wants nothing).

</execution-plan>
