---
name: Harness dependency rule debt
overview: "Close the one debt item the harness facade refactor (0f43e1b3..3e2a957d) left: the \"harness depends only on promptforge\" rule is stated as enforced, but the product-boundary check lets harness crates depend on build-* and other unaffiliated crates."
todos:
  - id: hf1-rule
    content: "DEBT-HF-1: add WORKSPACE_HACK constant and (Harness, Build) / (Harness, Unaffiliated except workspace-hack) arms in crates/build-xtask/src/product.rs; rewrite the harness module-doc bullet"
    status: pending
  - id: hf1-tests
    content: "DEBT-HF-1: add build-ui build-dependency and unaffiliated-crate fixtures to product-harness-tests.rs; run cargo test -p build-xtask, clippy, fmt"
    status: pending
isProject: false
---

# Harness dependency rule debt removal

<product-contract>

## Product Requirements

- Scope and target work:
  - Repository promptforge3, branch `vibe3`. Baseline `0f43e1b3`, endpoint and disposition ref `3e2a957d`. The worktree was clean and is excluded.
  - Target: the seven commits of the harness facade refactor, which are `80804125`, `c591baca`, `83ddebe5`, `4fa04a19`, `6527bf7a`, `a9516d62`, and `3e2a957d`. The refactor's plan is `vibe/2026-09-26-2-harness-facade.md`, and `vibe/archdoc.md` was also read as a design record.
  - Analysis read history, diffs, and source only. It ran no build, test, or doc command, so gate results in the target commit messages are unverified claims.
- Cleanup goals:
  - Make the product-boundary check enforce the harness dependency rule the repository states: outside its own family, a harness crate may depend only on `promptforge` and `workspace-hack`.
- Non-goals:
  - No change to any real dependency edge. No harness manifest names a `build-*` crate or an unaffiliated crate other than `workspace-hack` today.
  - No change to the promptforge family's rules. They list banned families, with no "only" claim.
  - No work on the exposed pre-existing cancellation debt (DEBT-HF-X1) unless the user expands the scope.
- Success criteria:
  - `cargo test -p build-xtask` reports a violation when a harness crate depends on any `build-*` crate, in any dependency table, or on an unaffiliated workspace crate other than `workspace-hack`. It still passes on the live workspace.
  - The `product.rs` module doc, `AGENTS.md` line 48 ("harness-* crates may depend only on `promptforge`"), and `AGENTS.md` line 83 (which says `cargo test -p build-xtask` enforces the matrix) all agree with the check.

## Functional Specification

### Debt Inventory

- DEBT-HF-1: the "harness depends only on promptforge" rule is stated as enforced, but harness dependencies on `build-*` crates get through. Classification: introduced.
  - Evidence:
    - `6527bf7a` narrowed `AGENTS.md` line 48 from a permissive list to "harness-* crates may depend only on `promptforge`".
    - `83ddebe5` and `a9516d62` wrote "may depend on: `promptforge` and container siblings only" into every `crates/harness-internal/*/src/lib.rs` invariant block and README.
    - `AGENTS.md` line 83 says `cargo test -p build-xtask` enforces the matrix.
  - Code facts at `3e2a957d`:
    - In `crates/build-xtask/src/product.rs`, `family()` maps `build-*` packages to `Family::Build`.
    - The `family_rule` match (lines 188-212) has harness arms only for `Workshop`, `Gateway`, and `Shared`.
    - The fallback (lines 213-228) fires only on edges into the promptforge or harness families.
    - Container privacy doesn't apply, because `build-*` crates sit at the `crates/` root.
    - `DEP_KINDS` scans `build-dependencies`, so the walker sees such an edge, but no rule rejects it.
    - The `build-ui` build-dependency pattern already exists in `crates/workshop/server/Cargo.toml` line 71 and `crates/gateway/config-ui/Cargo.toml` line 27.
  - Impact: a harness crate that copies that pattern passes `cargo test -p build-xtask` silently. That contradicts the stated rule, and it breaks the "built, published, or moved out with nothing but promptforge" property, because Cargo needs build-dependencies to build.
  - The same gap exists for unaffiliated crates. It is latent, since `workspace-hack` is the only unaffiliated workspace crate today.
  - Reversal cost: small. It's one match arm, one constant, a doc edit, and fixtures, all local to build-xtask, with no public, persisted, or wire change.
  - Target state: the check rejects the edge, and the docs and the check agree.
- Exposed pre-existing debt, not counted as debt added: DEBT-HF-X1. `harness::cancel` is documented as cancellation for harness session paths, but no harness code reads it.
  - Facts:
    - `crates/harness-internal/runner/src/cancel.rs` defines a task-local `CancelHandle`, and no session, runtime, or Workshop code calls `current`, `scope`, `maybe_scope`, `is_cancelled`, or `wait_cancelled`.
    - Session runs cancel through the engine's own flag, in `crates/harness-internal/sessions/src/lifecycle.rs`.
  - What was already there: at baseline, `crates/harness-api/src/cancel.rs` exported the same unwired module with the same summary line, and the engine doc already claimed the token "bridges to this flag".
  - What the target added: `83ddebe5` copied the "install with `scope` and cancel from a Ctrl-C task" guidance onto the published facade page `crates/harness/src/cancel.md`, and `a9516d62` kept "bridges to this flag" in `crates/promptforge-internal/types/src/cancel.rs` lines 10-15.
  - This is reported only, and is deferred below.
- Rejected candidates: 12.
  - Residual-but-acceptable (5):
    - The `harness-log` copies of the `shared-error-source` wrappers, a deliberate decision with no shape tie to the shared crate.
    - The `harness-sessions` dev-dependency on itself, a documented workaround with a recorded fallback.
    - A leak-guard live test that keeps an engine-only name.
    - `boundary_breach` growing by about 9 lines.
    - The facade not being under `cargo xtask api`, explicitly deferred, with no present unlinked type.
  - Weak or speculative (6):
    - The wrapper docs saying the error surface "names no engine type".
    - No test that a default build lacks `Harness::log`, which the `cfg` and the leak guard already cover.
    - `family_of` identifying families by array position.
    - The container name spelled in four places in one crate, where a partial rename fails loudly.
    - The import renames across Workshop.
    - Feature unification in workspace-wide test builds.
  - False (1): that the leak-guard exemption weakened promptforge coverage. It is strictly stricter.

</product-contract>
<implementation-contract>

## Technical Design

- DEBT-HF-1, check behavior: in `crates/build-xtask/src/product.rs`, the `family_rule` match gains two harness arms.
  - `(Family::Harness, Family::Build)` is always a breach.
  - `(Family::Harness, Family::Unaffiliated)` is a breach unless the dependency is `workspace-hack`, named by a new `WORKSPACE_HACK` constant beside `PUBLIC_HARNESS` (line 140).
  - Both messages name what's required versus what was found, matching the existing messages. For example: "harness crates must not depend on build-* crates; outside their family they may name only promptforge and workspace-hack". The unaffiliated arm uses the same form.
- The arms cover every dependency table the walker already reads: normal, dev, build, and target-specific (`DEP_KINDS`).
- The `crates/harness` facade and the `crates/harness-internal/*` crates depend only on family crates, `promptforge`, `workspace-hack`, and third-party crates, so the live workspace stays clean.
- DEBT-HF-1, contract text: the module doc bullet in `product.rs` (lines 14-17) changes. Today it says "`promptforge` is the one product crate they may name". It will say that outside their family, harness crates may name only `promptforge` and `workspace-hack`, which rules out `build-*` and other unaffiliated crates.
  - `AGENTS.md` lines 48 and 83, and the invariant blocks and READMEs, already state the rule and need no change.
  - The container-privacy comment calling `build-*` "meta tooling" (line 165) stays. It concerns `build-*` crates depending into containers, which is unchanged.
- No module, public interface, data, protocol, security, or lifecycle change.

</implementation-contract>
<verification-contract>

## Testing Plan

- Focused, DEBT-HF-1: new fixtures in `crates/build-xtask/src/product-harness-tests.rs`, which is 106 lines at `3e2a957d`. They use the existing `product-test-support.rs` helpers.
  - A harness container crate, such as `crates/harness-internal/runner`, listing `build-ui` under `[build-dependencies]` yields exactly one violation, carrying the build-crate message.
  - A harness crate depending on an unaffiliated workspace crate other than `workspace-hack` yields exactly one violation, carrying the unaffiliated message.
  - The existing `a_harness_crate_depending_on_workspace_hack_passes` still passes.
- Regression: the other existing product tests keep passing unchanged. These are the gateway, shared, and outside-into-harness fixtures, and the promptforge-family fixtures, including any promptforge crate depending on a `build-*` crate.
- Architecture: the live-workspace product-boundary test passes at the new endpoint, which confirms no current harness manifest trips the new arms.
- Exit checks:
  - `cargo test -p build-xtask`
  - `cargo clippy -p build-xtask --all-targets --all-features -- -D warnings`
  - `cargo fmt --all --check`

</verification-contract>
<decision-record>

## Decision Record

- Reversible decisions and consequences:
  - DEBT-HF-1: cover unaffiliated crates in the same change, with `workspace-hack` as the one named exception.
    - Rationale: "only promptforge" rules them out too, and the cost is one arm.
    - Tradeoff: adding a new unaffiliated crate that harness crates should use requires extending the exception.
- User-resolved architecture choices:
  - DEBT-HF-1: close the debt by tightening the product-boundary check, rather than by loosening the rule text to the three banned families.
    - This approves extending the dependency-topology check with harness arms for `build-*` crates and unaffiliated crates.
    - User's words: "I like tightening".
    - Rationale: the rule, the invariant blocks, and the stated goal all say "only promptforge". The goal of building or moving the harness with nothing but `promptforge` fails if a harness crate needs a `build-*` build-dependency.
    - Tradeoff: a future harness build script can't use `build-ui` without an explicit exception.
    - Verification: the fixtures in the Testing Plan.
  - From the refactor this plan follows: "harness only depends on promptforge (not gateway or shared or workshop)".
  - From the refactor this plan follows: "the aim is that the harness can be built, published, or moved out with nothing but promptforge". The user confirmed this aim.
- Rejected alternatives:
  - Narrowing `AGENTS.md` line 48, the invariant blocks, and the READMEs to "never on gateway, shared, or workshop crates". The user chose tightening over this, and it would contradict the stated aim. Revisit if harness build scripts come to need the `build-*` helpers.
  - A `Build`-only arm without the unaffiliated arm. This leaves a latent hole in the same contract for almost no saving.
- Assumptions and risks:
  - The checker's own comment calls `build-*` crates "meta tooling, not a product family". That could be read as intending to allow them. The user chose tightening, which settles that reading. The comment stays, because it concerns `build-*` crates depending into containers.
  - The target's gate results (build, tests, site) were not re-run during analysis.
  - The commit history before the target was not read in full, so an earlier corrective episode on the same causes is not ruled out.

### Deferred and Out of Scope

- Deferred: DEBT-HF-X1, the `harness::cancel` guidance that no harness code honors. Revisit if the user expands the scope. The options are:
  - Correct only the docs: `crates/harness/src/cancel.md`, `crates/harness/src/lib.md`, and `crates/promptforge-internal/types/src/cancel.rs` lines 10-15.
  - Remove `pub mod cancel` from the facade. This is a public API removal, with no workspace callers.
  - Wire `cancel::current()` into `Harness::launch` so a scoped handle closes the launched session. This is a behavior change.
- Out of scope: the promptforge family's matching missing `(Promptforge, Build)` arm. Its stated rule lists banned families only, so nothing it states is contradicted.
- Out of scope: the 12 rejected candidates in the Debt Inventory.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: no standalone workspace build; the compile gate is the clippy run (AGENTS.md forbids a separate `cargo check --workspace`). Binaries as CI builds them: `cargo build --locked -p gateway` and `cargo build --locked -p workshop`. Extra build-shape gate: `cargo check -p gateway --no-default-features`.
- Focused test command pattern: `cargo nextest run --locked -p <package> --all-features <test-name-filter>` (integration target: add `--test it`).
- Component test command pattern: `cargo nextest run --locked -p <package> --all-features`, then `cargo test -p <package> --all-features --doc`; for the boundary and structural harness, `cargo test -p build-xtask`.
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server -p workshop-server-api`. SPA: `npm test` in `crates/workshop/ui` (after `npm ci`).
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --exclude workshop-server-api --all-targets --all-features -- -D warnings` plus `cargo clippy -p workshop -p workshop-server -p workshop-server-api --all-targets -- -D warnings`; `cargo deny check` in CI and pre-push.
- Formatter check command: `cargo fmt --all --check`.
- Docs command: `RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server --exclude workshop-server-api`, plus `RUSTDOCFLAGS="-D warnings" cargo doc -p promptforge --no-deps`; facade surface `cargo +<pinned nightly> xtask api --check` (nightly named in `crates/build-xtask/src/api/toolchain.rs`); user guide `cargo xtask site --books-only`.
- Test placement and naming conventions: unit tests live in a sibling `<module>-tests.rs` wired with `#[cfg(test)] #[path = "<module>-tests.rs"] mod tests;` (three or more test files become a `<module>/tests/` subdirectory); integration tests are one `tests/it/` target with `main.rs` plus topic modules and a `support.rs`, run with `--test it`; test names are descriptive snake_case sentences; SPA tests are `test/**/*.mjs` and `src/**/*.test.mjs` under `node --test`; structural tests sit in `crates/build-xtask/src/*-tests.rs` (for example `product-harness-tests.rs`, `harness_bans-tests.rs`, `product-container-tests.rs`).
- Directory map:
  - `crates/` root: the public layer - facades `promptforge` and `harness`, `gateway-api-types`, `gateway-api-discovery`, `shared-*` (`shared-error-source`, `shared-loopback`, `shared-ui`), `build-*` meta tooling (`build-xtask`, `build-workshop`, `build-ui`, `build-user-guide`, `build-llama-cuda`), and `workspace-hack` (hakari).
  - `crates/promptforge-internal/`: private engine family - engine, lua, model-client, parser, store, types, vfs (`promptforge-*`).
  - `crates/harness-internal/`: private harness family - capabilities, log, models, runner, sessions, web, web-search, webfetch (`harness-*`).
  - `crates/gateway/`: private gateway family - app (package `gateway`), cloud-providers, config, config-ui, local, logging, progress, protocol, routing, web-search, and the nested `stt/` subsystem (only `gateway-stt` is family-visible).
  - `crates/workshop/`: private workshop family - desktop (package `workshop`), server, server-api, gateway, menu, protocol, registry, status, support, user-state, workspace, and the TypeScript SPA in `ui/`.
  - `guide/`: user guide books and site sources; `prompts/`: example prompt programs; `tools/`: Node staging and live-test scripts; `vibe/`: plans, `archdoc.md`, and dependency notes; `.github/workflows/`: CI and release; `.githooks/`: pre-commit and pre-push; `.config/`: nextest and hakari config.
- Component boundaries: executor (`promptforge-engine`) depends on store, Lua VM boundary, and shared substrate, and performs no I/O; `harness` is the executor's only production host and its `harness-*` crates may depend only on `promptforge` (never gateway, shared, or workshop crates), while outside crates reach it only through the `harness` facade and `promptforge-*`/`gateway-*` crates must not depend on harness; gateway depends only on the shared substrate and never on promptforge, workshop, or harness; workshop crates may name `harness`, `promptforge`, and the gateway public pair, never other gateway crates, and the desktop app sees the server only through `workshop-server-api`; outside crates reach promptforge only through the `promptforge` facade; store depends on VFS, and VFS and shared substrate depend on nothing product-side. A crate in a family container may depend only on `crates/` root crates and its own siblings, the rules bind normal, dev, build, and target dependencies, and `cargo test -p build-xtask` enforces them.
- Conventions summary: Rust edition 2024 workspace on the stable channel pinned by `rust-toolchain.toml`, every crate inheriting `edition.workspace`, with rustfmt, clippy `-D warnings`, cargo-deny, and hakari; every `workshop-*` and `harness-*` `lib.rs` opens with a `//!` doc holding a `## Invariants` marker, and files in marked crates stay at or under 500 lines; source directories are flat, with one or two related files as kebab siblings (`foo-bar.rs` plus `#[path]`) and three or more rehydrated into a subdirectory; facades are single-item re-exports grouped in documented role modules; structural checks need explicit user approval; behavior changes ship with tests; error messages are written for model consumption (required versus actual); comments explain only non-obvious constraints and cite upstream issue URLs for workarounds; replayed JSON round-trips exactly with sorted keys; SPA CSS sits beside its TypeScript and uses `--ws-*` tokens with no `localStorage`.

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Enforce the harness-only-promptforge dependency rule (DEBT-HF-1) [completed]

- Component: `none`
- Depends on: nothing. Touches only `crates/build-xtask`. No manifest, public interface, or real dependency edge changes.
- Artifacts:
  - `crates/build-xtask/src/product.rs`
    - New constant `WORKSPACE_HACK: &str = "workspace-hack"`, placed beside `PUBLIC_HARNESS` (line 140).
    - Function `boundary_breach`, `family_rule` match (lines 188-212): two new arms placed next to the existing `(Family::Harness, Family::Workshop | Gateway | Shared)` arms.
      - `(Family::Harness, Family::Build)`, always a breach, message: "harness crates must not depend on build-* crates; outside their family they may name only promptforge and workspace-hack".
      - `(Family::Harness, Family::Unaffiliated) if dep.package != WORKSPACE_HACK`, message: "harness crates must not depend on unaffiliated crates other than workspace-hack; outside their family they may name only promptforge and workspace-hack".
      - `boundary_breach` receives `dep` as a resolved workspace `CrateInfo`, so third-party crates never reach these arms. The arms apply to every table in `DEP_KINDS` (normal, dev, build) plus target-specific tables, because `manifest_dependencies` already collects them all.
    - Module doc harness bullet (lines 14-17): replace "`promptforge` is the one product crate they may name" with the complete rule: outside their family, harness crates may name only `promptforge` and `workspace-hack`, which rules out `build-*` and other unaffiliated crates.
    - Leave the container-privacy comment calling `build-*` "meta tooling" (line 165) unchanged. It concerns `build-*` crates depending into containers.
  - `crates/build-xtask/src/product-harness-tests.rs` (106 lines today), using `write_crate` and `product_boundary_violations` from `product-test-support.rs`:
    - `a_harness_crate_build_depending_on_a_build_crate_is_reported`: `harness-internal/runner` (package `harness-runner`) lists `build-ui` under `[build-dependencies]`, plus a `build-ui` crate at the `crates/` root. Assert exactly one violation, starting with `harness-runner depends on build-ui:` and containing the build-crate message.
    - `a_harness_crate_depending_on_an_unaffiliated_crate_is_reported`: a harness crate lists a fixture unaffiliated root crate (for example `some-tool`) under `[dependencies]`. Assert exactly one violation containing the unaffiliated message.
- Not changed: `AGENTS.md` lines 48 and 83, the `crates/harness-internal/*` invariant blocks and READMEs (they already state the rule), and the promptforge family's rules, including the absent `(Promptforge, Build)` arm.
- Tests covering this step:
  - The two new fixtures above.
  - Existing `a_harness_crate_depending_on_workspace_hack_passes` still passes, proving the exception.
  - All other existing product tests pass unchanged: gateway, shared, outside-into-harness, build-past-facade, container, and promptforge-family fixtures.
  - The live-workspace product-boundary test passes, proving no current harness manifest trips the new arms.
- Verification:
  - `cargo test -p build-xtask`
  - `cargo clippy -p build-xtask --all-targets --all-features -- -D warnings`
  - `cargo fmt --all --check`
- Commit: one commit containing the `product.rs` change and the two fixtures.
- Done when: all three checks pass, each new fixture reports exactly one violation with its message, and the `product.rs` module doc, `AGENTS.md` line 48, and `AGENTS.md` line 83 agree with the check.

</step-1>

</execution-plan>
