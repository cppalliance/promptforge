---
name: Gateway route decentralization
overview: "Restructure crates/gateway/app so each route area owns its own tier-named routes() constructor merged at the root, with the loopback wall applied once at the merge site, and delete the two hand-copied glue lines (extractor rejection mapping, blocking-task join mapping). Runs concurrently with 2026-09-20-2-gateway-api-types-progress; Steps 5 and 6 are gated on that plan's Step 5 commit. Idioms and order come from the 2026-09-20 field study of five axum servers (Windmill, Svix, mistral.rs, SGLang, llm-gateway-rs; 44 of 49 citations verified)."
todos:
  - id: blocking-helper
    content: Add blocking() helper in error.rs; replace the 11 JoinError map_err sites (Finding 3)
    status: completed
  - id: wire-extractors
    content: Add WireQuery/WirePath beside WireJson; delete the 7 deferred-rejection sites (Finding 2)
    status: completed
  - id: area-routes
    content: Add routes() to every area module; move web_search/health/config_ui_redirect out of lib.rs
    status: completed
  - id: walled-dir
    content: Move 13 walled modules under admin/walled/, existing admin/* to admin/open/; merge walled once behind route_layer
    status: completed
  - id: loopback-caller
    content: Add LoopbackCaller extractor; switch admin/walled handler signatures (Finding 4)
    status: completed
  - id: shrink-build-router
    content: Reduce build_router to merges; drop too_many_lines waiver; verify mounts identical
    status: completed
  - id: route-registry
    content: Per-area RouteInfo consts with tier kind plus registry test; shrink lib.rs doc paragraph (Finding 6)
    status: completed
  - id: typed-responses
    content: Serialize structs for admin replies, byte-identical JSON (Finding 5); status.rs deferred to the gated step
    status: completed
  - id: split-config-apply
    content: "GATED on progress plan Step 5: move apply_config command body beside commands; leave thin handlers (Finding 7); type the admin/open/status.rs reply; blocking() in the two commands.rs sites"
    status: completed
isProject: false
---

# Gateway Route Decentralization

Source: the field study "PromptForge Gateway: How to Structure an axum Server" (2026-09-20, five axum references dived at pinned commits). Three of five references (Windmill, Svix, llm-gateway-rs) decentralize route registration and name the tier in the constructor or path; the subject's `AuthedCaller`, single `GatewayError`, loopback wall, and lint set already beat every reference and are do-not-touch. This continues `2026-09-17-3-gateway-app-decomp`, which split handlers out of `lib.rs` but deliberately kept the route table whole; the study found that every reference at this scale moves the mounts out too.

## Target shape

```mermaid
flowchart TD
    BR[build_router] -->|merge| Open[open tier]
    BR -->|"merge + route_layer(loopback)"| Walled[walled tier]
    BR -->|merge + stt auth| STT[stt routes]
    BR -->|nest_service| SPA[config-ui]
    Open --> Relay[relay::routes]
    Open --> Speech[speech::routes]
    Open --> Models[models::routes]
    Open --> AdminOpen[admin::open::routes]
    Walled --> AdminWalled[admin::walled::routes]
```

## Branch and concurrency

- Base: `vibe2` HEAD at plan time (`5be00f3c`), fast-forwarded onto `vibe3`. The progress plan's Steps 1-3 are already in; the only gateway-app difference from the profiled tree is the `gateway_api_types` rename in `cloud_models.rs`.
- Concurrent with: `2026-09-20-2-gateway-api-types-progress` Steps 4-5 on `vibe2`, which rewrite `admin/progress.rs`, `admin/progress-tests.rs`, `admin/status.rs`, edit progress sites in `commands.rs` (24), `boot_load.rs` (30), `runner.rs` (12), `cache.rs` (8), `config_apply.rs` (7), delete `render.rs`, and rename `shared_progress` to `gateway_progress`.
- Steps 1-4 here run now. Expected rebase cost against their Steps 4-5: one hunk each in `lib.rs` (do not re-add `mod render`), `admin/status.rs` (add their `progress` field to the typed struct), `admin/progress.rs` and `admin/progress-tests.rs` (modify/delete: take their content at the new `admin/open/` path), plus a `rg` sweep for `shared_progress`.
- Steps 5 and 6 are gated on the progress plan's Step 5 commit (`Move progress into the gateway family as gateway-progress`). Step 5 here moves the `apply_config` body that their Step 4 edits in 7 places; concurrent execution would force a manual re-application of those edits inside a moved function. Do not start Step 5 or 6 until that commit is on `vibe2` and this branch is rebased onto it.
- Their Steps 6-7 (harness marker, webfetch split, tidy rule) touch nothing in the gateway app and may land in any order relative to this plan.

## Repo conventions that bind this plan

- Unit tests live in a sibling `<parent>-tests.rs` wired with `#[cfg(test)] #[path = "<parent>-tests.rs"] mod tests;`, never inline. The Step 3 registry test is a `-tests.rs` sibling of wherever the registry assembly lives.
- A `src/` subdirectory needs three or more files; `admin/walled/` (13) and `admin/open/` (5) qualify. A one- or two-file split uses `parent-label.rs` with `#[path]`.
- Integration tests are the single `tests/it/main.rs` target with one module per concern, invoked as `--test it`.
- Focused tests per step: `cargo nextest run --locked -p gateway <filter>`; `cargo check -p gateway --no-default-features` after any `#[cfg]` move. The full gate set (the progress plan's exit criteria) runs once, after Step 4 or Step 6, whichever is last to land.
- The 500-line ceiling does not bind this crate (no `## Invariants` marker); no new enforcement is added.

## Step 1: Delete copied glue (Findings 3 and 2)

Pure mechanical, no route moves, verified by the existing suite.

- Add `pub(crate) async fn blocking<T>(work) -> Result<T, GatewayError>` in `error.rs` wrapping `spawn_blocking` and mapping the join once to a new `GatewayError::BlockingTask` (500, `blocking_task_failed`) (Windmill's `blocking` helper; Svix re-raises the panic). Replace the join-mapping sites in `config_write.rs`, `env_file.rs` (2), `config_apply.rs` (4), `config_pending.rs` (2), `chat_templates.rs`, `admin/profiles.rs`, `reveal.rs`, `system.rs`, `orphans.rs`, `model_info.rs`, `cache.rs` (3), `cloud_models.rs` (the cache write; the launch-time read already handles its join with a warn). `SystemMetrics` and `system_metrics()` had no producer left and are removed. Every replaced mapping was already a 500 except the `cloud_models.rs` write (502 `cloud_models_unavailable`), which a panicked blocking task never was.
- Deferred to Step 5: the two `commands.rs` sites (`provision_model`, `unload_model`) handle the join inside a three-arm match beside `leaf.complete()` / `leaf.fail()` lines the progress plan's Step 4 rewrites; touching them now buys a merge conflict for two lines.
- Add `WireQuery<T>` and `WirePath<T>` beside `WireJson` in `error.rs` (Svix `ValidatedQuery` shape, `Rejection = GatewayError`). Delete the seven deferred `Result<Extractor, Rejection>` params and their copied `map_err` line in `config_write.rs`, `env_file.rs` (2), `reveal.rs`, `model_info.rs`, `hf.rs` (2). The three `Json` sites switch to `WireJson` as-is. Fallible extractors are listed after `AuthedCaller` so an unauthenticated caller earns 401 before its malformed input earns 400; `model_info.rs` and `hf.rs` had the deferred param before the caller and are reordered.
- Verified: clippy `-D warnings`, fmt, headless check, rustdoc `-D warnings`, nextest 469 passed.
- Commit: `Centralize blocking-task and extractor-rejection mapping`

## Step 2: Per-area routes() and the walled directory (Findings 1 and 4, merged)

- Each area module gains `pub(crate) fn routes() -> Router<AppState>` returning exactly its current mounts. Move `web_search`, `health`, `config_ui_redirect` out of `lib.rs` into their modules.
- Move the 13 flat walled modules (`system`, `hf`, `reveal`, `cloud_models`, `env_file`, `config_write`, `config_pending`, `config_apply`, `model_info`, `orphans`, `chat_templates`, `shutdown`, `handoff`) under `admin/walled/`; the existing `admin/{config,profiles,progress,queue,status}` become `admin/open/`. `admin::walled::routes()` merges its children; `build_router` applies `route_layer(require_loopback)` to that one merge.
- Feature-gated areas: keep `#[cfg]` on the `mod` and on the merge line (or return `Router::new()` in non-feature builds, per Windmill), whichever keeps the URL space identical.
- Verify mounts identical, then add a `LoopbackCaller` extractor (Svix per-tier extractor) wrapping `AuthedCaller` plus the loopback check, and switch every `admin/walled/` handler signature to it. The `route_layer` wall stays as defense in depth.
- `build_router` becomes a dozen merges plus the STT merge, SPA nest, and host wall; drop the `too_many_lines` waiver.
- As executed: `admin/config.rs` (the 21-line `GET /admin/config`) folded into `config_write.rs`, which became `admin/walled/config.rs`, so the `/admin/config` path has one owning module; `config_write_error` and `error_chain` now live there. The catalog wire types (`CatalogModelsResponse`, `CatalogModelInfo`, `SpeechCatalogModelInfo`) and their one test moved from `model_info.rs` to `models.rs` and a new `models-tests.rs`, so `model_info.rs` is the pure walled GGUF route and can sit behind `#[cfg(feature = "local")]` at the `mod` line. `health` and `web_search` became one-file area modules at the crate root; `config_ui_redirect` joined `handoff.rs` as the second route of the browser entry onto the SPA. `handoff`'s two routes take no caller (the handoff mints the credential). `LoopbackCaller` has `Rejection = Response`: the wall's bare 403 for a non-loopback or peerless caller, checked before auth, then `GatewayError`'s envelope for auth; its five tests live in `auth-loopback-tests.rs`.
- Verified: all 35 route paths present before and after; clippy `-D warnings`, fmt, headless check, rustdoc `-D warnings`, `cargo test -p build-xtask`, nextest 474 passed (469 plus the 5 new extractor tests).
- Commit: `Install routes per area behind tier-named constructors`

## Step 3: Route registry with a tier kind (Finding 6)

- Each area declares `const` route infos with a tier kind (mistral.rs `RouteInfo`) and binds them in `routes()`. One test asserts every registered path sits under the kind its module directory claims.
- Shrink the 80-line doc paragraph at the top of `lib.rs` to the tier model and a pointer at the registry.
- As executed: `registry.rs` holds `Tier { Open, Walled }`, `RouteInfo { path, methods, tier }` with `const fn open` / `walled`, and `all()`, which concatenates every area's `pub(crate) const ROUTES: &[RouteInfo]` under the same feature gates `build_router` merges under; `admin::open::registry()` and `admin::walled::registry()` do the same for their children. Every `routes()` binds `INFO.path`, never a literal. `build_router` logs the registry at debug level ("route mounted", path, methods, tier) so the registry has a production consumer. `registry-tests.rs` sweeps it against the assembled router: every walled route refuses a LAN peer and a peerless caller with 403 and admits a loopback peer (not 403, 404, or 405); every open route answers a LAN peer with something other than 403, 404, or 405; paths are unique and both tiers non-empty. Captures are filled with values that fail the handler's own validation, and the fixture's `HfProxy` points at a dead port, so the sweep never reaches the network. The hand-listed `walled_requests()` and bearer-only sweeps in `loopback-tests.rs` (which had drifted: `/shutdown` was missing) are replaced by these; its tempdir fixture moved to `test_support::walled_fixture`, and it keeps the SPA mount, feature-state, and host-wall tests the registry cannot enumerate.
- Verified: clippy `-D warnings`, fmt, headless check, rustdoc `-D warnings`, nextest 475 passed.
- Commit: `Declare routes as typed registry entries with a tier kind`

## Step 4: Typed admin responses (Finding 5)

- One `Serialize` struct per admin reply in the module that produces it, replacing `json!` literals in `admin/open/status.rs`, `admin/open/profiles.rs`, `admin/walled/config_apply.rs`, `admin/walled/config_pending.rs`, `admin/walled/env_file.rs`. Wire JSON stays byte-identical; the integration tests become shape tests against a type. When rebased onto the progress plan's Step 4, `StatusResponse` carries `progress: gateway_api_types::Progress` and no `active.fraction`.
- As executed: `ProfilesReply`, `SwitchProfileReply` (profiles), `ShadowReply` (config, shared by `PUT /admin/config` and `PUT /admin/env`), `ApplyReply`, `RevertReply` (config_apply), `PendingReply`, `DirtyReply` (config_pending), `EnvReply`, `EnvSection` (env_file), plus `CancelReply` (queue) and `OrphansReply` (orphans) for uniformity. `PendingReply.profile` stays a `serde_json::Value`: it is the config document itself with `active_profile` inserted, dynamic by nature. `dirty_reply`'s unit test now asserts on struct fields. `admin/open/status.rs` is deferred to Step 5: the progress plan's Step 4 rewrites its reply body (adds `progress`, drops `active.fraction`), and typing it now would put the two changes in the same lines.
- Verified: clippy `-D warnings`, fmt, headless check, rustdoc `-D warnings`, nextest 475 passed.
- Commit: `Type the admin reply shapes`

## Step 5: Split config_apply.rs (Finding 7) - GATED

- Gate: the progress plan's Step 5 commit is on `vibe2` and this branch is rebased onto it.
- Move `apply_config` and its snapshot types beside the other commands (`commands.rs` is already 1,400+ lines, so a `commands-apply.rs` sibling with `#[path]`); `admin/walled/config_apply.rs` keeps two thin handlers. Keep the apply mutex acquisition in the same relative position. This is the one step that can shift behavior.
- Also in this step, the two pieces deferred from Steps 1 and 4 because their lines are the progress plan's: the two `commands.rs` `spawn_blocking` sites (`provision_model`, `unload_model`) move to `blocking()`, and `admin/open/status.rs` gets a typed `StatusReply` carrying their `progress: gateway_api_types::Progress` field and no `active.fraction`.
- Gate opened during Step 4: the progress plan's Steps 4 and 5 (`fb44946b`, `52f3e824`) landed on `vibe2`, and this branch was rebased onto them. The rebase cost was as predicted: five conflict hunks, all imports (`lib.rs` two, `admin/open/progress.rs` two, `cache.rs`, `commands.rs`, `config_apply.rs`), plus one rustfmt fixup squashed into the route-move commit. Their Step 4 retired some progress tests, so the suite count is 469 from here.
- As executed: `commands-apply.rs` (module `commands::apply`, wired with `#[path]`) holds `ShadowCapture`, `ApplySnapshot`, `ApplyPlan`, `RESTART_SECTIONS`, `capture_apply`, `promote_captures`, `apply_config`, and the `apply_snapshot` commit; `admin/walled/config_apply.rs` keeps the two handlers, their replies, and `delete_all_shadows` (the revert body runs inline under the route's lock, not as a command). The apply mutex acquisition is unchanged on both sides. The `capture_apply_flags_restart_for_boot_read_sections_only` test stays in the route module's test block, which owns the fixtures it shares. `StatusReply`, `QueueReply`, `ActiveCommandReply`, `PendingCommandReply` in `status.rs`; `EndpointStatus` in `models.rs` derives `Serialize` so the endpoint entries need no mirror struct. The two `commands.rs` sites now go through `blocking()`, which flattens their three-arm matches to two.
- Verified: clippy `-D warnings`, fmt, headless check, rustdoc `-D warnings`, nextest 469 passed.
- Commit: `Move the apply command body beside the other commands`

## Step 6 (optional): AppState builder (Finding 8) - GATED

- Same gate as Step 5; their Step 4 edits 12 progress sites in `runner.rs`.
- Only if the test harness churn from Steps 2 and 4 multiplies `from_parts` calls. Confidence low; `runner.rs` is mostly the serve loop, not assembly.
- Decision: not done. `AppState::from_parts` still has exactly two callers (`runner.rs` and `test_support::state_over`); Steps 2 and 4 added none. The builder would be machinery with no second consumer.

## Outcome

- Five commits on `vibe3`, rebased onto the finished `vibe2` at `49d645ec`: `4e59eaea` (Step 1), `5d899e80` (Step 2), `bc7d6549` (Step 3), `fd9e8601` (Step 4), `ed471fe1` (Step 5). Step 6 not done, by the criterion above.
- `build_router` is 50 lines of merges and walls, down from 120 under a `too_many_lines` waiver. `lib.rs` is 528 lines, down from 668. Every route is mounted by its own module behind `routes()`, declared in the registry, and swept by the registry tests against the wall its tier promises.
- Full gate set on the combined tree: `cargo fmt --all --check`; workspace clippy `-D warnings` (excluding the workshop crates); workspace nextest `--all-features` 3,761 passed; workspace doctests passed; workspace rustdoc `-D warnings`; `cargo check -p gateway --no-default-features`; `cargo test -p build-xtask` 101 passed, including the mandatory-marker rule the progress plan's Step 7 added. The workshop crates, `npm test`, and `mdbook build guide` were not run: no file outside `crates/gateway/app` and this plan changed.

### Second rebase

`vibe2` was rewritten onto a newer `master` carrying the rust-rulebook sweep, so the final rebase crossed six sweep commits that touch `crates/gateway/app` and never appeared in the first one. Three conflicts, all mechanical:

- `error.rs`: the sweep reworded `system_metrics`'s doc while Step 1 deleted the function (its only producer became `blocking()`). Deletion wins; no reference survives.
- `cloud_models/tests/`: the sweep retired `mod.rs` (`tests/mod.rs` to `tests.rs`, children to `tests-refresh.rs` and `tests-version-gate.rs` with `#[path]`) while Step 2 moved the directory under `admin/walled/`. Resolved to the sweep's flat kebab layout at the new location; the `tests/` copies were dropped, since their inherited `#[path]` would have misresolved one level down.
- `model_info.rs`: the sweep's feature-gated import block against Step 2's rewritten one. Step 2's wins.

One silent break the conflicts did not surface: the sweep added a `crate::config_write::error_chain` call in `dialect.rs`, a module Step 2 had renamed to `admin::walled::config`. Caught by `cargo check`, repointed. It leaves the tool-call parser reaching into the walled admin module for a string formatter - see the note below.

### Follow-up, done after the rebase

Two tidy-ups the plan did not schedule, each its own commit after the gate above:

- `error_chain` moved from `admin/walled/config.rs` to `error.rs`. It renders an error's `source()` chain as the one line a wire message carries, and two of its five callers sat outside the walled tier (`dialect.rs` on the relay path, and a boot test). Three module docs that still named `tokio::task::spawn_blocking` were repointed at `crate::error::blocking`, which is what they call now.
- Every route module's tests moved to the kebab sibling convention. The crate states that convention and `admin/open/` followed it, but `admin/walled/` followed it in zero of twelve modules and `cache.rs` did not either - an accident of history, since the 2026-09-17 decomposition converted only the modules it extracted from the crate root, and those were the open-tier ones. Moving this plan's files into one directory made the split visible as a line down the middle of `admin/`. Pure code motion, pinned by the test count: 470 before, 470 after.

### Still open

- The non-route machinery keeps inline test blocks: `runner.rs`, `commands.rs`, `dialect.rs`, `boot.rs`, `error.rs`, `routing.rs`, `main.rs`, `api_error.rs`, `diagnostics.rs`, `relaunch.rs`, `test_support.rs`, `tray/logic.rs`. Out of scope here and unbound by any ceiling; they are what keeps those files large.
- `auth.rs` is the one file carrying both conventions at once: two `#[path]` siblings and two inline blocks. It is not a route module, so this pass left it alone, but it is the most obvious next candidate.
- `config_apply-tests.rs` is 701 lines, the largest test sibling in the crate. Splitting it by concern (capture, commit, cancellation, revert) would be the natural follow-on.

## Constraints

- Steps 1, 2, 3 are pure structure: status and error envelope for every route, including malformed query/path, must be byte-identical. Step 4 changes the serializer, not the JSON.
- Invariant: `tests/it/` plus the in-process loopback and auth tests. They move last and only to follow a renamed module path, never to change an assertion.
- Do not touch: `dialect.rs`, `routing.rs`, `tray/`, `shared-loopback`, the STT and config-ui crate boundaries, `GatewayError::classify()`.
- Per step: `cargo clippy -p gateway --all-targets --all-features -- -D warnings` clean, `cargo nextest run --locked -p gateway --all-features` green, `cargo check -p gateway --no-default-features`, `cargo fmt --all --check`, then one commit carrying the code, its tests, and this file's todo statuses.
- Stop condition: two consecutive failures on one step stop the run for a re-plan.
