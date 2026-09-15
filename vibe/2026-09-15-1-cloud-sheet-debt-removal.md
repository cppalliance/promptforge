---
name: Cloud sheet debt removal
overview: "Remove the debt attributed to the model taxonomy and cloud models UI work (commits 0e65c42f..91848953): make Refresh deliver a fresh sheet and surface failures, cap the sheet download's body read, gate the sheet on its schema version, and stop the end-to-end test from overstating the Cloud tab's add flow. Profile membership work is deferred; the exposed proxy-allowlist debt is reported, not remediated."
todos:
  - id: reader-cap-version
    content: "Gateway reader: capped download in download_once, shared schema-version constant, version gate on cache and download paths, module split for the ceiling; cap and version tests (DEBT-MTCU-3, DEBT-MTCU-4)"
    status: pending
  - id: refresh-delivery
    content: "Refresh delivery per the operator's choice (A: POST awaits and answers with the sheet or error; UI drops post-refresh poll); route and store tests (DEBT-MTCU-1)"
    status: pending
  - id: e2e-align
    content: "Align the e2e test to the UI merge: drop the profile push and the fixture profile; rerun (DEBT-MTCU-2 test half)"
    status: pending
  - id: schema-policy
    content: Document the additive-field serde(default) policy in shared-gateway-api; schema_version stays 1, no bump (DEBT-MTCU-4 discipline)
    status: pending
isProject: false
---

# Cloud Sheet Debt Removal

<product-contract>

## Product Requirements

Four debts introduced by the model taxonomy and cloud models UI work survive at the endpoint and are removed here: a Refresh action that cannot deliver a fresh sheet or show a failed download, a sheet download that reads its body without the size cap every other gateway outbound read honors, a sheet reader that never checks the schema version it depends on, and an end-to-end test that reaches the catalog through a profile step the UI never performs. One pre-existing debt the work exposed (the workshop proxy allowlist mirroring the gateway's admin routes) is inventoried and left out of scope. Profile-membership behavior is deferred by the operator's direction: "defer the profiles work, do the least amount of work on profile for debt relief (including doing nothing)."

- Problem and users: operators of the gateway config UI who press Refresh on the Cloud tab and see the same `generated_at`; gateway deployments whose sheet download is the one outbound read that can allocate an unbounded body; future gateways that meet a sheet in a newer shape and get only a serde error; readers of the integration suite who take `crates/gateway/tests/it/cloud_models.rs` as proof of a flow it does not restate.
- Goals:
  - Refresh either delivers a newer sheet to the open view or shows why it could not (DEBT-MTCU-1).
  - The sheet download reads at most `MAX_JSON_BODY` bytes, like every other gateway outbound read (DEBT-MTCU-3).
  - A sheet whose `schema_version` the gateway does not understand fails with an error that names the mismatch, both from the disk cache and from a download (DEBT-MTCU-4, reader side).
  - The end-to-end test performs exactly the merge the UI performs and asserts the behavior the UI produces (DEBT-MTCU-2, test half).
- Non-goals: any change to profile semantics or to whether the Cloud tab enrolls a model in a profile (deferred); publisher-side versioned assets or release pinning in `cppalliance/promptforge-cloud-providers` (deferred, cross-repo); remediation of the workshop proxy allowlist (exposed pre-existing, out of scope unless the operator expands it); updating `vibe/archdoc.md` or the source plan's decision record; the eleven rejected candidates.
- Success criteria: each debt's proving check in the Testing Plan passes; the existing gateway, `shared-cloud-providers`, `shared-gateway-api`, and config UI suites stay green; no new admin route or method is added (so the workshop proxy allowlist needs no change).
- Constraints:
  - Baseline `0e65c42f`, endpoint `91848953`, clean worktree; every finding was verified against the endpoint tree, not against commit messages. All file line numbers in this plan are as of `91848953`.
  - Repository: the `promptforge` workspace on branch `master`. Remote `origin` is the fork `vinniefalco/promptforge`; remote `upstream` is `cppalliance/promptforge`. Work lands on the fork's `master`, which carries the target commits from `3ff9df0f` onward; no new branch.
  - The source plan for the target work is `vibe/2026-09-14-6-model-taxonomy-cloud-ui.md`; the architecture record is `vibe/archdoc.md`. Both are read-only evidence for this plan.
  - `shared-*` crates depend on no product crate, so `shared-cloud-providers` cannot use `gateway-protocol`'s cap helpers; the cap lives gateway-side (`crates/gateway-protocol/src/http_util.rs`: `MAX_JSON_BODY` at line 14, `bounded_client` at 24, `read_bytes_capped` at 103).
  - The config UI reaches admin routes through the workshop proxy allowlist in `crates/workshop-server/src/routes/gateway_config.rs` (`forward_allowed`), which admits exactly `GET /admin/cloud-models` and `POST /admin/cloud-models/refresh`; remedies must stay within those two method-and-path pairs.
  - `SCHEMA_VERSION` is a private `const` in `crates/shared-cloud-providers/src/sheet.rs` line 17 (value 1); the gateway has no accepted-version constant today.
  - Workspace conventions: edition 2024, clippy `-D warnings`, no file over 500 lines (`crates/gateway/src/cloud_models.rs` is already 711 lines with its inline tests, so new code there must move tests or logic to a sibling module rather than grow the file), tests beside the change, no em dashes.
- Open questions: None

### Debt Inventory

- DEBT-MTCU-1 (introduced; `c87eb800`, `696bd9f1`): `admin_cloud_models` in `crates/gateway/src/cloud_models.rs` returns the in-memory sheet whenever `sheet()` is `Some` and consults `last_error` only otherwise; `admin_cloud_models_refresh` answers 202 `{"status":"downloading"}` for both a started and an in-flight download. `SheetStore.refresh` in `crates/gateway-config-ui/ui/src/services/sheet-store.ts` awaits the POST then calls `poll` once, and `poll` reschedules only on the `cloud_models_loading` error code. With a sheet in memory the GET answers 200 with the old sheet before the download completes, so the store stops with the stale sheet and the view re-renders the same `generated_at`; a failed download sets `last_error` but nothing routes it to a client that already holds a sheet. The store test "refresh forces a re-download and notifies again when the sheet lands" stubs the GET to return the fixture regardless and asserts only that a notification fired. Impact: the Refresh control does not do what its label, the store's doc comment, and the plan's functional specification say. Reversal cost: low to medium; two components, one existing route. Target state: after Refresh the view shows the downloaded sheet or a failure toast.
- DEBT-MTCU-2 (introduced; `696bd9f1`, `1c8889e2`): `mergeCloudModel` in `crates/gateway-config-ui/ui/src/services/cloud-merge.ts` appends `[[endpoint]]` and `[[model]]` only. `Config::select_profile` in `gateway-config` narrows `models()` to the selected profile's list before routing is built (pinned in `crates/gateway-config/src/config/tests/validation.rs` near line 1109), so under an active profile a Cloud-tab add validates and applies but the model never enters the catalog or `GET /v1/models`. `merge_cloud_model` in `crates/gateway/tests/it/cloud_models.rs` (line 167) additionally pushes the name into the active profile's `models` (lines 196 to 204) against a fixture declaring `[[profile]] name = "main" models = []` (line 118), then `wait_for_catalog` asserts the model in `/v1/models`; the test's stated purpose is to restate the UI merge. Impact: the only end-to-end proof of the acceptance criterion passes on a step production does not perform, and the two merge copies have already diverged. Reversal cost: low for the test; the behavior half is deferred. Target state: the test performs the UI's merge exactly and asserts the catalog under a configuration where the UI's merge is sufficient; the behavior gap is recorded as deferred with its options.
- DEBT-MTCU-3 (introduced; `c87eb800`): `download_once` in `crates/gateway/src/cloud_models.rs` calls `shared_cloud_providers::fetch_sheet` with `bounded_client()`; `fetch_sheet` does `error_for_status()?.json().await?` with no size cap and `bounded_client` bounds only connect and total time. `http_util.rs` states that every outbound gateway call reads bodies through `read_body_capped`; at the endpoint this download is the only production exception across `gateway`, `gateway-protocol`, `gateway-local`, and `gateway-web-search`. Impact: a reachable contradiction of a stated invariant; an oversized or mistaken asset at the release URL is read fully into the long-lived gateway process. Reversal cost: low, gateway-only. Target state: the download reads at most `MAX_JSON_BODY` bytes and an over-cap body is recorded as a download error.
- DEBT-MTCU-4 (introduced, prospective; `a2c02e07`, `c87eb800`, `3ff9df0f`): `Sheet.schema_version` is written as 1 and parsed, but no code in the gateway, `shared-cloud-providers`, or the UI compares it; `ModelEntry` and `ProviderSlice` gained required fields with no defaults, and `missing_new_fields_fail_to_parse` pins rejection of an older shape. The gateway persists the sheet at `<profile>/cloud-provider-models.json`, treats an unparseable cache as absent and re-downloads, and reports a download parse failure only as a serde string in a 502. The publisher clones `cppalliance/promptforge` master and republishes weekly to one rolling URL. Impact: the decision-record premise "there are no deployed readers today" is false at the endpoint; the next required-field change on master strands every not-yet-upgraded gateway within a week with no version signal. Reversal cost: the reader-side gate is cheap and needs no wire change (the field exists); preventing the strand needs either schema discipline in `shared-gateway-api` or a publisher-side change in another repository. Target state: a version mismatch produces a distinct, named error from both cache and download paths; additive fields carry defaults going forward with `schema_version` held at 1; publisher pinning deferred.
- Exposed pre-existing, not counted as added: DEBT-MTCU-5 (`30ed96cb` in target; earlier episodes `f6f36259` on 2026-09-04 and the original allowlist `c13ec5f7` on 2026-08-30): `forward_allowed` in `crates/workshop-server/src/routes/gateway_config.rs` is a literal `matches!` over method and path mirroring the gateway's admin router with no shared source and no test linking the two; twice a new admin route shipped, was verified without the proxy, and was repaired after a runtime refusal in the panel. Remediation is out of scope here; the cheapest form would be a workshop-server test enumerating the config UI's admin calls against the allowlist, and the fuller form a route list exported from a `shared-*` crate that both sides build from.
- Rejected candidates (11): residual-but-acceptable 5 (file sizes in `cloud_models.rs` and `cloud-models-view.ts`; `CloudModels` use before `launch`; the workshop build script's sidecar copy as mitigation; the test-only `apply_provider_taxonomy` dispatch mirroring the registry; `write_cache_atomic` renaming without fsync), weak/speculative 4 (unread `api` in `CloudModelsViewDeps`; the dead-code allowances later consumed; near-duplicate `kind_of` tables across four providers; the deferred manual live check, since performed), false 3 (native `<select>` replacement; shotgun surgery across 23 provider files; cache path from `run_dir.parent()`), unrelated pre-existing 1 (the sidecar path collision itself).

## Functional Specification

Observable behavior changes in three places: the Cloud tab's Refresh control, the gateway's handling of a sheet it cannot accept, and the integration test's claim. Nothing changes for hosts using `GET /v1/models`, and no admin route is added or removed.

- Actors and workflows: an operator presses Refresh on the Cloud tab and either sees the newer `generated_at` render in place or sees a failure toast naming the cause; the gateway at launch or on refresh meets a sheet body over the cap or with an unexpected `schema_version` and records a named error instead of allocating or reporting a raw serde string; a developer runs the gateway integration suite and reads a Cloud-tab test whose merge is the UI's merge.
- Inputs and outputs: the refresh POST returns the downloaded sheet (200) or the download error (502) instead of an immediate 202; the GET is unchanged. Error strings for cap and version failures are distinct from parse failures and name the limit or the versions involved.
- States and validation: sheet state stays absent, fresh, stale, or unreachable; a version-mismatched or over-cap cache file is treated as absent (logged, overwritten by the next good download) exactly as an unparseable one is today; a version-mismatched or over-cap download leaves any in-memory sheet in place and records the named error.
- Errors and recovery: a failed refresh surfaces through the POST's own error to the view's existing catch and toast; the loaded sheet stays visible; the next launch re-checks age as before.
- Security and privacy behavior: the download's memory ceiling becomes `MAX_JSON_BODY` (4 MiB); no secret material is involved; the admin routes remain loopback-walled and proxy-allowlisted exactly as today.
- Acceptance criteria: with a stub that serves sheet A then sheet B, pressing Refresh in the UI ends with B rendered; with a stub that serves A then fails, Refresh ends with A still rendered and a toast; a stub body over the cap yields a recorded cap error and no sheet swap; a stub sheet with `schema_version: 2` yields a recorded version error naming 2 and the accepted version, from both the cache path and the download path; the integration test's `merge_cloud_model` contains no profile handling and asserts the behavior the UI produces: the model applies but is absent from `GET /v1/models` under the fixture's empty profile, with the profile-membership gap recorded as deferred.

</product-contract>
<implementation-contract>

## Technical Design

All changes sit in the gateway crate, the config UI's sheet store, and one integration test; the shared crates are untouched unless the operator adopts the additive-field policy. The gateway keeps its two admin routes with their current methods and paths.

- Architecture: `download_once` in `crates/gateway/src/cloud_models.rs` stops calling `shared_cloud_providers::fetch_sheet` and performs the request itself: `bounded_client().get(url).send()`, `error_for_status()`, `read_bytes_capped(response, MAX_JSON_BODY)`, then `serde_json::from_slice::<Sheet>`. The 404 special case in `fetch_sheet` exists for the binary's previous-sheet semantics and is not needed by the gateway. A `Content-Length` precheck may precede the read so an over-cap body fails with a cap error rather than a truncated-parse error (DEBT-MTCU-3).
- Version gate: the gateway declares the accepted sheet schema version once (a `pub const` in `crates/shared-gateway-api/src/lib.rs` beside `Sheet`, so the binary's writer and the gateway's reader share it; `crates/shared-cloud-providers/src/sheet.rs` then uses that constant instead of its private one) and checks `sheet.schema_version` after every successful parse in both `read_cache` and `download_once`. A mismatch maps to a distinct error variant carrying the found and accepted versions; on the cache path it is treated as absent like `CacheRead::Unparseable`, on the download path it becomes `last_error`. No wire or persisted format changes; the field already exists (DEBT-MTCU-4, reader side).
- Refresh delivery: `admin_cloud_models_refresh` awaits the download it starts or joins and answers 200 with the sheet on success or the `CloudModelsUnavailable` 502 on failure; the in-flight guard keeps a second concurrent refresh from starting a second download and lets it await the same result. `SheetStore.refresh` in `crates/gateway-config-ui/ui/src/services/sheet-store.ts` uses the POST's answer directly: on 200 it stores the sheet and notifies; on failure it records the error, notifies, and rethrows so the view's existing catch toasts. The post-refresh `poll` call is removed. The launch-time background download is unchanged (DEBT-MTCU-1).
- Schema discipline: `schema_version` stays 1 and is not bumped by this work. Future additive fields on `Sheet`, `ProviderSlice`, and `ModelEntry` in `crates/shared-gateway-api/src/lib.rs` carry `#[serde(default)]` so a lagging reader survives an additive change; the version bump is reserved for removals and renames. The existing required fields are left as they are; the rule applies going forward and is recorded in the type-level doc comments beside `schema_version` (DEBT-MTCU-4, discipline).
- Integration test: `merge_cloud_model` in `crates/gateway/tests/it/cloud_models.rs` drops the profile push (lines 196 to 204) so it performs exactly the UI's merge, and the fixture at line 118 drops its `[[profile]]` block so no profile is selected and `Config::select_profile` does not narrow the catalog; `wait_for_catalog` then holds on the UI's merge alone. A one-line comment records that profile membership is not part of the add flow (DEBT-MTCU-2, test half).
- Modules and interfaces: `GatewayError` gains one variant for the version mismatch and, if the precheck is adopted, one for the body cap; both classify as 502 on the download path. `shared-gateway-api` gains one `pub const` for the accepted schema version. No other public API changes.
- File and public API changes: `crates/gateway/src/cloud_models.rs` (or a sibling module split out to respect the 500-line ceiling, since the file is already over it with its tests), `crates/gateway/src/error.rs`, `crates/shared-gateway-api/src/lib.rs` (constant only), `crates/shared-cloud-providers/src/sheet.rs` (use the shared constant), `crates/gateway-config-ui/ui/src/services/sheet-store.ts` and its test, `crates/gateway-config-ui/ui/src/services/gateway-api.ts` (refresh return type), `crates/gateway/tests/it/cloud_models.rs`.
- Data, persistence, failure, security, and privacy constraints: the cache file format is unchanged; a cache written by a future gateway with a newer schema is treated as absent by this gateway and overwritten by its next good download, which is the existing unparseable-cache behavior; the download's memory ceiling is `MAX_JSON_BODY`; the two admin routes keep their loopback wall and their proxy allowlist entries, so DEBT-MTCU-5 is not triggered by this work.

</implementation-contract>
<verification-contract>

## Testing Plan

Each debt has one proving check that fails at the endpoint and passes after its fix, plus the existing suites as regression.

- Unit:
  - DEBT-MTCU-1 (gateway): a route test where the stub serves sheet A at launch and sheet B on the next request asserts the refresh POST answers 200 with B and the GET then serves B; a second test where the stub serves A then fails asserts the POST answers 502 with the error and the GET still serves A; a third asserts two concurrent refresh POSTs produce one stub request.
  - DEBT-MTCU-1 (UI, `sheet-store.test.mjs`): the refresh test's stub returns the old sheet from GET and the new sheet from the POST; assert the store ends on the new sheet's `generated_at`. Replace the existing refresh test, which cannot distinguish the two.
  - DEBT-MTCU-3: a route test whose stub body exceeds `MAX_JSON_BODY` asserts the download records a cap error, swaps no sheet, and leaves any existing cache file unchanged.
  - DEBT-MTCU-4: a route test whose stub sheet carries `schema_version: 2` asserts `last_error` names 2 and the accepted version and the GET reports it (no cache) or keeps the old sheet (cache present); a cache-read test with a version-mismatched file asserts it is treated as absent and a download is spawned; a `shared-gateway-api` or `shared-cloud-providers` test asserts the emitted sheet's `schema_version` equals the shared constant.
- Integration and end-to-end:
  - DEBT-MTCU-2: `the_sheet_downloads_and_merged_models_apply_end_to_end` passes with the profile push removed and the fixture's `[[profile]]` block retained (the gateway cannot boot without a selected profile); the test asserts the UI's actual outcome - the model applies but is absent from `GET /v1/models` under the empty profile; a reviewer can diff `merge_cloud_model` against `mergeCloudModel` and find the same two array appends.
  - The existing gateway `it` suite and `cloud_models` unit tests stay green; the loopback-wall sweep still covers both admin routes.
- Regression, security, and performance: `cargo nextest run -p gateway`, `-p shared-gateway-api`, `-p shared-cloud-providers` (the binary integration suite `tests/sheet_binary.rs` proves `fetch_sheet` is unaffected); the UI `npm run build && npm run typecheck && npm test`; clippy `-D warnings`; `cargo fmt --all --check`; `cargo test -p build-xtask` for the file ceiling. No new outbound call and no change to launch timing.
- Exit criteria: all proving checks above pass; all listed suites green; no change to `forward_allowed` was needed.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Refresh delivers by awaiting: `POST /admin/cloud-models/refresh` waits for the download and answers 200 with the sheet or 502 with the error, and the UI drops its post-refresh poll. Chosen over adding download-state fields to the GET because it needs no polling state, no new route, and the failure path falls out of the view's existing catch. The operator's words: "1 A - okay" (DEBT-MTCU-1).
  - Additive sheet fields carry `#[serde(default)]` from now on, and `schema_version` stays 1 with no bump in this work; the bump is reserved for removals and renames. This reverses the earlier "no serde defaults" call, which was made when no deployed reader existed. The operator's words: "adopt #[serde(default)] okay but we are still at version 0 or 1 I dont want a bump yet" (DEBT-MTCU-4).
  - The body cap lives gateway-side in `download_once`, not as a parameter on `fetch_sheet`: `shared-cloud-providers` may not depend on `gateway-protocol`, the binary does not need a cap, and the gateway does not need the 404 first-run semantics; the small duplication of a GET and status check is cheaper than a public signature change on a shared crate (DEBT-MTCU-3).
  - The accepted schema version becomes one shared `pub const` in `shared-gateway-api`, used by both writer and reader: two private copies would be the next mirror to drift (DEBT-MTCU-4).
  - The version gate treats a mismatched cache as absent, matching the unparseable-cache rule, rather than refusing to start: a gateway must boot with no sheet, and the next good download repairs the cache (DEBT-MTCU-4).
  - The end-to-end test aligns to the UI by removing its profile push and the fixture's profile, not by adding profile behavior to the UI: the operator's direction is to defer profile work and do the least on it, including nothing; a test that restates production must not out-perform production (DEBT-MTCU-2). The operator's words: "defer the profiles work, do the least amount of work on profile for debt relief (including doing nothing)."
  - No new admin route or method: every remedy fits inside `GET /admin/cloud-models` and `POST /admin/cloud-models/refresh`, so the exposed allowlist debt is not re-triggered.
  - Reconciliation: diffs and the endpoint tree outranked commit-message and plan claims; the Challenger's corrections (the filter is `Config::select_profile`, not `runner.rs::model_allowlist`; the cap fix is gateway-only; the version gate is reader-side) are adopted.
  - Plan shape: the debt inventory sits as an H3 inside Product Requirements so the seven-section contract the executor requires holds unchanged.
  - The e2e alignment asserts actual production behavior rather than using a profile-less fixture: the gateway cannot boot without a defined, selected profile, and a selected profile's empty `models` list narrows the catalog to empty, so the plan's original fixture change was unreachable. The test performs the UI's exact merge and asserts the model applies but is absent from `GET /v1/models`, recording the deferred behavior gap. The operator's choice on 2026-09-15, after the coding sub-agent returned blocked: assert actual production behavior (DEBT-MTCU-2).
- Rejected alternatives:
  - Driving the Rust integration test from the TypeScript merge helper or a shared fixture document: correct but heavier than the finding needs; revisit if the two merges diverge again.
  - Pushing a download progress event through the existing SSE stream to fix Refresh: the workshop proxy refuses `/admin/progress` to the config UI by design, so the tab could not consume it; revisit never.
  - Adding the cap as a `max_bytes` parameter on `fetch_sheet`: public signature change on a shared crate for one caller; revisit if a second capped caller appears.
  - Remediating the proxy allowlist in this plan: exposed pre-existing, reported separately; revisit on the operator's word, in which case the minimal form is a workshop-server test enumerating the config UI's admin calls against `forward_allowed`.
- Assumptions, risks, and notes:
  - The refresh POST stays open for the download's duration, bounded by `bounded_client`'s 120 s total timeout; the workshop proxy's own forward timeout, if shorter, would surface as a proxy error rather than a hang. The download has measured under one second against the live release.
  - The source plan's decision record still states "there are no deployed readers today"; this plan does not edit design records, but the next author should read that sentence as superseded by `c87eb800`.
  - The publisher rebuilds from master weekly, so any sheet-shape change merged to master reaches deployed gateways within a week regardless of their version; until publisher-side versioning exists, additive changes with defaults are the only safe kind, and a removal or rename must wait for the deferred publisher work and the bump.
  - `crates/gateway/src/cloud_models.rs` is already over the workspace file ceiling with its inline tests; the executor should split before adding, following the `bedrock/sigv4.rs` sibling-module precedent from the same target range.
  - For any manual check in the Workshop panel: the Workshop must be closed before `cargo build --release -p gateway`, because its running gateway sidecar locks `target/release/promptforge-gateway.exe` and the failed replacement reports once then goes silent on later builds; `cargo build --release -p workshop` then refreshes the Tauri sidecar in `crates/workshop/binaries/` from the gateway just built (`crates/workshop/build.rs`, commit `91848953`). Build order is gateway, then workshop, then launch.
  - All four work items are settled; none waits on an operator decision.

### Deferred and Out of Scope

- Deferred: profile membership for Cloud-tab adds (DEBT-MTCU-2, behavior half). The operator's direction is to defer and do the least work, including nothing. Options recorded for the revisit: (1) profile lists ignore remote names and profiles gate only local and STT models; (2) profile lists reject remote names, with a config-version bump or a load-time migration for existing configs; (3) remote models are in every profile unless a profile opts out, keeping the persona use case. The operator's framing: "maybe profile should only apply to local models." Revisit when the operator decides whether profiles are resource checklists or personas.
- Deferred: publisher-side versioned asset names or pinned builds in `cppalliance/promptforge-cloud-providers` (DEBT-MTCU-4, prevention half, cross-repo). Revisit when the next sheet-shape change is planned.
- Deferred: a "sheet schema newer than this gateway" notice in the Cloud tab beyond the toast text. Revisit if operators hit the version error in practice.
- Out of scope: remediation of the workshop proxy allowlist mirror (DEBT-MTCU-5, exposed pre-existing) unless the operator expands scope; the eleven rejected candidates; updates to `vibe/archdoc.md` or the source plan.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build` (default-members builds only the gateway, which compiles on a fresh macOS/Linux clone with no CUDA or Tauri system packages); desktop app is explicit: `cargo build -p workshop`; config UI bundle: `npm run build` in `crates/gateway-config-ui/ui`
- Focused test command pattern: `cargo nextest run -p <crate> <name-filter>`; UI: `npm test` in `crates/gateway-config-ui/ui` (runs `node --test "src/**/*.test.mjs"`)
- Component test command pattern: `cargo nextest run -p <crate>` (plan-relevant set: `-p gateway`, `-p shared-gateway-api`, `-p shared-cloud-providers`); boundary and structural harness: `cargo test -p build-xtask`
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server`
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`)
- Formatter check command: `cargo fmt --all --check`
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`
- Test placement and naming conventions: Rust unit tests live inline in `#[cfg(test)]` modules beside the code; integration tests live in `crates/<crate>/tests/`, with the gateway using a `tests/it/` harness (`main.rs` plus one file per area, e.g. `cloud_models.rs`); TypeScript UI tests are `*.test.mjs` files beside their sources, run by `node --test`; Node tooling scripts in `tools/` follow the same `*.test.mjs` pattern
- Directory map: `crates/` holds all workspace members grouped by prefix (`gateway-*` inference service, `promptforge-*` pipeline executor, `workshop-*` desktop app, `shared-*` cross-product API surface, `build-*` build orchestration; `crates/shared-ui` is a TypeScript+CSS package excluded from the Cargo glob); `guide/` is the mdbook user guide; `prompts/` holds example prompt pipelines; `tools/` holds Node sidecar scripts; `vibe/` holds architecture docs and dated plan history; `local/` holds local run configs; `.github/workflows/` holds CI
- Component boundaries (from `vibe/archdoc.md` and `AGENTS.md`): executor parses and runs prompt pipelines and Lua agent programs and depends on gateway, store, Lua VM boundary, and shared substrate; gateway owns model routing, provider access, and local inference lifecycle; CLI and workshop UI are shell adapters over executor plus gateway; store/VFS is `promptforge-store` over `shared-vfs` with the `promptforge-vfs` policy gate. Dependency rules: `workshop-*` never depends on gateway crates, `gateway-*` never on promptforge or workshop crates, `promptforge-*` never on gateway or workshop crates, `shared-*` depends on no product crates, and crates outside the promptforge family may depend only on `promptforge-api` (the one door); tiers flow shell -> features -> services -> vocabulary
- Conventions summary: Rust edition 2024 on the stable toolchain (`rust-toolchain.toml`); `unsafe_code` forbidden workspace-wide; clippy `all` denied and `pedantic` warned with `unwrap_used`/`expect_used` denied; no file exceeds 500 lines (split first, then edit; `build-xtask` enforces); behavior changes ship with tests in the same change; error and status messages are written concise and self-contained for model consumption; every `workshop-*` lib.rs opens with a `## Invariants` doc listing allowed dependencies; UI CSS lives beside its TypeScript and uses `--ws-*` design tokens, never raw values

</project-survey>
<execution-plan>

## Execution Instructions

Components in dependency order:

1. `gateway-reader` (DEBT-MTCU-3, DEBT-MTCU-4 reader side): placed first because its module split of `crates/gateway/src/cloud_models.rs` must happen exactly once before any other edit to that file, and because `schema-policy` documents the constant this component introduces.
2. `refresh-delivery` (DEBT-MTCU-1): placed second because it edits the same gateway file after the split, and its UI half stubs the POST contract its gateway half defines.
3. `e2e-alignment` (DEBT-MTCU-2 test half): independent of every other component; placed third as test-only and lowest risk.
4. `schema-policy` (DEBT-MTCU-4 discipline): placed last because it documents the shared constant and types the reader component establishes.

<step-1>

### Step 1: Split cloud_models.rs under the file ceiling [completed]

- Component: gateway-reader

Move the inline `#[cfg(test)]` tests (or the download and cache logic) out of `crates/gateway/src/cloud_models.rs` (711 lines) into a sibling module such as `crates/gateway/src/cloud_models/tests.rs`, following the `bedrock/sigv4.rs` sibling-module precedent. No behavior change. Verify: `cargo nextest run -p gateway cloud_models` stays green and `cargo test -p build-xtask` passes the ceiling check.

</step-1>

<step-2>

### Step 2: Share the accepted sheet schema version [completed]

- Component: gateway-reader

Add `pub const ACCEPTED_SHEET_SCHEMA_VERSION` (value 1) beside `Sheet` in `crates/shared-gateway-api/src/lib.rs`; change `crates/shared-cloud-providers/src/sheet.rs` to use it instead of its private `SCHEMA_VERSION` (line 17). Add a test in `shared-gateway-api` or `shared-cloud-providers` asserting the emitted sheet's `schema_version` equals the shared constant. Pieces in this component are sequential: the constant must exist before the gate in step 4 can reference it.

</step-2>

<step-3>

### Step 3: Cap the sheet download body [completed]

- Component: gateway-reader

In `download_once` (`crates/gateway/src/cloud_models.rs` or its split sibling), replace the `shared_cloud_providers::fetch_sheet` call with `bounded_client().get(url).send()`, `error_for_status()`, a `Content-Length` precheck, `read_bytes_capped(response, MAX_JSON_BODY)` from `crates/gateway-protocol/src/http_util.rs`, and `serde_json::from_slice::<Sheet>`. Add a `GatewayError` variant in `crates/gateway/src/error.rs` for the over-cap body, classified 502 on the download path. Add the route test whose stub body exceeds `MAX_JSON_BODY`, asserting a recorded cap error, no sheet swap, and an unchanged cache file.

</step-3>

<step-4>

### Step 4: Gate the sheet on its schema version [completed]

- Component: gateway-reader

Check `sheet.schema_version` against the shared constant after every successful parse in both `read_cache` and `download_once`. Add a `GatewayError` variant carrying the found and accepted versions. On the cache path treat a mismatch as absent (logged, overwritten by the next good download), matching `CacheRead::Unparseable`; on the download path record it as `last_error` and keep any in-memory sheet. Tests: a route test whose stub sheet carries `schema_version: 2` asserts `last_error` names 2 and the accepted version; a cache-read test with a version-mismatched file asserts it is treated as absent and a download is spawned.

</step-4>

<step-5>

### Step 5: Refresh POST awaits and answers [completed]

- Component: refresh-delivery

Change `admin_cloud_models_refresh` in `crates/gateway/src/cloud_models.rs` to await the download it starts or joins and answer 200 with the sheet on success or the `CloudModelsUnavailable` 502 on failure; keep the in-flight guard so concurrent refreshes share one download. Tests: the stub serves sheet A at launch and sheet B next, asserting the POST answers 200 with B and the GET then serves B; the stub serves A then fails, asserting 502 with the error and the GET still serving A; two concurrent refresh POSTs produce one stub request. Sequential before step 6: the store test stubs the POST contract this step defines.

</step-5>

<step-6>

### Step 6: Sheet store consumes the POST answer [completed]

- Component: refresh-delivery

In `crates/gateway-config-ui/ui/src/services/sheet-store.ts`, make `SheetStore.refresh` use the POST's answer directly: on 200 store the sheet and notify; on failure record the error, notify, and rethrow so the view's existing catch toasts. Remove the post-refresh `poll` call. Update the refresh return type in `crates/gateway-config-ui/ui/src/services/gateway-api.ts`. Replace the refresh test in `sheet-store.test.mjs`: the stub returns the old sheet from GET and the new sheet from the POST; assert the store ends on the new sheet's `generated_at`.

</step-6>

<step-7>

### Step 7: Align the e2e merge test to the UI [completed]

- Component: e2e-alignment

In `crates/gateway/tests/it/cloud_models.rs`, remove the profile push from `merge_cloud_model` (lines 196 to 204) so it performs exactly the UI's two array appends. Keep the fixture's `[[profile]]` block (line 118): the gateway cannot boot without a defined, selected profile, and a selected profile's empty `models` list narrows the catalog to empty. Change the assertions to the behavior the UI actually produces: the merged model validates and applies, and it is absent from `GET /v1/models` under the empty profile. Add a comment that profile membership is not part of the add flow and that the behavior half of DEBT-MTCU-2 is deferred. Verify: `the_sheet_downloads_and_merged_models_apply_end_to_end` passes in this shape, and a diff of `merge_cloud_model` against `mergeCloudModel` in `crates/gateway-config-ui/ui/src/services/cloud-merge.ts` shows the same two array appends.

</step-7>

<step-8>

### Step 8: Document the additive-field policy

- Component: schema-policy

In `crates/shared-gateway-api/src/lib.rs`, record in the doc comments beside `schema_version` and on `Sheet`, `ProviderSlice`, and `ModelEntry` that future additive fields carry `#[serde(default)]` and that the version bump is reserved for removals and renames; `schema_version` stays 1 and no existing field changes. Verify by reading the doc comments and by the step 2 constant test asserting the emitted version is 1.

</step-8>

Final gates after the last step: `cargo nextest run -p gateway -p shared-gateway-api -p shared-cloud-providers`; `npm run build && npm run typecheck && npm test` in `crates/gateway-config-ui/ui`; clippy `-D warnings`; `cargo fmt --all --check`; `cargo test -p build-xtask`. Exclusions: no change to `forward_allowed`, to profile semantics, to the publisher workflow, or to design records.

</execution-plan>