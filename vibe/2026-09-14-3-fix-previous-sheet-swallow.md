---
name: Fix previous-sheet swallow
overview: "Remove DEBT-PMS-1: the sheet binary treats a failed previous-sheet download identically to a first run, so a double failure publishes a regressed sheet with a success exit code. Distinguish absent-release (tolerate) from unreachable/corrupt-release (fatal) in the binary."
todos:
  - id: debt-pms-1
    content: "DEBT-PMS-1: distinguish previous-sheet error kinds in fetch_sheet/previous_sheet; fatal exit on outage or corruption, tolerate unset URL and 404; unit + 3 integration tests; nextest/clippy/fmt green"
    status: pending
isProject: false
---
<product-contract>

## Product Requirements

- Scope and target work: the provider-model-sheets run (`1537ecf2..970f3c29`, 12 commits) in `c:\Users\Vinnie\cursor\promptforge`. One accepted debt: DEBT-PMS-1 (introduced by `8e27fe60`, interacting with `79d1d691`).
- The debt: `crates/shared-cloud-providers/src/main.rs` `previous_sheet` returns `None` on any `fetch_sheet` error (transport, non-success status, parse failure) with only a stderr note. With `previous: None`, `crates/shared-cloud-providers/src/sheet.rs` `stale_or_unavailable` records every failed provider fetch as `unavailable` with an empty model list, and the binary writes the sheet and exits 0. A transient outage or misconfigured `MODELS_SHEET_PREVIOUS_URL` therefore publishes a sheet that replaces last-known-good data with empty slices - defeating the "never drops data" contract in exactly the double-failure case it exists for, with no signal to the workflow to withhold publication.
- Cleanup goals: a configured-but-unreachable or unparseable previous-sheet URL is fatal (nonzero exit, no output written); a genuinely absent release (URL unset, or HTTP 404) is tolerated as first run.
- Non-goals: no schema, wire, or public-API changes; no changes to provider fetch/normalization; no changes to the aggregation workflow repo (not yet created); the 8 rejected candidates (R1-R8) stay as recorded.
- Success criteria: the double-failure case exits nonzero and writes nothing; first-run and 404 cases still succeed; full workspace gates stay green.

### Debt Inventory

- DEBT-PMS-1 (introduced, accepted by analysis and upheld by challenge): swallowed previous-sheet download failure. Evidence: `previous_sheet` in `main.rs` collapses all error kinds to `None`; commit `8e27fe60` carries `Design: new swallowed-exception @ ...main.rs::previous_sheet`; consequence is a durable-data path onto the published release artifact. Reversal cost: low - `previous_sheet` is private to the binary with no consumers yet. Target state: error-kind distinction with fatal exit on outage, tolerance on genuine absence.
- Exposed pre-existing debt: none observed.
- Rejected candidates: 5 weak/speculative (R1 dispatch/registry duplication, R5 pub `Capabilities` fields, R6 unbounded body reads, R7 repeating-cursor pagination, R8 non-atomic write), 3 residual-but-acceptable (R2 `ModelEntry` bag-of-state, R3 re-export shims, R4 `static_json` test-only arms). Full evidence in the findings and challenge scratch files of the debt-collector run dated 2026-09-14.

## Functional Specification

- Actors and workflows: the sheet-building binary (the `shared-cloud-providers` bin target), run locally or by the aggregation workflow in the separate repo. It reads provider keys and `MODELS_SHEET_PREVIOUS_URL` from the environment, downloads the previous sheet when configured, builds the merged sheet, and writes `models.json`.
- Inputs and outputs: environment variables in; one `models.json` file out at argv[1] or `./models.json`; the process exit code is the workflow's publish/withhold signal.
- States and validation: the previous-sheet download has exactly three tolerated-or-fatal outcomes - URL not configured (first run, proceed), release absent (HTTP 404, proceed as first run), sheet fetched (proceed with history).
- Errors and recovery: on a configured URL, transport failure, a non-404 non-success status, or an unparseable body is fatal - the binary writes nothing, exits nonzero, and stderr names the URL and the failure. Provider fetch failures keep their existing semantics (`stale` propagation when history exists, `unavailable` when not); only the loss of history itself becomes fatal.

</product-contract>
<implementation-contract>

## Technical Design

- In `crates/shared-cloud-providers/src/main.rs`, change `previous_sheet` to return a three-way result: no URL configured (first run), release absent (HTTP 404), or a fetched `Sheet`. Any other failure - transport error, non-404 non-success status, or unparseable body - propagates as a run error: `run` returns before writing output and `main` exits nonzero with a concise stderr message naming the URL and the failure.
- `fetch_sheet` in `crates/shared-cloud-providers/src/sheet.rs` needs its error to carry the HTTP status (or a dedicated `NotFound` variant on `FetchError`) so the binary can distinguish 404 from other failures. This is an internal enrichment of an in-flight crate, not a wire or persisted change.
- No change to `build_sheet` propagation semantics: `stale`/`unavailable` behavior is correct once history loss is fatal.

</implementation-contract>
<verification-contract>

## Testing Plan

- Unit (DEBT-PMS-1): `previous_sheet` maps unset URL, 404, transport error, 500, and unparseable-200 to the correct three-way outcome; loopback stub server per the existing `fetch_sheet_parses_a_successful_response` idiom.
- Integration (DEBT-PMS-1): in `crates/shared-cloud-providers/tests/sheet_binary.rs` - (a) stub previous-sheet URL returning 500 plus all provider keys stripped: assert nonzero exit and no output file written; (b) stub returning 404: assert success with `unavailable` slices; (c) stub returning 200 with invalid JSON: assert nonzero exit, no write.
- Regression: existing `sheet::` and `binary_` suites stay green.
- Exit checks: `cargo nextest run -p shared-cloud-providers`, clippy `-D warnings`, `cargo fmt --check`.

</verification-contract>
<decision-record>

## Decision Record

- Selected remedy (reversible, chosen autonomously): distinguish error kinds - fatal on configured-but-unreachable or corrupt previous sheet, tolerate only unset URL and 404. Consequence: the workflow's failure signal is the exit code it already watches; no schema or consumer change.
- Rejected alternatives: hard-fail on any download problem including 404 (contradicts the first-run tolerance); keep warn-and-continue (that is the debt); stamp the envelope with a propagation-missing flag (adds wire surface to work around a build-time problem; consumers would each need to enforce it); write output only when every failed fetch had a previous slice (blocks publication forever for a provider that never existed).
- User-resolved architecture choices: none required - reversal cost is low and the binary has no consumers yet.
- Assumptions and risks: the aggregation workflow repo will treat nonzero exit as do-not-publish (standard Actions behavior); the 404-as-absent rule assumes the release asset URL 404s before the first publication, which the workflow design should honor.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (builds only the gateway, the default member, on a fresh clone); the desktop app is explicit: `cargo build -p workshop`
- Focused test command pattern: `cargo nextest run -p <crate> <test-name-filter>`
- Component test command pattern: `cargo nextest run -p <crate>`
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`)
- Formatter check command: `cargo fmt --all --check`
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`
- Test placement and naming conventions: unit tests live in `#[cfg(test)]` modules beside the source; integration tests live in `crates/<crate>/tests/` (present in gateway, promptforge-api, workshop-server, and about a dozen other crates); JavaScript tools in `tools/` carry sibling `*.test.mjs` files; nextest profiles and a `heavy` test group for tensor/FFI suites are configured in `.config/nextest.toml`; the boundary and structural harness runs as `cargo test -p build-xtask`
- Directory map: `crates/` holds all workspace members (Rust crates plus the excluded TypeScript package `shared-ui`); `guide/` is the mdBook user guide; `prompts/` holds example PromptForge prompt files; `tools/` holds standalone JS tools and docs; `vibe/` holds design and plan documents including `archdoc.md`; `.github/workflows/` holds CI; `.config/` holds nextest config; `.githooks/`, `.cargo/`, `images/`, `local/`, `target/`, and `target-msrv/` are support and build output
- Component boundaries: three products with strict naming and dependency rules: `promptforge-*` (executor, parser, Lua boundary, store, VFS policy, web tools; may not depend on gateway or workshop crates), `gateway-*` (inference gateway: routing, protocol, config, STT, sidecar; may not depend on promptforge or workshop crates), `workshop-*` (Tauri desktop shell and in-process server; may not depend on gateway crates); `shared-*` crates carry the cross-product API surface and depend on no product crates; `build-*` crates build specific outputs; the one-door rule: crates outside the promptforge-* family may depend only on `promptforge-api`, never on internal promptforge-* substrate crates; dependency direction is shell -> features -> services -> vocabulary, enforced by `build-xtask`
- Conventions summary: edition 2024, workspace-inherited lints forbid unsafe code and deny clippy `all`, `unwrap_used`, and `expect_used`; behavior changes ship with tests in the same change; reuse of existing facilities is preferred over new machinery; error messages are written for model consumption (concise, factual, self-contained); no file exceeds 500 lines; every workshop-* crate's lib.rs opens with a `## Invariants` doc marker; SPA CSS lives beside its TypeScript with `--ws-*` design tokens, never raw values; long-running work reports through `shared-progress`; Cargo features gate real constraints, not product shape

</project-survey>
<execution-plan>

## Execution Instructions

<step-1>

### Step 1: distinguish previous-sheet error kinds [completed]

- Component: none
- Artifacts: `crates/shared-cloud-providers/src/sheet.rs` (`FetchError`, `fetch_sheet`), `crates/shared-cloud-providers/src/main.rs` (`previous_sheet`, `run`, `main`), `crates/shared-cloud-providers/tests/sheet_binary.rs`.
- In `sheet.rs`, add a `NotFound` distinction to `FetchError` (or carry the HTTP status) so `fetch_sheet` lets callers tell 404 from other failures; internal enrichment only, no wire or schema change.
- In `main.rs`, rework `previous_sheet` to a three-way outcome: URL unset (first run), HTTP 404 (release absent, first run), or fetched `Sheet`. Any other failure (transport error, non-404 non-success status, unparseable body) propagates: `run` returns before writing output and `main` exits nonzero with a concise stderr message naming the URL and the failure. Provider fetch failures keep existing `stale`/`unavailable` semantics; only loss of history is fatal.
- Tests in the same commit: unit tests for `previous_sheet` mapping unset URL, 404, transport error, 500, and unparseable-200 to the correct outcome, using the loopback stub idiom of `fetch_sheet_parses_a_successful_response`; integration tests in `tests/sheet_binary.rs`: (a) stub previous-sheet URL returning 500 with provider keys stripped asserts nonzero exit and no output file, (b) stub returning 404 asserts success with `unavailable` slices, (c) stub returning 200 with invalid JSON asserts nonzero exit and no write.
- Regression: existing `sheet::` and `binary_` suites stay green.
- Verify: `cargo nextest run -p shared-cloud-providers`, clippy with `-D warnings`, `cargo fmt --check`.
- One commit containing code and tests (operator directive 2026-09-14: exactly one commit).
- Explicit exclusions: no schema changes, no provider-file changes, no workflow-repo changes, no remediation of rejected candidates R1-R8.

</step-1>

</execution-plan>
