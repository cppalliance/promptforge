---
name: Fix Config UI and gateway bugs
overview: Five post-run bug fixes in one commit - HF proxy route consistency plus README proxy, chip-input blur commit, secret field visibility toggle, and whisper stderr collision with indicatif.
todos:
  - id: routes
    content: Refactor model route from {*repo} to {owner}/{name} and add readme sibling
    status: completed
  - id: client
    content: Add hfReadme to gateway-api.ts and hf-api.ts, remove fetchFn from Discover
    status: completed
  - id: chip-blur
    content: Add blur handler and flush() to chip-input.ts
    status: completed
  - id: eye-toggle
    content: Add Eye/EyeOff toggle to settings-view secretControl
    status: completed
  - id: whisper-hooks
    content: Enable whisper-rs tracing_backend, install logging hooks, filter gateway subscriber
    status: completed
  - id: tests
    content: Add Rust and UI tests for all five fixes
    status: completed
  - id: verify
    content: Run focused verification plus manual TTY acceptance
    status: completed
isProject: false
---

# Fix HF proxy routes, README rendering, and Config UI bugs

## Current state

Three routes in `hf.rs`, mounted in `lib.rs:350-351`:
- `GET /admin/hf/search` - hub model search
- `GET /admin/hf/model/{*repo}` - hub model detail (wildcard catch-all)
- No README proxy: `discover-view.ts:272` fetches `https://huggingface.co/{repo}/raw/main/README.md` directly, which is CORS-blocked in both panel mode and standalone mode, so every model card shows "No README available"

## Route changes (`hf.rs` and `lib.rs`)

Replace the wildcard `{*repo}` with two named segments and add a readme sibling:

```
GET /admin/hf/search                       (unchanged)
GET /admin/hf/model/{owner}/{name}         (was {*repo})
GET /admin/hf/model/{owner}/{name}/readme  (new)
```

**Model route refactor:**
- Change the mount from `.route("/admin/hf/model/{*repo}", ...)` to `.route("/admin/hf/model/{owner}/{name}", ...)`
- The handler receives `Path<(String, String)>` instead of `Path<String>`, joins them with `/`, and calls the existing `validate_repo` which already checks two segments
- Existing behavior, auth, error handling, and all tests stay identical

**README route (`admin_hf_readme`):**
- `GET /admin/hf/model/{owner}/{name}/readme` - bearer-authed, same `HfProxy::forward` to `/{owner}/{name}/raw/main/README.md`
- Returns `text/markdown; charset=utf-8` on success, 404 when the hub returns 404 (no README), upstream error envelope for other failures
- Same `validate_repo` + `HF_TOKEN` forwarding as the model route
- Cap response body at 1 MiB (READMEs can be huge with embedded base64 images)

**Workshop proxy allowlist** (`gateway_config.rs:68`):
- The existing `path.starts_with("/admin/hf/")` already allows all HF paths, so the new readme route passes without a change

## Client changes

**`gateway-api.ts`** - add `hfReadme(repo: string, signal?: AbortSignal): Promise<string | null>`:
- Hits `GET /admin/hf/model/{encoded}/readme`
- Returns the text body on 200, `null` on 404 (upstream not-found), throws on other errors

**`hf-api.ts`** - add `readme(repo: string, signal?: AbortSignal): Promise<string | null>`:
- Delegates to `this.api.hfReadme(repo, signal)`

**`discover-view.ts`** (`showDetail`, ~line 260-288):
- Replace `fetchFn("https://huggingface.co/...")` with `hf.readme(model.repo, controller.signal)`
- On non-null, render through `renderMarkdown` and `setSanitizedHtml` as before
- On null, show "No README available" as before

**`discover-view.ts` deps** and **`main.ts`**:
- Remove `fetchFn: FetchLike` from `DiscoverViewDeps` - it was only used for the README fetch
- Remove `hubFetch` from `main.ts` panel-mode wiring (~line 141-142) and standalone mode

## Bug 2: chip-input blur commit

**`chip-input.ts`** (~line 90):
- Add a `blur` handler on the text input that calls `add(entry.value)`, same as the Enter path
- Export `flush(): void` on the `ChipInput` interface for defense-in-depth
- The existing `add` guards (empty, duplicate, not-in-options) make blur on an empty input a no-op

## Bug 3: secret field visibility toggle

**`settings-view.ts`** (`secretControl`, ~line 786-802):
- Add an Eye/EyeOff toggle button next to the password input, matching `secrets-view.ts:114`
- Add `input::-ms-reveal { display: none }` to `controls.css` to suppress the browser-native reveal

## Tests

- `hf.rs`: test `admin_hf_readme` proxies to the correct upstream path, returns `text/markdown`, returns 404 for a missing README, requires bearer auth, validates the repo, and caps the body
- `hf.rs`: existing `admin_hf_model` tests updated for `{owner}/{name}` extraction (behavior unchanged)
- UI: chip-input blur-commit and filtered-blur-rejection
- UI: secret field toggle button renders and toggles `input.type`
- UI: discover README renders through the proxy (harness returns markdown for the stub repo)

## Bug 4: whisper stderr corrupts indicatif progress bars

During STT engine startup, whisper.cpp writes init logs (`whisper_model_load:`, `ggml_cuda_init:`, etc.) directly to stderr from C. The gateway's progress renderer draws an indicatif `MultiProgress` on the same stderr (`render.rs` `tty_loop`). Foreign writes break indicatif's cursor tracking, so bars jam together.

**`crates/promptforge-transcribe/Cargo.toml`:**
- Enable `tracing_backend` on `whisper-rs`: `whisper-rs = { workspace = true, features = ["tracing_backend"] }`

**`crates/promptforge-transcribe/src/engine.rs`:**
- Call `whisper_rs::install_logging_hooks()` at the top of `SttEngine::new_with_progress`, before any whisper context loads. Idempotent, no `Once` needed.

**`crates/promptforge-gateway/src/main.rs`:**
- Replace `tracing_subscriber::fmt::init()` with an `EnvFilter`-based subscriber defaulting `whisper_rs::whisper_logging_hook` and `whisper_rs::ggml_logging_hook` to `WARN`. `RUST_LOG` overrides still work. The `promptforge-workshop` binary has no subscriber, so rerouted logs are simply dropped there.

Rejected: `MultiProgress::println`/`suspend` (cannot intercept C fd writes); fd-level stderr redirect (platform-fragile).

## Verification

- `cargo fmt --all --check`
- `cargo clippy -p promptforge-gateway -p promptforge-transcribe --all-targets --all-features -- -D warnings`
- `cargo test -p promptforge-gateway`, `cargo test -p promptforge-transcribe`
- `npm run build`, `npm run typecheck`, `npm test` in config-ui
- `cargo test -p promptforge-workshop-server gateway_config` (proxy allowlist)
- `cargo build -p promptforge-workshop` (CUDA feature unification)
- Manual TTY acceptance: run `.\target\release\promptforge-workshop.exe` and confirm clean progress bars

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: fix_hf_proxy_consistency_ce2baa41

## Origin

This plan was created mid-chat while running `chat_templates_and_injection_defense_faef0fb8`. After that run finished, the user dogfooded the result (ran `promptforge-workshop.exe`, used the rebuilt Config UI) and reported four live bugs in quick succession; a fifth (whisper stderr) came from the garbled terminal output in that same dogfooding session. Each bug report was a short user message with a screenshot or terminal reference; the agent diagnosed root causes in-chat before planning.

## Why five fixes in one plan

The whisper stderr collision was initially planned as a separate plan file (`fix_whisper_stderr_collision_f919dfd7.plan.md`). The user rejected that outright: "NO WHY THE FUCK DID YOU MAKE A NEW PLAN? JUST PUT IT IN THIS ONE!" The agent merged it and deleted the stale plan files. The "one commit" shape is user-mandated, not an agent choice.

## Bug-by-bug rationale

**Whisper stderr vs indicatif.** User report: "why is there so much weird indicatif output when I run the workshop (see terminal)", then "plan it". The agent's diagnosis: whisper.cpp/GGML C code writes init logs directly to fd 2 behind indicatif's back, invalidating the MultiProgress cursor tracking so redraws land mid-line. Chosen fix: whisper-rs `tracing_backend` feature plus `install_logging_hooks()`, routing C logs into tracing. A subtlety captured in the chat but not the plan: the workshop binary installs no tracing subscriber, so rerouted logs are silently dropped there (desired); the gateway binary's default `fmt::init()` subscriber runs at INFO and would re-corrupt the display, which is why the plan adds an EnvFilter pinning the two whisper hook module paths to WARN while keeping RUST_LOG overrides. Discarded alternatives (also in the plan): `MultiProgress::println`/`suspend` cannot intercept C fd writes; fd-level stderr redirect is platform-fragile.

**Chip-input blur commit.** User report: "I added the endpoint but when I try to associate it with my sonnet remote model entry it doesnt save it says 'no endpoint specified'". The agent first traced the save/validation pipeline expecting a payload bug, then realized from the screenshot that "anthropic" was still text in the input, never committed as a chip: "The user typed 'anthropic' into the datalist input field... but the chip was not committed because Enter was not pressed." (agent analysis, paraphrase context). Fix: commit on blur, plus an exported `flush()` as defense-in-depth. The existing add-guards make blur on an empty or invalid input a no-op, which is why blur-commit is safe.

**Secret field visibility toggle.** User report: "web search API key edit box does not show the visibility icon when focused. it shows up only sometimes." Diagnosis: the intermittent icon was the Edge/WebView2 browser-native password reveal, which is inconsistent by platform design. The Settings secret control rendered a bare password input while the Secrets view already had an explicit Eye/EyeOff toggle; the fix copies that toggle into Settings and suppresses the native one via `input::-ms-reveal { display: none }`.

**README proxy + route consistency.** User report: "Discover model cards never show a README now despite them all having it". Diagnosis: `discover-view.ts` fetched `https://huggingface.co/{repo}/raw/main/README.md` directly, CORS-blocked from the loopback origin in both panel and standalone mode, with a bare `catch {}` making the failure silent. The agent initially proposed a separate `/admin/hf/readme/{repo}` route. The user steered the design: "wouldn't /admin/hf/{repo}/readme make more sense", then "wouldn't /admin/hf/{repo} make more sense for EVERY hf path?" The user then caught the collision in their own suggestion: "wait but then you can never have a repo named 'search' or 'model'". The agent offered a `/-/` action-namespace separator (GitLab-style, collision-proof) as the clean long-term shape but recommended against it for this commit: it would touch every HF call site, the workshop proxy allowlist, and all tests. The user then directed scope: "review all the hf routes make them consistent". The final shape (named `{owner}/{name}` segments replacing the `{*repo}` wildcard, readme as a sibling) came from the agent's observation that mixing a wildcard catch-all with a more-specific sibling route is fragile and version-dependent in axum, and that `validate_repo` already enforced exactly two segments so the wildcard added nothing.

## Verification note

The plan's manual TTY acceptance step exists because the whisper fix cannot be verified by automated tests; the agent noted a release build was required and the user already had one running.
