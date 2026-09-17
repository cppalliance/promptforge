---
name: Gateway app module decomposition
overview: "Split crates/gateway/app/src/lib.rs (4,199 lines: route table, handlers, inline tests) into feature modules with kebab test siblings, finishing the migration the crate already started with cache.rs and hf.rs. Pure code motion, no behavior change. The gateway family reorganization has landed; this runs as its own single commit. A final step applies six function-shape idioms from the 2026-09-17 field study as a separate follow-up commit."
todos:
  - id: extract-relay
    content: Extract relay.rs, speech.rs, models.rs with their test siblings from lib.rs; test siblings import shared fixtures from test_support.rs instead of copying them
    status: pending
  - id: extract-admin
    content: Create admin.rs + admin/ directory (profiles, status, progress, config, queue) with test siblings; test siblings import shared fixtures from test_support.rs
    status: pending
  - id: absorb-auth-boot-tray
    content: Move auth helpers into auth.rs, boot/tray/loopback test modules to kebab siblings; siblings import shared fixtures from test_support.rs
    status: pending
  - id: rewire-router
    content: Update build_router mounts to module paths; shrink lib.rs to docs, state, route table, tiny glue handlers; state the handler-placement rule in the crate docs
    status: pending
  - id: consolidate-fixtures
    content: Dedup the remaining fixture definitions crate-wide (shutdown.rs, handoff.rs, orphans.rs, commands.rs, config_apply.rs, boot_load-tests.rs); move auth.rs's parameterized state(trust_loopback) into test_support.rs as the configurable entry point behind a default state() wrapper
    status: pending
  - id: verify
    content: Run the verification battery; test counts before and after must match; one commit
    status: pending
  - id: function-shape-idioms
    content: Apply the six function-shape idioms from the field study (snapshot extractor, auth extractor, Json rejection, cancellation policy, error extensions, agent error fields) as a separate commit after the decomposition lands
    status: pending
isProject: false
---

# Gateway App Module Decomposition

## Target shape

`crates/gateway/app/src/` after the split (`src/` stays; this plan does not flatten it). `*` marks files created from lib.rs content; everything else is unchanged.

```
crates/gateway/app/src/
  lib.rs                <- crate docs, mod decls, AppState/LiveState, park,
                           build_router (route table unchanged), and the three
                           tiny glue handlers (health, config_ui_redirect,
                           web_search wrapper, ~40 lines total). 4199 -> ~650 lines
  relay.rs            * <- chat_completions, embeddings, rerank, relay_sse,
                           resolve_routed_model (the OpenAI passthrough surface)
  relay-tests.rs      *
  speech.rs           * <- audio_speech, audio_voices, relay_audio,
                           relay_speech_stream, relay_terminal, speech_fallback_mime
  speech-tests.rs     * <- speech_auth_tests + transcription_auth_tests
  models.rs           * <- list_models, endpoint_status, with_speech_endpoint
  models-tests.rs     *
  admin.rs            * <- module root; AdminConfig struct, config_path helper
  admin/profiles.rs   * <- admin_list_profiles, admin_switch_profile, SwitchProfileRequest
  admin/profiles-tests.rs * <- switch_route_tests
  admin/status.rs     * <- admin_status, instant_epoch_seconds
                           (imports EndpointStatus from models.rs)
  admin/status-tests.rs * <- status_surface_tests
  admin/progress.rs   * <- admin_progress, progress_sse_response, event_line
  admin/progress-tests.rs * <- progress_tests
  admin/config.rs     * <- admin_config
  admin/queue.rs      * <- admin_queue_cancel, admin_queue_cancel_pending
  auth.rs               <- absorbs authorize_stt_route, gateway_realtime_origin_allowed,
                           secret_eq, check_auth from lib.rs
  auth-tests.rs       * <- auth_tests
  loopback-tests.rs   * <- loopback_wall_tests (router-level, stays beside lib.rs)
  boot.rs               <- loses its inline test modules only; no other change
  boot-tests.rs       * <- provisioning_tests
  boot-speech-tests.rs  * <- boot_speech_tests (the 750-line one)
  tray/logic.rs         <- loses its inline test module only; no other change
  tray/logic-tests.rs * <- tray_status_tests
  ... every other existing module (cache.rs, hf.rs, reveal.rs, system.rs,
      cloud_models.rs, model_info.rs, config_pending.rs,
      config_write.rs, chat_templates.rs, env_file.rs, diagnostics.rs, relaunch.rs,
      render.rs, handoff.rs, routing.rs, error.rs, api_error.rs, runner.rs,
      dialect.rs, tray/*) unchanged
  ... fixture-dedup-only edits (test modules import from test_support.rs,
      no other change): shutdown.rs, orphans.rs, commands.rs,
      config_apply.rs, boot_load-tests.rs
  test_support.rs       <- absorbs the canonical state(), state_with_loopback(),
                           wait_until(), fake_chat_backend(), parking_executor()
```

## Rules being followed

- Feature modules with routes mounted by module path: the pattern `cache.rs` and `hf.rs` already established in this crate (`.route("/v1/cache", get(cache::list_cache))` in `build_router`, lib.rs:485-602). Five surveyed axum codebases (tensorzero, crates.io, svix-webhooks, openobserve, clean-axum-demo) all converge on this shape: one handler-free route table naming handlers by full module path.
- The handler-placement rule is written down, not just applied: add one paragraph to the crate docs in lib.rs stating that a route area gets a module named after it, a module earns a directory at 3+ files, and a new endpoint never edits the crate root outside the route table. Three of the five references state the rule explicitly (crates.io in docs, openobserve in a README, clean-axum-demo in its README); the gateway's stranded `admin_*` handlers are what happens without it.
- Kebab `-tests.rs` siblings wired with `#[cfg(test)] #[path = "foo-tests.rs"] mod tests;`: the repo's flat-sources convention. Test siblings are named 1:1 after the route surface module they pin (relay-tests pins relay.rs, admin/status-tests pins admin/status.rs), so a route's tests are findable by path alone - the crates.io / svix / clean-axum-demo mirroring rule in this repo's local form.
- Shared fixtures live in `test_support.rs` only: `state()`, `wait_until()`, `fake_chat_backend()`, and `parking_executor()` are currently redefined in nearly every inline test block across lib.rs, `shutdown.rs`, `handoff.rs`, `orphans.rs`, `commands.rs`, `config_apply.rs`, and `boot_load-tests.rs`. All of them import from `test_support` after this lands. `auth.rs`'s parameterized `state(trust_loopback)` moves there too as `state_with_loopback()`, with `state()` as its default wrapper.
- A directory only at 3+ files: `admin/` qualifies (5 modules); `relay`, `speech`, `models` stay flat siblings.
- `lib.rs` keeps the route table whole so the API surface remains one screen. The three tiny handlers stay beside it; making one-file modules for 5-line functions would fight the flat-sources rule.
- Size bound stays in force after the split: every reference that adopted module-per-area without a size ceiling recreated the monolith one level down (tensorzero's `endpoints/inference.rs` is 3,591 lines; openobserve has 168-190 KB handler files). If an extracted module later approaches 1,000 lines, it splits by sub-surface the way `admin/` does.

## What moves where

From [lib.rs](c:\Users\Vinnie\cursor\promptforge\crates\gateway\app\src\lib.rs) (the gateway family reorganization has landed; this is the current path). Line numbers verified 2026-09-17 against the 4,199-line file; if anything lands in lib.rs before this runs, locate items by name, not by line.

- Lines 683-849 (`resolve_routed_model`, `chat_completions`, `relay_sse`) plus `embeddings` (849) and `rerank` (879) -> `relay.rs`
- Lines 925-1208 (`audio_speech` through `audio_voices`, the relay helpers) -> `speech.rs`
- Lines 1208-1322 (`list_models`, `EndpointStatus` at 1275, `endpoint_status`, `with_speech_endpoint`) -> `models.rs`; `admin/status.rs` imports `EndpointStatus` from there
- Lines 1251-1275, 1612-1660 -> `admin/profiles.rs`; 1322-1429 -> `admin/status.rs`; 1491-1611 -> `admin/progress.rs`; 249 (struct), 1461-1491, 1668-1733 -> `admin/config.rs` and the `config_path` helper on `admin.rs`; 1429-1461 -> `admin/queue.rs`
- Lines 640-682 and 1733-1741 (`authorize_stt_route`, `gateway_realtime_origin_allowed`, `secret_eq`) -> `auth.rs`; `check_auth` (1698, already `pub(crate)`, called by every handler) also moves to `auth.rs`
- Test modules at lines 1743-4199 to their kebab siblings per the tree above; each sibling imports shared fixtures from `test_support.rs` as it is created, rather than copying then deduping
- Fixture dedup sweep (the `consolidate-fixtures` todo): `shutdown.rs:85`, `handoff.rs:220`, `orphans.rs:257`, `commands.rs:833/858/871`, `config_apply.rs:561`, `boot_load-tests.rs:17`, and `auth.rs:144` lose their local `state()`/`wait_until()`/`parking_executor()` definitions and import from `test_support.rs`
- `build_router` mounts change from bare names to module paths: `post(relay::chat_completions)`, `get(models::list_models)`, `get(admin::status::admin_status)`, etc.

## Constraints

- Pure code motion: no behavior change, no visibility change beyond `pub(crate)` adjustments the moves require, no route table changes. The diff should show deletions from lib.rs and insertions elsewhere of the same code.
- Steps are not individually compilable: handlers extracted in steps 1-2 reference auth helpers that don't land in `auth.rs` until step 3. Only the final state is verified; do not chase intermediate `cargo check` failures between steps.
- `runner.rs` (1501), `commands.rs` (1436), `dialect.rs` (1077), `config_apply.rs` (1059) stay over 500 lines; they are not route code and are out of scope for splitting. (`commands.rs` and `config_apply.rs` still get the fixture-dedup edit in their test modules - dedup only, no restructuring.) The 500-line ceiling does not bind this crate (no `## Invariants` marker); this plan opts into the spirit without adding the marker or new enforcement.
- One commit; the gateway family reorganization it waited on has landed (commit 32e4cf05). If run with the vibe coder, same strict one-commit variant; plan copy `vibe/2026-09-17-3-gateway-app-decomp.md` (or the next disambiguator).
- No structural enforcement is added, so the repository policy exception is not needed; the existing conventions are applied, not extended.

## Follow-ups (out of scope for this commit)

From the what-to-steal field study "Gateway App: How to Structure API Route Code That Grows" (2026-09-17, five axum references dived at pinned commits). These change behavior or add machinery, so they are not part of this pure-code-motion commit:

- Gate features in the handler, not the mounting: register routes in all builds and let a thin shim return 404/501 when the feature is compiled out, replacing the `#[cfg]`-gated `let router = router.route(...)` blocks (openobserve's rule; its own 1,150-line cfg-fragmented `service_routes()` shows the cost of the alternative). Behavior changes at the edges - check clients and the headless gate first.
- Make the mounted surface enumerable: build the route table as data (`Vec<(&str, MethodRouter)>` folded into the router, tensorzero-style) or adopt a generated OpenAPI spec pinned by a snapshot test (crates.io-style), so the prose doc header stops being the only full route list.
- Review-gate the route table: add lib.rs (or a future routes.rs) to CODEOWNERS so API surface changes get forced review friction (tensorzero).

## Step: function-shape idioms (separate commit, after the decomposition)

From the same 2026-09-17 field study, comparing the five references' handler signatures against this crate's. The current shape - `State(state): State<AppState>, caller: Caller, Json(request): Json<ChatRequest>) -> Result<Response, GatewayError>` - already matches the crates.io/svix baseline, so these are refinements. Each changes handler signatures or wire behavior, so none belongs in the pure-code-motion commit; they land together as one follow-up commit after `verify` passes.

1. **Snapshot extractor for live state** (tensorzero): quarantine `AppState.live` behind a lint or stated convention and hand handlers a per-request snapshot via `FromRef`, replacing the scattered `state.live.read().await` calls in `config_apply.rs`, `commands.rs`, `boot_load.rs`. Confidence: high - same AppState/LiveState swap shape as the reference, and the reference's quarantine was added by unmarked human commits.
2. **Auth in the extractor** (svix): an `AuthedCaller` extractor that enforces `check_auth` during extraction, deleting the repeated first line of ~19 handlers across `lib.rs` (post-split: the feature modules), `cloud_models.rs`, and `config_apply.rs`. Watch the 401-vs-405 ordering semantics around the admin `route_layer` (openobserve's mount-point comment). Confidence: medium.
3. **Rejection-shaped Json** (tensorzero `StructuredJson`): a custom extractor mapping axum `Json` rejections into `GatewayError::MalformedRequest` so malformed bodies keep the OpenAI error envelope instead of plain-text 400/422. Confidence: medium-high.
4. **Cancellation-safe relay policy** (tensorzero `possibly_prevent_request_cancellation`): decide deliberately whether client disconnect cancels upstream work or the relay spawns to completion; document the choice at the relay seam in `relay.rs`. Confidence: medium - a policy decision, not just code.
5. **Error-in-extensions** (tensorzero): `IntoResponse for GatewayError` inserts the error into response extensions so the tracing layer records it without depending on the error type. Confidence: medium.
6. **Agent-facing error fields** (openobserve `hint`/`suggestions`): consider envelope fields aimed at agent self-correction, since the gateway's clients are LLM agents. Wire-format decision; confirm before implementing. Confidence: low-medium.

Explicitly not stolen: openobserve's no-`State` global-singleton handlers (actix-migration shape, flagged non-idiomatic by the dive), crates.io's `Deref`-newtype state extractor (cosmetic), svix's `ModelIn`/`ModelOut` (the `dialect.rs` layer already covers conversion discipline), clean-axum-demo's `Arc<dyn Trait>` service graph (demo-scale).

Verification for this step: the same battery as below, plus a wire check that a malformed JSON body now returns the OpenAI error envelope (item 3) and that auth rejections return the same status codes as before (item 2).

## Verification

- `cargo test -p gateway` (every moved test module runs under its new path; counts before and after must match)
- `cargo check -p gateway --no-default-features` (the pre-push hook's headless gate)
- `cargo nextest run --locked -p gateway --all-features`
- `cargo clippy -p gateway --all-targets --all-features -- -D warnings`, `cargo fmt --all --check`, `cargo doc -p gateway` with `RUSTDOCFLAGS="-D warnings"`
- `cargo test -p build-xtask` (nothing structural changed; must stay green)
- Route-table invariant: diff the `.route(` lines of `build_router` before and after (paths, methods, order) - identical except handler module paths
- Fixture dedup check: `state()`, `state_with_loopback()`, `wait_until()`, `fake_chat_backend()`, and `parking_executor()` each have exactly one definition in the crate (in `test_support.rs`); every test module only imports them