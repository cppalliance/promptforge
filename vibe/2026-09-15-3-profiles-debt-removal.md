---
name: Profiles debt removal
overview: Remove the three debts the Profiles-gate-local plan (c17173ce..73b17e66) introduced in promptforge - readers of the unselected live config after an apply, the non-round-trippable GET/PUT /admin/config document, and the workbench frame that cannot show an in-flight switch to no profile - each as a concrete fix with its proving check.
todos:
  - id: pgl-001-orphans-status
    content: "PGL-001: admin_orphans diffs against catalog_local_models/catalog_stt_models; admin_status configured closure reads catalog_local_models; fix orphans.rs docs and guide 11 line 35; add post-apply orphan and configured tests"
    status: pending
  - id: pgl-002-config-roundtrip
    content: "PGL-002: remove active_profile insertion from admin_config; drop boot_speech_tests strip; add GET-then-PUT round-trip test and config-store empty-diff UI test"
    status: pending
  - id: pgl-003-switch-in-flight
    content: "PGL-003: add switch_in_flight to WorkbenchSnapshot and snapshot(); thread through protocol.ts, workbench-service.ts, window-menu.ts; remove Known gap notes; update frames.rs, workbench-frames.mjs, and window-menu.mjs tests"
    status: pending
  - id: gate
    content: Run full regression list, regenerate guide exports with build-user-guide, confirm exit rg checks are empty
    status: pending
isProject: false
---

# Profiles Gate Local: Debt Removal

<product-contract>

## Product Requirements

Three debts introduced by the seven commits `c17173ce..73b17e66` on `master` of repository `promptforge` (`c:\Users\Vinnie\cursor\promptforge`) are removed. After that work, a config apply publishes a live configuration document with no profile selected, and two readers of its selected subsets were never re-pointed; the running document `GET /admin/config` returns is refused by `PUT /admin/config`; and the Workshop's `workbench` frame cannot distinguish an in-flight switch to no profile from no switch. Each debt is fixed by a small change whose proving check fails at `73b17e66` and passes afterward. Nothing else about the profiles design changes.

- Problem and users: gateway operators who open the config UI after applying any configuration change see every cached model file, including the running profile's own children, listed under Orphans with a Delete button (`crates/gateway/src/orphans.rs` 41-50 diffs against `config.local_models()` and `config.stt_models()`, both empty on the applied document), and see the local endpoint's LED lose its provisioning state (`crates/gateway/src/lib.rs` 1358-1368 `configured` reads `live.config.local_models()`); API clients and the gateway's own tests that round-trip `GET /admin/config` into `PUT /admin/config` are refused with 422 because the GET body carries `active_profile` (`lib.rs` 1459-1474) while `save_config_shadow` in `crates/gateway-config/src/shadow.rs` refuses that key; Workshop users who pick "No profile" see every Model menu row stay enabled and unmarked for the length of the restart ladder and get an `error` frame on a second pick (`crates/workshop-server/ui/src/ui/menu/window-menu.ts` 153, 210).
- Goals:
  - `GET /admin/orphans` lists only cache files that no `[[local_model]]` or `[[stt_model]]` declared in the catalog references, under any profile and before or after an apply (DEBT-PGL-001).
  - `GET /admin/status` reports a local endpoint as `configured` when the catalog declares a local model of that kind, before or after an apply (DEBT-PGL-001).
  - The document `GET /admin/config` returns is accepted verbatim by `PUT /admin/config` and carries no `active_profile` key; the config UI's Review diff shows no phantom `active_profile` row (DEBT-PGL-002).
  - The `workbench` frame carries an explicit `switch_in_flight` boolean; while a switch to no profile runs, every Model menu row is disabled and the "No profile" row shows the pending mark (DEBT-PGL-003).
- Non-goals: unwinding the copy-per-field `LiveState` design (`profile_name`, `model_allowlist`, `stt_vram_gb`, `local`); changing `save_config_shadow`'s refusal of `active_profile`; changing the shape or meaning of the existing `switching` field; the inbound `switch_profile` frame; any of the fifteen rejected candidates listed in the Decision Record; architecture record updates in `vibe/archdoc.md`; any change to `config-version` or the cloud sheet `schema_version`.
- Success criteria: the new tests in the Testing Plan fail at `73b17e66` and pass after the change; existing suites for `gateway`, `gateway-config`, `workshop-protocol`, `workshop-menu`, `workshop-sessions`, `workshop-server`, and both UIs stay green; `rg 'insert\("active_profile"' crates/gateway/src/lib.rs` returns nothing; `rg 'remove\("active_profile"\)' crates/gateway/src/lib.rs` returns nothing; `rg "Known gap" crates` returns nothing.
- Constraints:
  - Base commit `73b17e66` on `master`, clean worktree. Line numbers in this plan are as of that commit.
  - Edition 2024, clippy `-D warnings`, `cargo fmt --all --check`. `cargo test -p build-xtask` fails any `.rs` file over 500 raw lines inside `workshop-*` crates, so `crates/workshop-menu/src/menu.rs` and `crates/workshop-protocol/src/workbench.rs` must stay under that ceiling after the field is added.
  - The Workshop's UI `ProfileMenuService.switching` is typed `string` (`window-menu.ts` 103-113) while the frame's `switching` is `string | null` (`services/protocol.ts` 48-56); the adapter between them lives outside `protocol.ts`, `workbench-service.ts`, and `window-menu.ts` and must be located before wiring the new field.
  - Every wire change here is additive or subtractive with in-repo consumers only: the config UI reads the running profile from `GET /admin/status` `profile` (`crates/gateway-config-ui/ui/src/services/config-store.ts` 254), never from the running document's `active_profile`.
- Open questions: None.

## Functional Specification

Observable behavior changes at three surfaces: two gateway admin routes, one gateway admin document, and the Workshop's Model menu. No new route, frame type, or key is introduced except the `switch_in_flight` boolean on the existing `workbench` frame.

- Actors and workflows:
  - Operator applies a `[[model]]`-only change in the config UI: the Orphans list is unchanged, and no running child's artifact is offered for deletion. Operator declares a second profile whose `[[local_model]]` is not in the running profile: its cached artifact is not an orphan.
  - Operator reads `GET /admin/status` after an apply: an endpoint serving only local chat models still reports `configured: true` and, while its child provisions, the provisioning state.
  - API client or test fetches `GET /admin/config` and PUTs the body unchanged: `PUT /admin/config` answers 200 `{"shadow"}`. The running document never contains `active_profile`; the running profile is read from `GET /admin/status` `profile` and the persisted selection from `GET /admin/config-pending` `profile.active_profile`.
  - Config UI user opens the Review diff with a profile running and nothing staged: the diff is empty.
  - Workshop user picks "No profile" from the Model menu: the `workbench` frame reports `switching: null, switch_in_flight: true`; every model and profile radio is disabled and the "No profile" row shows the pending mark until the switch settles; a named profile pick reports `switching: "<name>", switch_in_flight: true` with the pending mark on that row, as today.
- Inputs and outputs: `GET /admin/orphans` keeps its shape; its `configured` set is every `[[local_model]]` and `[[stt_model]]` in the catalog. `GET /admin/status` keeps its shape. `GET /admin/config` returns `live.config.to_json()` with no `active_profile` key. The outbound `workbench` frame gains `switch_in_flight: bool` (`true` while `workshop-menu` holds a switch target, `false` otherwise); `switching` keeps its `string | null` value and meaning.
- States and validation: no new states. `switch_in_flight` is `true` exactly when `switching` names a profile or the in-flight target is no profile.
- Errors and recovery: unchanged. `PUT /admin/config` still refuses a body carrying `active_profile` with the existing config-write error; the Workshop server still answers a second pick during a switch with `MenuRefusal::SwitchInProgress`, now behind a disabled row.
- Security and privacy behavior: no route, bearer, loopback, or trust-boundary change.
- Acceptance criteria: after a `[[model]]`-only apply, `GET /admin/orphans` lists no declared model's artifact and `GET /admin/status` still reports the local endpoint configured; a declared but unselected local model's artifact is never an orphan; `GET /admin/config` then `PUT /admin/config` of the same body succeeds; the UI `pendingDiff()` is empty when pending equals running while a profile runs; `begin_switch(None)` yields `switching: null, switch_in_flight: true` in the snapshot; the Workshop menu disables all rows and marks "No profile" pending when `switchInFlight` is true and `switching` is null; `frames.rs` and `workbench-frames.mjs` round-trip the new field.

</product-contract>
<implementation-contract>

## Technical Design

Two gateway readers move from the live document's selected subsets to its catalog-wide accessors, the running document loses a key the server never accepted back, and the Workshop's workbench frame gains one additive boolean threaded through three UI files. Every change is confined to `crates/gateway`, `crates/workshop-protocol`, `crates/workshop-menu`, and `crates/workshop-server/ui`, plus one guide sentence.

- Architecture:
  - Live configuration after an apply (DEBT-PGL-001): `capture_apply` in `crates/gateway/src/config_apply.rs` keeps publishing the applied document with `select_profile(None)`, so `live.config.local_models()` and `live.config.stt_models()` are empty for the rest of the process; every reader that needs the declared local or STT set reads `catalog_local_models()` and `catalog_stt_models()` instead, which do not move on an apply. The running children stay described by `live.local`, `live.profile_name`, `live.model_allowlist`, and `live.stt_vram_gb`.
  - Running document (DEBT-PGL-002): `GET /admin/config` returns the document as `Config::to_json()` produces it. The running profile is exposed only by `GET /admin/status` and the persisted selection only by `GET /admin/config-pending`, matching the rule that `active_profile` is not a configuration key.
  - Workbench frame (DEBT-PGL-003): `workshop-menu` already tracks `SwitchTarget::{Profile, NoProfile}`; its snapshot exposes both the target name (`switching`) and the fact that a target exists (`switch_in_flight`), so the UI no longer infers idleness from the name alone.
- Modules and interfaces:
  - `crates/gateway/src/orphans.rs` `admin_orphans` (41-50): pass `config.catalog_local_models()` to `gateway_local::cache::orphans` and build `stt_sources` from `config.catalog_stt_models()`. The module doc (1-3), function doc (22-23), and the comment at 36-38 state that an orphan is a cache file no `[[local_model]]` or `[[stt_model]]` declared in the catalog references.
  - `crates/gateway/src/lib.rs` `admin_status` `configured` closure (1358-1368): the local arm reads `live.config.catalog_local_models().iter().any(|model| model.kind() == kind)`; the remote arm over `models()` is unchanged; the result still feeds `endpoint_status` as its third argument (1370-1379).
  - `crates/gateway/src/lib.rs` `admin_config` (1459-1474): return `live.config.to_json()`; delete the `active_profile()` match and its `table.insert`. `boot_speech_tests` (3116-3120) drops the `remove("active_profile")` line, so the test round-trips the document as served.
  - `crates/workshop-protocol/src/workbench.rs` `WorkbenchSnapshot` (18-30): add `pub switch_in_flight: bool` beside `switching: Option<String>`; the "Known gap" paragraphs (11-16, 23-25) become one sentence documenting the pair (`switching` names the target when it is a profile; `switch_in_flight` is true for any in-flight target, including no profile).
  - `crates/workshop-menu/src/menu.rs` `snapshot` (382-399): set `switch_in_flight: state.switching.is_some()`; `switching` keeps `state.switching.as_ref().and_then(|target| target.name().map(str::to_owned))`.
  - `crates/workshop-server/ui/src/services/protocol.ts` `WorkbenchFrame` (48-56): add `switch_in_flight: boolean`; delete the gap comment at 43-44. `services/workbench-service.ts` `WorkbenchSnapshot` and `applySnapshot` (16-22, 60-64): carry `switchInFlight: frame.switch_in_flight`. `ui/menu/window-menu.ts`: `ProfileMenuService` (103-113) gains `switchInFlight: boolean` and loses the exception note at 109-111; `isIdle` (153) becomes `!profileService?.switchInFlight`; each radio row keeps `enabled: isIdle` (167); the "No profile" row's `isPending` (210) becomes `!isIdle() && !profileService.switching`; named rows keep `!isIdle() && profile === profileService.switching` (221). The adapter that composes `ProfileMenuService` from the workbench snapshot (outside these three files; it maps `switching: null` to `""`) forwards `switchInFlight`.
- File and public API changes:
  - Wire: `GET /admin/config` loses the `active_profile` key (subtractive; in-repo consumers read `GET /admin/status` `profile`). The outbound `workbench` frame gains `switch_in_flight: bool` (additive; consumers are `protocol.ts`, `workbench-service.ts`, `window-menu.ts`, and the adapter). `GET /admin/orphans` keeps its shape and widens its `configured` set to the catalog. No inbound frame or request body changes.
  - Rust public API: `workshop_protocol::WorkbenchSnapshot` gains a public field. No `gateway` or `gateway-config` signature changes.
  - Files: `crates/gateway/src/{orphans.rs,lib.rs}` and their tests; `crates/workshop-protocol/src/workbench.rs`; `crates/workshop-menu/src/menu.rs` and tests; `crates/workshop-server/ui/src/{services/protocol.ts,services/workbench-service.ts,ui/menu/window-menu.ts}`, the `ProfileMenuService` adapter, and `crates/workshop-server/ui/test/{window-menu.mjs,workbench-frames.mjs}`; `crates/workshop-protocol/tests/it/frames.rs`; `crates/gateway-config-ui/ui/src/services/config-store.test.mjs`; `guide/src/gateway/11-serving-and-observing.md` line 35 and the regenerated `guide/promptforge-gateway-guide.md`.
- Data, persistence, failure, security, and privacy constraints: no persisted format, queue, error path, or trust boundary changes. `guide/src/gateway/09-editing-configuration.md` line 9 ("PUT takes the same JSON shape that GET returns") becomes true again and stays as written; line 11 (the refusal note) and `crates/gateway/README.md` line 61 are unchanged.

</implementation-contract>
<verification-contract>

## Testing Plan

Each debt has a proving check that fails at `73b17e66` and passes after the change, plus the existing suites of every touched crate and both UIs as regression. Exit is gated on three `rg` sweeps.

- Unit:
  - DEBT-PGL-001, `crates/gateway` (`orphans.rs` or the `lib.rs` apply tests, `--features test-fixtures`): boot with a profile whose local model has a cached artifact, apply a `[[model]]`-only shadow, then assert `GET /admin/orphans` lists no declared model's artifact and `GET /admin/status` still reports the local endpoint `configured`. Second case: a `[[local_model]]` declared in the catalog but absent from the running profile has a cached artifact that is not listed as an orphan.
  - DEBT-PGL-002, `crates/gateway`: fetch `GET /admin/config`, PUT the body unchanged, assert 200 and a reply carrying `shadow`; `boot_speech_tests` passes without its strip. `crates/gateway-config-ui/ui/src/services/config-store.test.mjs`: with `status.profile = "alpha"` and a running document equal to the pending document, `pendingDiff()` is empty.
  - DEBT-PGL-003: `crates/workshop-menu` test that `begin_switch(None)` snapshots `switching: None, switch_in_flight: true`, `begin_switch(Some("beta"))` snapshots `switching: Some("beta"), switch_in_flight: true`, and `finish_switch` returns both to `None, false`. `crates/workshop-protocol/tests/it/frames.rs` serializes and accepts `switch_in_flight`. `crates/workshop-server/ui/test/workbench-frames.mjs` carries the field in its frame helper. `crates/workshop-server/ui/test/window-menu.mjs`: with `switching` empty and `switchInFlight` true, every model and profile radio is disabled and the "No profile" row is pending; with `switchInFlight` false, all rows are enabled and none is pending.
- Integration and end-to-end: `crates/workshop-server/tests/it/session/menu.rs` and `menu/restart.rs` assert the `workbench` frames they already observe carry `switch_in_flight` true during the ladder and false at `Completed`.
- Regression, security, and performance: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features` and the matching `--doc` run; `cargo nextest run --locked -p workshop -p workshop-server` and `cargo test --doc -p workshop -p workshop-server`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` and `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo test -p build-xtask`; `npm run build && npm run typecheck && npm test` in `crates/gateway-config-ui/ui` and `crates/workshop-server/ui`; `mdbook build guide`; `cargo run -p build-user-guide` followed by `git diff --exit-code -- guide/`. No performance-sensitive path changes.
- Exit criteria: all above pass; `rg 'insert\("active_profile"' crates/gateway/src/lib.rs`, `rg 'remove\("active_profile"\)' crates/gateway/src/lib.rs`, and `rg "Known gap" crates` each return nothing.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - DEBT-PGL-001 is fixed by re-pointing the two readers to catalog accessors, not by re-selecting the running profile on the applied document. The applied document's profile members can differ from the children actually running, so `live.config.local_models()` would still not describe the runtime; the catalog is stable across applies and matches the profile-as-checklist semantics.
  - `GET /admin/orphans` adopts catalog semantics ("no declared `[[local_model]]` or `[[stt_model]]` references it"). Consequence: another profile's declared artifacts are never offered as deletable orphans, which also settles a pre-existing gap where the route diffed against the running profile only while the guide said "configured".
  - The `configured` closure reads `catalog_local_models()`, not `model_allowlist`: the closure tests `model.kind() == kind` and `model_allowlist` is `Option<Vec<String>>` of names (`crates/gateway/src/lib.rs` 206-207), which cannot answer a kind query.
  - DEBT-PGL-002 removes `active_profile` from `GET /admin/config` rather than making `save_config_shadow` ignore it or rewording the guide. User: "A A" (2026-09-15) choosing option A for both escalated items. Rationale: the plan that introduced the refusal and `crates/gateway/README.md` line 61 already declare the key not a configuration key; the key already vanished after the first apply, so no consumer could rely on it.
  - DEBT-PGL-003 adds an additive `switch_in_flight: bool` rather than changing `switching` to a tri-state object or leaving the documented gap. User: "A A" (2026-09-15). Rationale: `switching` keeps its shape and meaning for every existing consumer; the boolean is the smallest field that separates "no switch" from "switch to no profile".
- Rejected alternatives:
  - `select_profile(live.profile_name)` on the applied document: the applied profile's members may differ from the running children; revisit only if a reader needs the applied profile's member list specifically.
  - Diffing orphans against `live.local.models()` ("loaded" semantics): STT artifacts have no runtime handle on `LiveState` beyond `stt_vram_gb`, and profile-switch-by-restart makes the other profiles' artifacts legitimate residents of the cache; revisit if operators ask for a "not loaded now" view.
  - Adding a `running_selection` field to `LiveState`: a third copy of a fact already held by `model_allowlist` and `local`; revisit never.
  - `save_config_shadow` ignoring `active_profile` (PGL-002 option B): reverses an explicit prior decision and leaves the phantom UI diff row; rejected by the user.
  - Rewording the guide to document the GET/PUT asymmetry (PGL-002 option C): leaves the round trip broken; rejected by the user.
  - `switching` as a tri-state object such as `{"target": null}` (PGL-003 option B): breaks the existing field for every consumer; rejected by the user.
  - Leaving the documented gap with `SwitchInProgress` as the only guard (PGL-003 option C): rejected by the user.
  - Reading `chat_ready` for idleness: it is false for a missing model, an unreachable gateway, and an in-flight switch alike (`crates/workshop-protocol/src/workbench.rs` 7-9), so it would disable the menu in states where selection should be allowed.
- Assumptions, risks, and notes:
  - The debt inventory derives from a static trace of `c17173ce..73b17e66` at endpoint `73b17e66` with a clean worktree; the post-apply orphan listing and the phantom diff row were traced through code, not observed in a running system. Falsifier for PGL-001: the new test passes at `73b17e66` without the fix.
  - Fifteen candidates were rejected and are not work items: nine residual-but-acceptable (documented `expect(dead_code)` on `Enqueued.operation` in `crates/gateway/src/commands.rs`; duplicated `NO_PROFILE_LABEL` in two config UI files; unread `SwitchOutcome.profile` in the config UI; inlined `t.after` teardown in two UI tests; feature-gated test seams `ResolvedGateway::with_base_url`, `state_with_gateway_and_restart_bound`, `SessionsState::with_restart_bound`; 404 `profile_not_found` on Set Active for a staged-but-unapplied profile; `crates/gateway/src/lib.rs` at 4,175 lines; the Workshop proxy's `/admin/*` forwarding rule; an apply blocking behind the boot load), four weak/speculative (restart ladder misreport when the sidecar inherits `PROMPTFORGE_PROFILE`; removed quit-cancels-active-command IT coverage; `merge-routing` name collision between a new `[[model]]` and a dropped running child; no menu indicator after a Deferred LAN selection), two unrelated pre-existing (`load_pending_for_running` answering 500 when a shadow deletes the running profile; the two boot paths `Gateway::new` and `Gateway::from_config`).
  - Risk: an out-of-repo client reading `active_profile` from `GET /admin/config` breaks. None is known; the README already declares the key not a configuration key.
  - Risk: the `ProfileMenuService` adapter is outside the three UI files named; if it derives `switching` from more than the frame, `switchInFlight` must be derived from the frame alone.
  - Note: `crates/workshop-menu/src/menu.rs` and `crates/workshop-protocol/src/workbench.rs` are under the enforced 500-line ceiling today; the added field and doc sentence must keep them there. The ceiling also scans test files in `workshop-*` crates: `crates/workshop-server/tests/it/session/menu/restart.rs` is 434 lines and `crates/workshop-server/tests/it/session/menu.rs` is 330 lines at `73b17e66`, so the added frame assertions must not push either past 500 (split a sibling module if needed).
  - Note: at `73b17e66` the local `master` is five commits ahead of `origin/master` (`c17173ce`); the unpushed commits are the base this plan builds on. Pushing is the operator's call and not part of this plan.

### Deferred and Out of Scope

- Deferred: consolidating `LiveState`'s per-field copies (`profile_name`, `model_allowlist`, `stt_vram_gb`) into one running-selection value; revisit when a third reader needs the running selection.
- Out of scope: the fifteen rejected candidates above; `save_config_shadow` behavior; the shape of `switching`; the inbound `switch_profile` frame; `vibe/archdoc.md`; `config-version` and cloud sheet `schema_version`.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build command: `cargo build --locked -p gateway` (default member; workshop desktop is an explicit `cargo build --locked -p workshop`; headless gateway check `cargo check -p gateway --no-default-features`).
- Focused test command pattern: `cargo nextest run --locked -p <crate> -E 'test(<name>)'`, or for the per-crate integration binary `cargo test -p <crate> --test it <filter>` (CI example: `cargo test -p gateway-stt --test it architecture`).
- Component test command pattern: `cargo nextest run --locked -p <crate> --all-features` for non-workshop crates; `cargo nextest run --locked -p workshop -p workshop-server` for workshop crates (`--features headless` variant for workshop-server); SPA packages run `npm test` inside `crates/workshop-server/ui` or `crates/gateway-config-ui/ui` (`node --test "src/**/*.test.mjs"`).
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`, then `cargo nextest run --locked -p workshop -p workshop-server` and `cargo test --doc -p workshop -p workshop-server`; boundary and structural harness `cargo test -p build-xtask`.
- Linter command: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; SPA: `npm run typecheck` (`tsc --noEmit`) in each ui package; pre-push also runs `cargo deny check` when installed.
- Formatter check command: `cargo fmt --all --check` (enforced by `.githooks/pre-commit`).
- Docs command: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide `mdbook build guide`.
- Test placement and naming conventions: Rust unit tests live inline under `#[cfg(test)] mod tests` (304 files); integration tests use one binary per crate at `crates/<crate>/tests/it/` with a `main.rs` module tree and per-topic files (`boot.rs`, `chat.rs`, ...), plus optional `tests/common/` helpers; `promptforge-api` uses `tests/suite/` with `tests/prompts/` fixtures; benches in `crates/<crate>/benches/`; SPA tests are `*.test.mjs` beside sources under `ui/src/` or in `ui/test/`. Nextest config in `.config/nextest.toml` (60s slow timeout, `heavy` group for tool-picker and STT crates).
- Directory map: `crates/` holds all workspace members (`gateway-*` inference service, `promptforge-*` runtime and API, `workshop-*` desktop product, `shared-*` cross-product substrate, `build-*` build tooling including `build-xtask`; `crates/shared-ui` is a TypeScript+CSS package excluded from Cargo); `guide/` is the mdBook user guide (`src/agent`, `gateway`, `language`, `workshop`); `prompts/` are example prompt pipelines; `tools/` has Node sidecar staging scripts and their `.test.mjs`; `vibe/` holds `archdoc.md` and dated plan records; `local/` holds untracked local config, profiles, and fixtures; `.githooks/` has pre-commit and pre-push; `.github/workflows/` has `ci.yml`, nightly, release, guide, and platform-specific workflows; `.cargo/config.toml` defines `cargo workshop` and `cargo xtask` aliases; `target/` and `target-msrv/` are build artifacts.
- Component boundaries: executor (`promptforge-api` as the one door; internal `promptforge-parser`, `promptforge-lua`, `promptforge-model-client`, `promptforge-store`, `promptforge-vfs`, `promptforge-web*`, `promptforge-tool-picker`) depends on gateway protocol, store, Lua VM boundary, shared substrate; gateway (`gateway`, `gateway-config`, `gateway-config-ui`, `gateway-local`, `gateway-logging`, `gateway-protocol`, `gateway-routing`, `gateway-stt*`, `gateway-web-search`, `gateway-whisper-ffi`) depends only on shared substrate and must not depend on promptforge or workshop crates; workshop (`workshop`, `workshop-server`, `workshop-gateway`, `workshop-menu`, `workshop-protocol`, `workshop-registry`, `workshop-sessions`, `workshop-status`, `workshop-support`, `workshop-workspace`) depends on executor, gateway, store, shared substrate and must not depend on gateway crates directly per AGENTS.md; store facade sits over the VFS layer (`shared-vfs` + `promptforge-vfs`); shared substrate (`shared-progress`, `shared-loopback`, `shared-sidecar`, `shared-gateway-api`, `shared-promptforge-api`, `shared-cloud-providers`) depends on no product crate. Workshop internal tiers flow shell -> features -> services -> vocabulary; crates outside `promptforge-*` may depend only on `promptforge-api`. `cargo test -p build-xtask` enforces the tier graph, lint inheritance, 500-line ceiling, and product-boundary matrix.
- Conventions summary: Rust edition 2024, stable toolchain, resolver 3, workspace lints (`unsafe_code = "forbid"`, `missing_docs`, `unreachable_pub`, clippy `all` deny, `pedantic` warn, `unwrap_used` and `expect_used` deny, rustdoc broken links deny); no file over 500 lines; every `workshop-*` lib.rs and SPA concern `index.ts` opens with a `## Invariants` doc marker; behavior changes ship with tests in the same change, structural checks need explicit approval; Cargo features gate real constraints only; long-running work reports via `shared-progress`; error messages written for model consumption (required versus actual); comments cite upstream issue URLs for workarounds; SPA CSS lives beside its TypeScript with `--ws-*` tokens from `tokens/` (base, semantic, component) and no raw values; static CRT on Windows MSVC; `cargo deny` and `cargo audit` in CI; per-crate `AGENTS.md` and `README.md` files.

</project-survey>
<execution-plan>

## Execution Instructions

Base: `95116424` on `master`, clean worktree. `95116424` is the rebased twin of `73b17e66` (same subject `Close plan: Profiles gate local`); the two trees differ only inside `crates/shared-cloud-providers`, which this plan never touches, so every line number and every "fails at `73b17e66`" claim below holds at `95116424`. Do not check out `73b17e66`. Two components in dependency order: `gateway` first (lowest layer; the workshop depends on the gateway and the guide regeneration rides with the gateway's guide edit), then `workshop` (the only cross-crate wire change, Rust before UI so the UI test helper mirrors a field that exists). The two `gateway` pieces (Steps 1 and 2) are independent but both edit `crates/gateway/src/lib.rs`, so they land as serial commits. The `workshop` pieces are sequential: Step 4 consumes the frame field Step 3 adds. Exclusions: the fifteen rejected candidates; `save_config_shadow`; the shape of `switching`; `LiveState` field consolidation; `vibe/archdoc.md`; the pre-existing orphan-semantics gap is not its own item (Step 1's second test pins it).

<step-1>

### Step 1: Catalog-wide orphan and configured readers (DEBT-PGL-001) [completed]

- Component: gateway
- Piece: admin readers of the applied live document
- Depends on: nothing
- Changes:
  - `crates/gateway/src/orphans.rs` `admin_orphans` (41-50): pass `config.catalog_local_models()` to `gateway_local::cache::orphans`; build `stt_sources` from `config.catalog_stt_models()`. Rewrite the module doc (1-3), the function doc (22-23), and the comment at 36-38 so each states that an orphan is a cache file no `[[local_model]]` or `[[stt_model]]` declared in the catalog references.
  - `crates/gateway/src/lib.rs` `admin_status` `configured` closure (1358-1368): the local arm reads `live.config.catalog_local_models().iter().any(|model| model.kind() == kind)`; the remote arm over `models()` and the `endpoint_status` call (1370-1379) are unchanged.
  - `guide/src/gateway/11-serving-and-observing.md` line 35: a cache file is an orphan when no `[[local_model]]` or `[[stt_model]]` declared in the catalog references it. Run `cargo run -p build-user-guide` and include the regenerated `guide/promptforge-gateway-guide.md` in this commit.
- Tests (`crates/gateway`, `--features test-fixtures`, in `orphans.rs` tests or the `lib.rs` apply tests; both must fail at `73b17e66`):
  - Boot with a profile whose local model has a cached artifact, apply a `[[model]]`-only shadow, then assert `GET /admin/orphans` lists no declared model's artifact and `GET /admin/status` still reports the local endpoint `configured: true`.
  - A `[[local_model]]` declared in the catalog but absent from the running profile has a cached artifact; assert it is not listed as an orphan.
- Verification: `cargo nextest run --locked -p gateway --lib --all-features -E 'test(orphan) | test(configured)'`; `mdbook build guide`; `cargo run -p build-user-guide && git diff --exit-code -- guide/` after staging.
- Commit: one commit with the two source edits, the guide source edit, the regenerated export, and the two tests.

</step-1>

<step-2>

### Step 2: Round-trippable running document (DEBT-PGL-002) [completed]

- Component: gateway
- Piece: `GET /admin/config` document shape
- Depends on: Step 1 only for commit ordering in `lib.rs`; no code dependency
- Changes:
  - `crates/gateway/src/lib.rs` `admin_config` (1459-1474): return `live.config.to_json()`; delete the `active_profile()` match and its `table.insert`.
  - `crates/gateway/src/lib.rs` `boot_speech_tests` (3116-3120): delete the `remove("active_profile")` line so the test round-trips the document as served.
  - No change to `save_config_shadow` in `crates/gateway-config/src/shadow.rs`, `guide/src/gateway/09-editing-configuration.md` lines 9 and 11, or `crates/gateway/README.md` line 61.
- Tests:
  - `crates/gateway` (`--features test-fixtures`): fetch `GET /admin/config`, `PUT /admin/config` the body unchanged, assert 200 and a reply carrying `shadow`. Must fail at `73b17e66`.
  - `crates/gateway-config-ui/ui/src/services/config-store.test.mjs`: with `status.profile = "alpha"` and a running document equal to the pending document, `pendingDiff()` is empty.
- Verification: `cargo nextest run --locked -p gateway --lib --all-features -E 'test(config)'`; `node --test src/services/config-store.test.mjs` in `crates/gateway-config-ui/ui`; `rg 'insert\("active_profile"' crates/gateway/src/lib.rs` and `rg 'remove\("active_profile"\)' crates/gateway/src/lib.rs` each return nothing.
- Commit: one commit with the two `lib.rs` edits, the round-trip test, and the UI test case.

</step-2>

<step-3>

### Step 3: `switch_in_flight` on the workbench frame (DEBT-PGL-003, Rust) [completed]

- Component: workshop
- Piece: protocol and menu snapshot (sequential; Step 4 consumes this field)
- Depends on: nothing in `gateway`
- Changes:
  - `crates/workshop-protocol/src/workbench.rs` `WorkbenchSnapshot` (18-30): add `pub switch_in_flight: bool` beside `switching: Option<String>`. Replace the "Known gap" paragraphs (11-16, 23-25) with one sentence: `switching` names the target when it is a profile; `switch_in_flight` is true for any in-flight target, including no profile. Keep the file under 500 lines.
  - `crates/workshop-menu/src/menu.rs` `snapshot` (382-399): set `switch_in_flight: state.switching.is_some()`; `switching` keeps `state.switching.as_ref().and_then(|target| target.name().map(str::to_owned))`. Keep the file under 500 lines.
- Tests:
  - `crates/workshop-menu`: `begin_switch(None)` snapshots `switching: None, switch_in_flight: true`; `begin_switch(Some("beta"))` snapshots `switching: Some("beta"), switch_in_flight: true`; `finish_switch` returns both to `None, false`.
  - `crates/workshop-protocol/tests/it/frames.rs`: the `workbench` frame serializes and accepts `switch_in_flight`.
  - `crates/workshop-server/tests/it/session/menu.rs` (330 lines) and `session/menu/restart.rs` (434 lines): the `workbench` frames already observed carry `switch_in_flight: true` during the restart ladder and `false` at `Completed`. Keep both under 500 lines; split a sibling module if an assertion set would push one over.
- Verification: `cargo nextest run --locked -p workshop-protocol -p workshop-menu -p workshop-sessions -p workshop-server`; `cargo test --doc -p workshop -p workshop-server`; `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; `cargo test -p build-xtask` (500-line ceiling and tier graph).
- Commit: one commit with the two source edits and the three test sets.

</step-3>

<step-4>

### Step 4: Model menu idleness from `switchInFlight` (DEBT-PGL-003, UI) and regression gate [completed]

- Component: workshop
- Piece: Workshop UI consumers of the frame field
- Depends on: Step 3 (the field exists on the wire; `workbench-frames.mjs` mirrors it)
- Changes:
  - `crates/workshop-server/ui/src/services/protocol.ts` `WorkbenchFrame` (48-56): add `switch_in_flight: boolean`; delete the gap comment at 43-44.
  - `crates/workshop-server/ui/src/services/workbench-service.ts` `WorkbenchSnapshot` and `applySnapshot` (16-22, 60-64): carry `switchInFlight: frame.switch_in_flight`.
  - The `ProfileMenuService` adapter (outside the three named files; it maps `switching: null` to `""`): locate it first with `rg -n "switching" crates/workshop-server/ui/src` excluding `protocol.ts`, `workbench-service.ts`, and `window-menu.ts`; forward `switchInFlight` derived from the frame field alone.
  - `crates/workshop-server/ui/src/ui/menu/window-menu.ts`: `ProfileMenuService` (103-113) gains `switchInFlight: boolean` and loses the exception note at 109-111; `isIdle` (153) becomes `!profileService?.switchInFlight`; each radio row keeps `enabled: isIdle` (167); the "No profile" row's `isPending` (210) becomes `!isIdle() && !profileService.switching`; named rows keep `!isIdle() && profile === profileService.switching` (221).
- Tests:
  - `crates/workshop-server/ui/test/workbench-frames.mjs`: the frame helper carries `switch_in_flight`.
  - `crates/workshop-server/ui/test/window-menu.mjs`: with `switching` empty and `switchInFlight` true, every model and profile radio is disabled and the "No profile" row is pending; with `switchInFlight` false, all rows are enabled and none is pending.
- Verification: `npm run build && npm run typecheck && npm test` in `crates/workshop-server/ui`; `rg "Known gap" crates` returns nothing.
- Commit: one commit with the four UI source edits and the two test files.
- Gate (runs after this commit, closes the plan): the full regression list from the Testing Plan - `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`; `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; `cargo nextest run --locked -p workshop -p workshop-server`; `cargo test --doc -p workshop -p workshop-server`; `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings`; `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`; `cargo fmt --all --check`; `cargo test -p build-xtask`; `npm run build && npm run typecheck && npm test` in `crates/gateway-config-ui/ui` and `crates/workshop-server/ui`; `mdbook build guide`; `cargo run -p build-user-guide && git diff --exit-code -- guide/`; then the three exit sweeps `rg 'insert\("active_profile"' crates/gateway/src/lib.rs`, `rg 'remove\("active_profile"\)' crates/gateway/src/lib.rs`, `rg "Known gap" crates`, each empty. Any gate failure is fixed in a follow-up commit inside the component that owns the failing file. Pushing `master` is the operator's call and not part of this plan.

</step-4>

</execution-plan>
