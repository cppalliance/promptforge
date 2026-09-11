---
name: Unify toolchain and fix CI
overview: Remove the MSRV/local-vs-CI toolchain split so local builds and CI use the same stable Rust, and fix the three CI failures the skew caused.
todos:
  - id: fix-ci-failures
    content: Fix audio.rs:317 parens, bridge.rs expect lints, decode.rs flaky test
    status: pending
  - id: unify-toolchain-files
    content: rust-toolchain.toml to stable, remove rust-version from workspace and all 34 crates
    status: pending
  - id: simplify-ci-yml
    content: Remove RUSTUP_TOOLCHAIN overrides and msrv job from ci.yml
    status: pending
  - id: simplify-other-workflows
    content: Remove MSRV env and validation step from stt-miri.yml, remove from release-workshop.yml
    status: pending
  - id: docs-and-cleanup
    content: Remove skew bullet from AGENTS.md, MSRV section from README.md, delete validation script and its test
    status: pending
  - id: verify
    content: Run full gate locally on stable, push, confirm CI green
    status: pending
isProject: false
---

# Unify toolchain and fix CI

<product-contract>

## Product Requirements

The repository pins local builds to Rust 1.89 while CI lints and tests on stable (currently 1.98). This skew just caused three CI failures on a pull request that passed every local check. Every crate in the workspace is `publish = false`, so no downstream consumer needs an MSRV guarantee.

- Problem and users: the developer (and their self-hosted runner, which is the same machine) cannot trust that green locally means green on CI.
- Goals: local builds and CI use the same Rust toolchain; the three current CI failures are fixed; the MSRV machinery is removed.
- Non-goals: changing the Miri nightly pin (that pins a specific nightly for diagnostic text stability, a different concern); supporting consumers who build from crates.io (there are none).
- Success criteria: the full verification gate passes locally on stable and CI goes green on the same toolchain.
- Constraints: the self-hosted runner must track stable without manual intervention after the change; the `rust-version` field's resolver side effect (preferring dependency versions compatible with the declared MSRV) is accepted as going away.
- Open questions: none.

## Functional Specification

The developer runs the same commands locally that CI runs, on the same toolchain, so a green local gate predicts a green CI run instead of merely suggesting it. The three current CI failures disappear as a side effect of removing the skew that produced them, not as patches layered on top of it. A contributor cloning the repository gets the right toolchain from rustup automatically - no version number to look up, no MSRV policy to learn, no validation script to run.

- Actors and workflows: the developer builds and tests locally; CI runs the same checks on every pull request.
- Inputs and outputs: code changes go in; a green or red CI status comes out.
- States and validation: before, local and CI can disagree on lint and test outcomes; after, they cannot disagree on toolchain behavior.
- Errors and recovery: the three current failures (a clippy lint, a stale lint expectation, a flaky paused-time test) are fixed in place.
- Security and privacy behavior: none.
- Acceptance criteria: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace --all-features`, `cargo test --doc`, and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass locally on stable, and the CI run on the same commit is green.

</product-contract>
<implementation-contract>

## Technical Design

The change removes the MSRV pin and its CI overrides, then fixes the three code issues the skew exposed.

- `rust-toolchain.toml`: `channel = "1.89"` becomes `channel = "stable"`.
- `Cargo.toml`: remove `rust-version = "1.89"` from `[workspace.package]`.
- All 34 crate manifests under `crates/`: remove `rust-version.workspace = true` (line 5 of each).
- `.github/workflows/ci.yml`: remove the global `env.RUSTUP_TOOLCHAIN: stable` (line 19) and every per-job `env.RUSTUP_TOOLCHAIN: stable` (lines 37, 87, 136, 166, 238); delete the entire `msrv` job (lines 344-372); remove `msrv` from `ci-green`'s `needs:` list (line 394).
- `.github/workflows/stt-miri.yml`: remove `env.RUSTUP_TOOLCHAIN: 1.89` and `env.RUSTUP_AUTO_INSTALL: "0"` from `native-whisper` (lines 49-51); remove the "Verify preinstalled MSRV Rust" step (lines 55-57). The Miri job's `nightly-2026-09-05` pin stays.
- `.github/workflows/release-workshop.yml`: remove the MSRV comment (lines 30-31) and `RUSTUP_TOOLCHAIN: stable` (line 32).
- `AGENTS.md`: remove the last bullet about CI linting on a newer stable than the pinned local MSRV.
- `README.md`: remove the "Minimum Rust Version" section (lines 112-114); change "Every build needs Rust 1.89 or later and Node.js 22" to "Every build needs Rust and Node.js 22" (line 68); remove or update the `rust-1.89+` badge (line 3).
- Delete `tools/validate-rust-1.89.0.ps1` and `tools/check-stt-native-workflow.test.mjs` (the test exists only to test the script).
- `crates/gateway-stt/src/audio.rs:317`: remove unnecessary parens - `<(dyn std::error::Error + 'static)>::is::<...>` becomes `<dyn std::error::Error + 'static>::is::<...>`.
- `crates/workshop/src/bridge.rs`: replace `#[expect(clippy::ptr_as_ptr, clippy::borrow_as_ptr)]` with `#[expect(clippy::ref_as_ptr)]` on the `#[implement(...)]` macro blocks (the old lints were replaced by `ref_as_ptr` in Rust 1.98).
- `crates/gateway-stt-engine/src/test_fixtures/tests/scenario_cleanup/decode.rs`: revert the `decode_rendezvous_timeout` test to real-time (remove `start_paused = true`); the paused clock is structurally incompatible with the test's real-time condvar rendezvous.

</implementation-contract>
<verification-contract>

## Testing Plan

The full verification gate runs locally on stable after the changes, then CI confirms on the same toolchain.

- Unit: the existing suite passes unmodified except for the three fixed files.
- Integration and end-to-end: the full workspace test suite passes on stable.
- Regression, security, and performance: the three CI failures are fixed and do not recur; the `ci-green` aggregate job reflects the remaining jobs accurately after `msrv` is removed from its `needs:` list.
- Exit criteria: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -- -D warnings`, `cargo test --locked --workspace --all-features`, `cargo test --doc`, and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features` all pass locally on stable, and the CI run on the same commit is green.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Unify on `stable` rather than pinning to an exact version like `1.98`: "I want a rule: local builds and CI use the same environment. I never want CI to fail when local succeeds." Pinning to an exact version would require periodic bumps; `stable` tracks automatically.
  - Remove `rust-version` entirely rather than setting it to match stable: "should we get rid of all this msrv and surrounding crap? I want a dramatic simplification to the build and CI." Every crate is `publish = false`, so the field has no consumer audience.
  - Delete the `msrv` CI job rather than keeping it as a lower-bound check: the job exists to verify the MSRV, which no longer exists.
  - Revert the `decode_rendezvous_timeout` test to real-time rather than fixing the paused-clock interaction: the test's condvar rendezvous runs on real OS threads, making paused time structurally incompatible.
- Rejected alternatives:
  - Pin to an exact stable version (e.g., `1.98`): rejected because it requires manual bumps and reintroduces a drift window; revisit if a future stable release breaks the build in a way that needs pinning.
  - Keep `rust-version` set to the current stable: rejected because it would drift and serve no consumer; revisit if the project ever publishes a crate.
- Assumptions, risks, and notes:
  - The self-hosted runner needs `rustup default stable` run once after this change; after that, rustup tracks stable automatically.
  - Removing `rust-version` means Cargo's resolver may select dependency versions that require newer Rust than any previously declared MSRV; on stable, this is always satisfied.
  - A contributor on an old Rust who clones the repo gets `stable` from rustup automatically via the toolchain file; no compile error, no manual install.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (builds the gateway, the default member); `cargo build -p workshop` for the desktop app (explicit opt-in; needs Tauri system packages and `npm ci --prefix crates/workshop-server/ui` plus `npm ci --prefix crates/gateway-config-ui/ui` first)
- Focused test command pattern: `cargo nextest run -p <crate> <filter>` or `cargo test -p <crate> --test it <name>` (integration binaries are named `it`)
- Component test command pattern: `cargo nextest run -p <crate>` (all features: add `--all-features`)
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc` for doctests; workshop crates run separately with `cargo nextest run --locked -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; workshop crates: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`
- Formatter check command: `cargo fmt --all --check`
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS: -D warnings`
- Test placement and naming conventions: unit tests live in `src/tests.rs` or `#[cfg(test)]` modules beside the code; integration tests live in `tests/it/` as one binary (`main.rs` plus per-area modules such as `boot.rs`, `chat.rs`); behavior tests ship in the same change as behavior changes
- Directory map: `crates/` holds all workspace members (Rust crates plus `shared-ui`, a TypeScript+CSS package excluded from the Cargo glob); `guide/` holds the user guide sources; `prompts/` holds prompt pipelines; `tools/` holds Node maintenance scripts (sidecar staging, workflow checks); `vibe/` holds `archdoc.md` architecture documentation; `images/` and `local/` hold assets and local config; `.github/workflows/` holds CI; `target/` and `target-msrv/` are build outputs
- Component boundaries: executor (`promptforge`, `promptforge-core`, `promptforge-lua`, `promptforge-parser`, `promptforge-agent`, `promptforge-tools`, `promptforge-store`, `promptforge-webfetch`, `promptforge-web-search`, `promptforge-model-client`, `promptforge-tool-picker`) executes pipelines and Lua programs; `gateway*` crates own model routing, provider access, STT, and local inference as an independent server; `workshop` and `workshop-server` are the Tauri desktop shell and in-process server; `shared-*` crates are the dependency-free substrate (progress, loopback, protocol, sidecar). Dependency rules: PromptForge crates cannot depend on Gateway or Workshop crates; Gateway crates cannot depend on PromptForge or Workshop crates; Workshop crates cannot depend on Gateway crates
- Conventions summary: Rust edition 2024, MSRV 1.89 pinned in `rust-toolchain.toml` while CI lints and tests on stable; `unsafe_code` forbidden workspace-wide with documented exceptions at owned boundaries; clippy `all` denied, `pedantic` warned, `unwrap_used`/`expect_used` denied; long-running work reports through `shared-progress`; comments explain non-obvious constraints and cite upstream issue URLs for workarounds; UI bundles build into `OUT_DIR` and no build step may dirty the repository tree

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: Unify toolchain, fix the three CI failures, and remove MSRV machinery [completed]

- Component: `none`

Apply every change in one commit (code fixes plus their tests, per repo convention that behavior tests ship with behavior changes):

- `crates/gateway-stt/src/audio.rs:317`: remove unnecessary parens - `<(dyn std::error::Error + 'static)>::is::<...>` becomes `<dyn std::error::Error + 'static>::is::<...>`.
- `crates/workshop/src/bridge.rs`: replace `#[expect(clippy::ptr_as_ptr, clippy::borrow_as_ptr)]` with `#[expect(clippy::ref_as_ptr)]` on the `#[implement(...)]` macro blocks.
- `crates/gateway-stt-engine/src/test_fixtures/tests/scenario_cleanup/decode.rs`: revert `decode_rendezvous_timeout` to real-time (remove `start_paused = true`).
- `rust-toolchain.toml`: `channel = "1.89"` becomes `channel = "stable"`.
- `Cargo.toml`: remove `rust-version = "1.89"` from `[workspace.package]`; remove `rust-version.workspace = true` from all 34 crate manifests under `crates/`.
- `.github/workflows/ci.yml`: remove global `env.RUSTUP_TOOLCHAIN: stable` (line 19) and per-job overrides (lines 37, 87, 136, 166, 238); delete the `msrv` job (lines 344-372); remove `msrv` from `ci-green`'s `needs:` list (line 394).
- `.github/workflows/stt-miri.yml`: remove `env.RUSTUP_TOOLCHAIN: 1.89` and `env.RUSTUP_AUTO_INSTALL: "0"` (lines 49-51) and the "Verify preinstalled MSRV Rust" step (lines 55-57); keep the `nightly-2026-09-05` Miri pin.
- `.github/workflows/release-workshop.yml`: remove the MSRV comment (lines 30-31) and `RUSTUP_TOOLCHAIN: stable` (line 32).
- `AGENTS.md`: remove the last bullet about CI linting on a newer stable than the pinned local MSRV.
- `README.md`: remove the "Minimum Rust Version" section (lines 112-114); change line 68 to "Every build needs Rust and Node.js 22"; remove or update the `rust-1.89+` badge (line 3).
- Delete `tools/validate-rust-1.89.0.ps1` and `tools/check-stt-native-workflow.test.mjs`.

</step-1>

<step-2>

### Step 2: Run the full verification gate on stable and confirm green CI

- Component: `none`

Run once on the self-hosted runner: `rustup default stable`. Then locally on stable, in order: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --locked --workspace --all-features`; `cargo test --doc`; `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`. All must pass. Commit any residual fixes, push, and confirm the CI run on the same commit is green, with `ci-green` reflecting the remaining jobs after `msrv` removal.

</step-2>

</execution-plan>
