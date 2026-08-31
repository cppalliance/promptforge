---
name: Subset profiles, chat templates, injection defense
overview: Four work packages in one vibe-rulebook run, sequenced after the Workshop session decomposition plan. (1) Replace the multi-file profile/include system with a single gateway.toml holding one global model catalog plus [[profile]] checklist entries, with the Config UI rebuilt as Local / Remote / Profiles tabs. (2) Make speech-to-text models first-class catalog entries ([[stt_model]]) governed by profiles, moving transcription engine ownership and the /voice endpoint from workshop-server to the gateway, with recommended digest pins and a restore button. (3) Port unsloth's chat-template catalog with conservative auto-selection for local llama-server launches, surfaced in the new Local Models tab. (4) Port unsloth's control-markup neutralization inventory into the untrusted-content envelope. Hard break on the old profile layout.
todos:
  - id: ui-bugs
    content: "Step 0: config-ui bug fixes - README rendering, textarea radius, globe icon, native select, capabilities badges, interaction bugs (commit 0)"
    status: completed
  - id: extract-data
    content: "Step 1: research subagent extracts unsloth template/mapper/override/delimiter data to cabinet/_research (no commit)"
    status: completed
  - id: config-schema
    content: "Step 2: config crate - [[profile]] + [[stt_model]] schema, single-file loader, hard-break errors, per-profile validation (commit 1)"
    status: completed
  - id: gateway-switch
    content: "Step 3: gateway startup + switch simplification - no profiles dir, switch from loaded catalog (commit 2)"
    status: completed
  - id: voice-move
    content: "Step 4: transcription ownership moves to the gateway - STT engine lifecycle, /voice route, ArtifactStore provisioning, workshop-server slimmed (commit 3)"
    status: completed
  - id: ui-tabs
    content: "Step 5: Config UI - Local/Remote tabs, dual-list Available/Chosen Profiles view, VRAM readout, Discover type filters, download-on-apply (commit 4)"
    status: completed
  - id: catalog-module
    content: "Step 6: chat_templates catalog module with bundled assets, family aliases, model mapper, known-overrides (commit 5)"
    status: completed
  - id: auto-select
    content: "Step 7: launch-time auto-selection with builtin:<family> resolution and artifact staging (commit 6)"
    status: completed
  - id: ui-templates
    content: "Step 8: Config UI - template dropdown in Local Models tab, Discover pre-fill, catalog endpoint (commit 7)"
    status: completed
  - id: neutralization
    content: "Step 9: control-markup neutralization in untrusted.rs with delimiter inventory (commit 8)"
    status: completed
  - id: docs-verify
    content: "Step 10: docs rewrite + full workspace verify (commit 9)"
    status: completed
isProject: false
---

# Subset Profiles, STT Models, Chat Templates, and Injection Defense

## Run state

This plan runs **after** `chat_ws_decomposition_521dd939.plan.md` completes (user decision, 2026-08-29). Preconditions before Step 0: decomposition fully merged, `git status --short` clean, record starting HEAD in `cabinet/_scratch/vibe-chat-templates/vibe-ledger.md`. If the tree is dirty, stop.

Decisions locked with the user: sequencing after decomposition (2026-08-29); scope limited to the template catalog and injection defense packages from the unsloth inventory (2026-08-29); the Config UI is the only operator surface - operators never hand-edit TOML (2026-08-30); profiles become **pure checklists** with no per-profile field overrides, and the old layout is a **hard break** with no compat loader or migration command (2026-08-30); the profiles redesign is part of this plan, not a separate run (2026-08-30); profile editing is a dual-list Available/Chosen shuttle (2026-08-30); speech-to-text models become **first-class catalog entries governed by profile membership**, with transcription engine ownership and the `/voice` endpoint moving from workshop-server to the gateway, and the gateway shipping recommended digest pins with a restore button (2026-08-30); the capability layer is named **STT** (`[[stt_model]]`, `SttEngine`), matching industry convention and unsloth's post-migration vocabulary - "voice" survives only as the `/voice` route path and UI labels (2026-08-30); the STT runtime gets its own crate `promptforge-stt` (mirroring gateway-local vs llama.cpp), the gateway gains an OpenAI-compatible `POST /v1/audio/transcriptions` alongside the streaming `/voice` socket, and the pairing rule is interim-only allowed (degraded) while final-without-interim is a validation error (2026-08-30); the active profile lives in a sibling state file (persisted on apply, not on selection), not the config (2026-08-30); profile switching is deferred to Apply like every other config change (2026-08-30); the hard break stands with no migrate command - the format shipped the same day and has no users to migrate (2026-08-30); **download-on-apply**: Discover "Download" stages the model entry as a pending config edit, the actual download happens during Apply through the gateway's normal provisioning path - eliminates orphan-on-revert, invisible-until-refresh, and progress-bar-stuck bugs (2026-08-30).

Rulebooks binding this run: [vibe-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\vibe-rulebook.md), [rust-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\rust-rulebook.md), [typescript-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\typescript-rulebook.md) for Steps 0, 5, and 8, and [html-css-rulebook.md](c:\Users\Vinnie\cursor\tools-public\rulebooks\html-css-rulebook.md) for Steps 0, 5, and 8. Governing AGENTS.md files by path: `promptforge/AGENTS.md`, `promptforge/crates/promptforge-gateway-local/AGENTS.md`, the crate AGENTS.md files for `promptforge-gateway-config` and `promptforge-gateway` where present, `promptforge/crates/promptforge-workshop-server/AGENTS.md` and `promptforge/crates/promptforge-transcribe/AGENTS.md` (Step 4 rewrites the boundary both describe), and the AGENTS.md governing `promptforge-core-support` if present (verify at dispatch). `promptforge-gateway-config-ui` has no AGENTS.md; the root file governs it.

Coders work from this plan, the governing AGENTS.md files, and the Step 1 research file. They do **not** read the unsloth repo - `promptforge/AGENTS.md` forbids referencing files outside the repo, so every implementation fact enters through Step 1's research output.

### Rust rulebook application (binding on every step)

The rust-rulebook is already listed above; these are its rules with specific teeth in this plan:

- **Reference oracle for the port** (rulebook section 12): the template catalog is a port, so correctness comes from differential testing, not compilation. Step 1 captures golden renderings; Step 6 diffs against them.
- **New crate checklist** (`promptforge-stt`, section 8): the split is justified under "isolate a heavy optional dependency" (whisper/CUDA) - state that in the commit message. Manifest: `[lints] workspace = true`, `publish = false` unless the workspace table says otherwise, dependencies via `workspace = true`, feature named for the thing itself (`cuda`, matching the existing `voice-cuda` lineage), features purely additive.
- **Naming** (section 2): acronyms are one word - `SttEngine`, `SttSlot`, not `STTEngine`. Modules are `stt.rs` plus `stt/` children; no `mod.rs` (section 7).
- **New public API shape** (sections 5-6): new error types are `thiserror` enums with `#[non_exhaustive]`, lowercase messages, no `failed to` prefix, `#[source]` on wrapped causes. New public enums (`Family`, the STT role) get `#[non_exhaustive]` at introduction. Every public item carries `///` docs with `# Errors` where fallible (already required by AGENTS.md; the rulebook adds the heading conventions).
- **Ignored live test** (section 11): the URL+pin verification test for the recommended STT pair carries a `#[ignore = "reason"]` string and runs on the scheduled/ignored path, matching the existing `live_cuda` pattern.
- **Semver** (section 6): several touched crates are published to crates.io. Removing `include`, the `models` allowlist, and the `[workshop.voice]` model keys is breaking; renaming `VoiceEngine` is breaking. Run `cargo semver-checks` on published members and bump majors per the rulebook's table - the hard break is deliberate, but the version numbers must say so. Centralize config in root `[workspace.metadata.cargo-semver-checks.lints]` with per-crate opt-in (research: workspace-level lint config is the 2026 pattern, and semver-checks is on track to merge into `cargo publish` itself)

### TypeScript rulebook application (binding on Steps 0, 5, 8)

- **Strict config**: `strict: true`, `noUncheckedIndexedAccess`, `verbatimModuleSyntax` in the config-ui tsconfig; `import type` for type-only imports; explicit return types on exported functions
- **No `any`**: external data (HF API responses, gateway config payloads) arrives as `unknown` and validates through Zod or a schema before use; the existing `gatewayStub` test harness data uses `satisfies`, not `as`
- **No `enum`**: the `Family` and STT role types use `as const` objects with derived union types, not TypeScript enums
- **No barrel files**: import directly from source files, no `index.ts` re-exports in the UI source tree
- **Async discipline**: no floating promises (`no-floating-promises`); every promise awaited or explicitly detached with `void` + `.catch()`; `AbortController` for cancellable operations (search, download)
- **Testing**: `satisfies` over `as` for test data; `vi.fn<[Args], Return>()` typed mocks; `expectTypeOf` for type-level assertions

### HTML/CSS rulebook application (binding on Steps 0, 5, 8)

- **Semantic elements**: `<button>` for actions, `<a href>` for navigation; never `<div>` with `onclick`; the dual-list shuttle uses `<ul role="listbox">` with `<li role="option">` per APG
- **Focus**: never `outline: none` without a `:focus-visible` replacement; the focus-ring clipping fix in Step 0 uses inset outline or container padding, not removal
- **Labels**: every form control has an associated `<label>`; icon-only buttons carry `aria-label`
- **Contrast**: body text 4.5:1, large text 3:1, non-text UI (borders, focus rings, icons) 3:1 - verify the dark theme meets these
- **Cascade layers**: the existing `@layer` structure stays; new component styles go in the `components` layer, not inline or with `!important`
- **Tap targets**: at least 24x24 CSS px for all interactive elements (the `.button-xs` and `.chip-remove` classes need verification)
- **Reduced motion**: wrap non-essential animation in `@media (prefers-reduced-motion: reduce)`

### Verify schedule (vibe rulebook)

Verify runs when review-and-fix dirtied the tree, on every 3rd step, at the end of each high-level component, and on the final step. For this plan that means: after Step 3 (every-3rd), after Step 5 (ends Part 1), after Step 7 (ends Part 2), after Step 9 (every-3rd; ends Part 3), and at Step 10, which runs the **full workspace suite** instead of the step's tests. A scheduled Verify gates the next step; three red rounds stop the run and report to the user.

### Execution machinery (vibe rulebook)

Each step runs the per-step loop: Coder subagent (code + focused tests), main stages, Message subagent writes the commit message from the staged diff alone, Review-and-Fix subagent against the diff (open findings of any severity block the next step), amend if review dirtied the tree, Verify per the schedule above. Every dispatch is asynchronous and carries: its role, the rulebook path, this plan's path, the step number, the XML tag blocks to apply, and the governing AGENTS.md paths from the rules manifest (root plus every nested AGENTS.md on the ancestor chain of touched files). Main keeps `cabinet/_scratch/vibe-chat-templates/vibe-ledger.md` (append-only: step, commit hash, Verify status, decisions with falsifiers) and the review subagent keeps `vibe-review.md` beside it (open findings, carried verbatim until fixed or rejected with a stated reason). Before Step 0 dispatches, main reads this plan once for defects - each step receives what earlier steps produce, no step admits two interpretations - fixes what the pass finds, then does not re-read.

### Review 2026-08-30 (HEAD ae01495, tree clean)

51 commits landed after the first draft, mostly the gateway config-ui series. Workspace grew from 26 to 29 crates: new `promptforge-gateway-config-ui`, `promptforge-gateway-loopback`, `promptforge-progress`. No crates removed.

Citation drift: `runtime.rs` `launch_options` moved 269 to 315, `chat_template_file` 279 to 325. All other citations unchanged and re-verified: `support.rs:299/332`, `untrusted.rs:55/71/85`, `tool_loop.rs:259/263`, `host.rs:99`.

Decomposition status: both steps completed, with the session split landed at `add98be`; that commit is an ancestor of reviewed HEAD `ae01495`. The sequencing precondition is met. The decomposition plan explicitly deferred splitting `voice.rs` ("its own split is a follow-up") - Step 4 of this plan is that follow-up, with the gateway as the destination.

New facilities in `promptforge-gateway-local` since the first draft:

- `src/gguf.rs` - a bounded GGUF header parser (architecture, layer count, parameter count) backing `GET /admin/model-info`. Step 7 needs the embedded `tokenizer.chat_template` metadata key for hash-keyed overrides (the override table keys on template content, per Ollama's proven pattern). Research note (`cabinet/_research/2026-08-30-plan-research-rust-config-artifacts.md`): the key lives in the header metadata block - never read tensor data; candle's `gguf_file::Content` is an option since candle is already in the workspace, but extending the existing bounded `gguf.rs` reader is preferred (gguf-rs's default container truncates arrays); the key may be absent, so always `Option`
- `src/artifacts/progress.rs` and `src/artifacts/confine.rs` - progress-tree reporting (via `promptforge-progress`) and confinement. Step 7's asset staging must reuse these and re-read `artifacts/verified.rs` at execution time rather than trust this plan's description of the marker API.
- `GET /admin/orphans` cache scan - context only, no overlap.

The neutralization surface (`promptforge-core-support`, `promptforge-core`, `promptforge-lua`) has zero commits since the first draft.

## Design constraint: the Config UI is the only operator surface

Operators never hand-edit TOML. Every operator-facing capability in this plan must be reachable from the gateway Config UI (`promptforge-gateway-config-ui`, nested at `/config`). Config keys remain the on-disk representation, but the UI writes them. Consequences:

- Profile switching, model selection, STT model selection, and template selection are all UI operations; the schema exists to be written by the UI.
- Template auto-selection (Step 7) is the primary path, not a convenience: known-bad embedded templates must be fixed with zero operator action.
- `builtin:<family>` exists so the UI has a stable, serializable value to write - it is not a hand-editing feature.

## Background: profiles today (measured 2026-08-30)

Boot `gateway.toml` plus `profiles/<name>.toml` files stitched by an `include` DAG (depth-first, max depth 16, cycle detection, whole-entry replace by `id`/`name` in `profile/merge.rs`). Profiles can carry any section - no schema gate; boot ownership of `[server]`/`[workshop]` is enforced only at runtime by equality checks (`runner.rs:672-724`). `models = [...]` already filters the merged catalog before validation and spawn (`config/validate.rs:18-51`). Live switch (`promptforge-gateway/src/lib.rs` `run_switch`) reloads the profile from disk, stops all local children, eagerly spawns every local model in the new set, and swaps routing atomically. The Models view already distinguishes local vs remote (filters at `models-view.ts:42-43,158-159`); the Profiles view has an include-chain editor built on provenance tracking.

## Background: voice today (measured 2026-08-30)

`[workshop.voice]` configures two whisper models by path (`interim_model`, `final_model`; empty interim path means disabled) plus download URLs (`interim_source`, `final_source`) and capture tuning (`window_seconds`, `interval_ms`, `vocabulary`) - see `promptforge-gateway-config/src/config/workshop.rs:194-204`. `promptforge-workshop-server` owns everything runtime: `provision.rs` downloads the models via its own `cache_fetch` (no digest pinning, no ArtifactStore, invisible to the Downloads view), `voice.rs` (1,315 lines) serves the `/voice` WebSocket (WebView streams 16 kHz mono PCM; interim + pipelined final passes), and `promptforge-transcribe` (used only by workshop-server) runs the whisper engine. The gateway and workshop-server run in one merged process, so this boundary is module-level, not a process boundary. Voice VRAM is invisible to the gateway's accounting today.

## Background: templates and injection defense (measured 2026-08-29)

**Templates.** `promptforge-gateway-local` always launches llama-server with `--jinja` (`src/server/support.rs:299`) and only adds `--chat-template-file` when the operator sets `chat_template_file` per `[[local_model]]` (`support.rs:332`, `runtime.rs:325`). There are no bundled template assets and no per-model-family tables; `dialect.rs:57-112` scores family markers for tool-dialect resolution only. Unsloth maintains exactly the missing data: ~25 families of Jinja templates with alias sets, default system messages, and stop tokens (`unsloth/chat_templates.py`), an HF-id to family mapper (`unsloth/ollama_template_mappers.py`), and a known-override policy for GGUFs with broken embedded templates (`studio/backend/core/inference/chat_templates.py`).

**Injection defense.** `promptforge-core-support/src/untrusted.rs` wraps untrusted tool/Lua content in a nonce XML envelope and escapes `<` to `&lt;` (`encode`, lines 85-87). That neutralizes angle-bracket control tokens (`<|im_start|>`) but not bracket-style delimiters (`[INST]`, `[AVAILABLE_TOOLS]`, `[TOOL_CALLS]`), fullwidth variants, or turn-channel markers that llama-server's Jinja rendering can honor. Unsloth ships a closed delimiter inventory with a space-the-opener neutralizer and role-differentiated rules (`unsloth/studio/backend/core/inference/chat_template_helpers.py`, `_CONTROL_MARKUP` ~L45-126).

## Step 0: config-ui bug fixes (commit 0)

Pre-existing config-ui bugs, combined into one commit (vibe rulebook rule 7). Follows the per-step loop (Coder, Message, Review-and-Fix).

**Visual:** textarea pill radius (`.input` on textareas: `border-radius: 0.75rem`, `height: auto`); remote icon cloud-to-globe (Lucide, `models-view.ts:1111-1123`); native `<select>` replaced with custom disclosure using `.select` trigger + `.menu` / `.menu-item` (ARIA listbox keyboard nav); focus ring clipping (inset outline or padding on scrollable panes); capability pills (images, thinking) alongside the kind badge in list rows and detail header; non-breaking space between number and unit in `formatBytes` (`format.ts:14`, use `\u00a0` instead of `" "`) so sizes like "3.0 GiB" never break across lines.

**Interaction:** adopt refreshes orphan list; `.verified` files filtered from orphans; disabled Delete carries a tooltip; Apply/Revert visible after same-session saves; restart banner dismisses on config-generation advance; context slider log-mapping investigated and fixed.

**Discover rendering:** markdown-it (`html: true`) + sanitize-last DOMPurify (Open WebUI CVE precedent) + native Sanitizer API detection + gray-matter frontmatter strip.

**Download-on-apply** (design change, replaces the current separate-download flow): the Discover "Download" button no longer triggers an immediate `POST /v1/cache` download. Instead, it **adds the model entry to the pending config** (with source URL, sha256, vram_gb pre-filled from the HF listing). The actual download happens during **Apply**, when the gateway provisions the model through its normal `ArtifactStore::ensure_model` path (which already downloads missing GGUFs before spawning, with the indicatif progress that works beautifully). This eliminates three bugs at their root: orphans-on-revert (nothing on disk until Apply commits both config and file), invisible-until-refresh (the model entry is in the pending config immediately), and progress-bar-stuck (progress is the gateway's provisioning SSE during Apply, not a separate client-side stream). The `DownloadStore` class and its SSE `cacheDownload` path in `gateway-api.ts` are removed; the config-ui's progress strip in panel mode is removed (the Workshop status bar already owns Apply progress display). The Downloads tab is **deleted**; its cache listing and the orphan/unconfigured section are absorbed into the Local Models tab (file status per model entry, unconfigured files as a compact subsection).

Tests: textarea radius, globe icon, custom dropdown keyboard nav, capability pills, adopt refresh, verified filtered, disabled tooltip, Apply visibility, restart dismissal, README rendering + XSS fixtures, download-on-apply creates a pending entry without touching disk.

## Step 1: extract unsloth data (research, no commit)

A research subagent reads the unsloth repo and writes `cabinet/_research/2026-08-29-conduct-research-unsloth-template-data.md` (YAML frontmatter per cabinet rules) containing, verbatim:

- Jinja template strings, alias sets, default system messages, and stop-token lists for these families only: `chatml`, `llama-3`, `llama-3.1` (with 3.2/3.3 aliases), `qwen-2.5`, `qwen-3`, `gemma-3`, `gemma-4`, `mistral`, `phi-3`, `phi-4`, `gpt-oss`, `zephyr`
- The HF-repo-id to family mapping subset covering those families
- The known-override policy table from Studio (which model ids get a bundled template instead of the embedded GGUF template, e.g. gemma-4 edge vs standard), including its precedence rule: user override > family default > embedded
- The full `_CONTROL_MARKUP` delimiter inventory with per-family grouping and the role-differentiated sweep rules (full neutralization on user/system/tool content; turn boundaries only on assistant replay), plus the space-the-opener technique
- **Golden render fixtures** (the reference oracle for the port): for each ported family, capture (template, context JSON, reference output) triples - a fixed message fixture (system + user + assistant + tool-call turn where the template supports tools) serialized as JSON, plus the exact string the original Python Jinja2 stack produces, with the Python/Jinja2 versions recorded. Without these, Step 6 can only prove the Rust port compiles, not that it renders identically

## Part 1: subset profiles and voice as first-class models

### Target design

One `gateway.toml`. Global sections once: `[server]`, `[workshop]`, `[local]`, `[tools]`, `[[endpoint]]`, `[[dominion]]`, `[[model]]` (remote), `[[local_model]]` (local chat), `[[stt_model]]` (speech-to-text). Profiles are checklists over the whole catalog:

```toml
[[stt_model]]
name = "whisper-interim"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.en.bin"
sha256 = "..."
vram_gb = 0.3

[[profile]]
name = "work"
models = ["gpt-5", "qwen3-local", "whisper-interim", "whisper-final"]

[[profile]]
name = "travel"
models = ["qwen3-local"]
```

The active profile is **pending state**: stored in the pending shadow alongside other config edits under the key `active_profile`, applied atomically on Apply (which triggers the switch), and persisted to the sibling state file `<config-stem>.state.toml` on successful apply. The state file has one canonical TOML key:

```toml
active_profile = "work"
```

The pending admin JSON uses the same `active_profile` key. Precedence at startup: `--profile NAME` flag > `PROMPTFORGE_PROFILE` env var > state file > refuse with an error listing the defined profiles.

Semantics: the active profile filters the catalog before validation and spawn - exactly what `models = [...]` does today. Checked local chat models spawn llama-server children (unchanged); checked remote models route (unchanged); checked STT models load into the gateway-owned transcription engine (new, Step 4). The active profile resolves per the precedence above; a state file naming a profile that no longer exists is a startup error naming the stale value.

STT enablement is profile membership: transcription is on when the active profile's Chosen list contains STT models, off otherwise. There is no separate `enabled` flag - an earlier draft had one and the first-class-model design replaces it. The `[workshop.voice]` section is renamed `[workshop.stt]` and shrinks to capture tuning (`window_seconds`, `interval_ms`, `vocabulary`); the model path/source keys move into `[[stt_model]]` entries.

Discover download-on-apply: the Discover "Download" button does not trigger an immediate download. It stages a `[[local_model]]` or `[[stt_model]]` entry (with source, sha256, vram_gb pre-filled) into the pending config. The actual download happens during Apply, when the gateway provisions missing artifacts through `ArtifactStore::ensure_model`. Nothing hits disk until Apply commits both config and file atomically. Revert discards the entry and no orphan exists because no file was downloaded.

### What gets deleted

- Include machinery in `promptforge-gateway-config/src/profile.rs`: `MAX_INCLUDE_DEPTH`, cycle detection, `resolve_include`, `take_includes`, `ChainResolution`, `ConfigError::IncludeCycle` / `IncludeDepth`
- `profile/merge.rs` wholesale (keyed-array merge exists only to stitch files)
- Profiles-dir discovery: `list_profiles`, `profiles_dir_for` (`runner.rs:663-669`), the sibling `profiles/` convention
- Boot-match checks `check_server_matches_boot` / `check_workshop_matches_boot` (`runner.rs:672-724`) - sections live in the one file, nothing to drift
- Per-profile env files: only `<boot-stem>.env` loads (today: profile env then boot env, `runner.rs:775-776`)
- Top-level `models = [...]` allowlist key in `RawConfig` - profiles carry the lists; `apply_model_allowlist` mechanics stay but are fed by the active profile
- Provenance / `source_file` tracking and the UI concepts built on it (include drill-in, inherited badges, copy-into-leaf note)
- `PUT /admin/include` and the include-chain editor in the Profiles view
- `[workshop.voice]` model keys: `interim_model`, `final_model`, `interim_source`, `final_source` (replaced by `[[stt_model]]`), and the section name itself - tuning moves to `[workshop.stt]`
- workshop-server's voice ownership: `voice.rs` moves to the gateway renamed as `stt.rs`, the voice half of `provision.rs` is replaced by ArtifactStore provisioning, the `promptforge-transcribe` dependency moves with them (Step 4)

Keep: `ProfileName` rules (now for URL/identifier safety, not filesystem), the shadow/pending write path (`gateway.toml.next`, `GET /admin/config-pending` / `config-dirty`), and the `POST /admin/switch-profile` route shape (the Workshop session menu calls it).

### Step 2: config crate - schema, loader, validation (commit 1)

- `RawConfig` gains `[[profile]]` (`name`, `models: Vec<String>`) and `[[stt_model]]` (`name`, `role` = `interim` | `final`, `source`, optional `sha256`, `vram_gb`, optional `dominion`); loses `include`, the top-level `models` allowlist, and the `[workshop.voice]` model keys. The active profile is **not** in `RawConfig` - it lives in the sibling state file (see target design)
- A built-in **recommended STT pair** table ships in the config crate: interim and final entries (name, role, source URL, SHA-256 pin, VRAM estimate) for the conventional whisper.cpp GGML models. Research-backed values (`cabinet/_research/2026-08-30-plan-research-stt.md`): interim `tiny.en` or `base.en`, final `small.en` on CPU or `large-v3-turbo-q5_0` on GPU (547 MiB, the GPU sweet spot). Canonical URLs are `huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-<model>.bin`; whisper.cpp publishes only SHA-1, so source the SHA-256 pins from the HF LFS tree API (the auricle-stt crate already hardcodes verified values for base.en/small.en - captured in the research file). An ignored live test verifies each URL+pin pair so table rot surfaces deliberately
- Loading a document with an `include` key, a sibling `profiles/` dir, or the old `[workshop.voice]` model keys fails with a hard-break error naming the new layout in one sentence. Research on breaking config migrations (`cabinet/_research/2026-08-30-plan-research-rust-config-artifacts.md`, alacritty/deno precedents): the error must name the file, key, and line - alacritty's location-less errors were its top user complaint. Add a `config-version = 2` key (rustfmt style-edition pattern) as a future migration hook, and update `gateway.local.example.toml` into a fully commented example of the new layout in this same commit - alacritty's refusal to ship one was its most-cited grievance
- Layout within the file: dotted-header tables rather than deep inline nesting (TOML readability falls off past 2-3 levels), and a canonical section ordering so semantically-irrelevant ordering does not create merge noise
- `Config::load` replaces the `load_profile*` family; profile selection happens after parse: resolve active profile, apply its list as the allowlist, validate
- Validation changes: profile names unique and `ProfileName`-legal; every name in every profile's list must exist in the catalog (STT models included); a state file naming an unknown profile is a startup error naming the stale value; **every** profile's local subset is checked against local dominion VRAM budgets at load, with STT models counting through their optional `dominion` binding under the same rules as local chat models (today only the active profile is validated - validating all profiles at load makes a live switch unable to land on an invalid profile); each profile may contain **at most one interim-role and one final-role** STT model (the engine drives exactly one of each); **interim without final is allowed** (today's documented degraded mode: nothing crystallizes mid-take, the final pass falls back to one interim decode at `stop`), while **final without interim is a validation error** naming the fix - the interim model drives the streaming take, and final-only operation would be new engine work with no identified use case
- Delete the include/merge test suites with the machinery; new tests pin the schema, hard-break errors, per-profile validation, STT role rules, and selection
- Update the crate's module docs and README surface per AGENTS.md

### Step 3: gateway startup and switch simplification (commit 2)

- `load_startup` (`runner.rs:771-825`): single file load, single env file, no profiles dir, no boot checks; `--profile` optional override
- `run_switch` (`promptforge-gateway/src/lib.rs:855+`): the catalog is already loaded and validated, so switching becomes: look up the named profile in the loaded config, apply its subset, stop current local children, spawn the new subset, atomic swap, and persist the choice to the state file. Apply triggers this when the pending `active_profile` differs from the running one. Delete `load_switch_config`, env reload, and boot re-checks. The SSE stage stream (`loading-profile` / `stopping-models` / `starting-models` / terminal) is preserved for the Workshop menu. Research on switching practice (`cabinet/_research/2026-08-30-plan-research-model-switching.md`): today's immediate no-drain cutover has documented failure modes in direct prior art (llama-swap mid-request kill races, #667) - switch to a **bounded drain** (~30 s, llama-swap/LocalAI precedent): register in-flight requests under the switch lock, wait for them, cancel stragglers, then stop children. Readiness stays defined as llama-server's own health endpoint reporting the model loaded (never process-started). On partial start failure, leave successfully started models running and report which loaded (LocalAI pattern); failed models get the existing respawn cooldown semantics
- Eager loading on switch is validated by research: LocalAI moved realtime pipelines from lazy to eager blocking warm-up because lazy loading stalled cold sessions and surfaced errors mid-stream; the plan's eager spawn stands
- `GET /admin/profiles` lists profiles from the loaded config instead of the directory; create/rename/delete profiles become ordinary pending-shadow edits to the single file
- Degraded-mode behavior on start failure after stop stays as documented (`lib.rs:834-848`)
- Tests: switch without disk reload, unknown profile refusal, `--profile` override precedence

### Step 4: transcription ownership moves to the gateway (commit 3)

The gateway becomes the owner of STT model lifecycle, making profile membership real for transcription. The engine itself (`promptforge-transcribe`) does not change behaviorally - what moves is who constructs it, who provisions its models, and who serves the endpoints. The runtime lands in a **new crate `promptforge-stt`**, mirroring `promptforge-gateway-local`'s relationship to llama.cpp: transcribe is the engine, stt is the runtime/HTTP owner. The crate boundary keeps the whisper/CUDA dependency behind the stt crate's own feature gate instead of complicating the gateway's core feature matrix. Naming follows the STT decision: the transcribe crate's public types rename (`VoiceEngine` to `SttEngine`, `VoiceSlot` to `SttSlot`) since the boundary is being rewritten anyway - half-renamed is the trap unsloth had to migrate out of.

- `promptforge-stt`: on startup and on each applied switch, provision the active profile's checked STT models through `ArtifactStore::ensure_model` (`.part` downloads, SHA-256 enforced when `sha256` is set, verified markers, confinement, progress trees) and construct the engine from the checked interim/final pair; on switch-out, tear the engine down so unchecked STT models hold no VRAM. Interim-only is supported as today's documented degraded mode (no crystallization mid-take; final pass falls back to one interim decode at `stop`)
- The `/voice` WebSocket implementation moves from `workshop-server/src/voice.rs` into `promptforge-stt`. The route stays served on the workshop loopback listener (the gateway already hosts that listener in the merged process) and keeps the `/voice` path for this run, so the WebView client and the wire contract (generations, interim/final frames, silence gating) are unchanged; a route rename is a possible later cleanup
- New OpenAI-compatible endpoint on the gateway's bearer-authed API: `POST /v1/audio/transcriptions` - multipart file in, JSON transcript out, following the gateway's OpenAI-shaped surface. Speaches (formerly faster-whisper-server) is the reference implementation; match its request/response dialect, including the literal `timestamp_granularities[]` form-field name and the `json` / `verbose_json` response formats; a 25 MB upload cap follows vLLM's precedent. The `model` parameter must name a loaded STT entry (the active profile's checked interim or final); anything else is a 404 `model_not_found`, consistent with chat. This is request/response transcription for arbitrary OpenAI-client tooling; it does not replace the streaming `/voice` contract, which no OpenAI standard covers. Note that whisper.cpp's own server is NOT OpenAI-compatible by default (it serves `/inference`) - our endpoint exceeds upstream here
- workshop-server loses `voice.rs`, the voice half of `provision.rs` (`cache_fetch` voice paths, `download_leaf` wiring), and its `promptforge-transcribe` dependency; `relay.rs`'s `VoiceSlot` usage moves with the endpoint. Rewrite the boundary paragraphs in `workshop-server/AGENTS.md` and `promptforge-transcribe/AGENTS.md` to describe the gateway-owned engine, and record the new crate in the workspace README crate table
- Headless rule: a gateway with no `[workshop]` section refuses a profile containing STT models, same as the existing headless refusal for `[[local_model]]`
- `provision.rs` already uses `promptforge-progress` at reviewed HEAD `ae01495`; re-read its current shape at execution time before moving the voice provisioning boundary
- Tests: pinned digest enforced, unpinned still works, switch-in loads and switch-out fully unloads the engine, interim-only degraded mode, headless refusal, `/voice` wire contract unchanged (move the existing voice tests mechanically - do not rewrite assertions), `/v1/audio/transcriptions` round-trip and model-not-found

### Step 5: Config UI rebuild - tabs and checklists (commit 4)

Tab bar order (user decision, 2026-08-30): **Settings, Discover, Local, Remote, Profiles, Secrets**. The Downloads/Cache tab is deleted; the Local tab absorbs its content.

- Models view (`ui/src/views/models-view.ts`): promote the existing `local | remote` filters to router tabs; each keeps its current detail pane and settings registry. The Local tab shows both local chat and STT models (both load onto hardware), with a secondary filter (`All | Chat | STT`) to narrow within the tab. Each model entry's detail pane shows its file status (downloaded / not downloaded, size on disk, cache path) and a Delete button for the cached file. Unconfigured orphan files (downloaded but no config entry) appear as a compact "Unconfigured files" subsection at the bottom of the Local tab, not a separate Cache section - they are just local models waiting to be adopted or cleaned up. STT models render with a Mic badge alongside the Cpu badge, with their own settings section (role, source, sha256, vram_gb, dominion). The Kind dropdown does not appear in the STT detail pane - kind is implicit from the entry type; the badge reads "stt" and is not editable. The filter bar's current Unconfigured/Allowlist pills are removed (the allowlist is deleted by the profiles redesign; orphans are inline in the Local tab)
- Discover view (`ui/src/views/discover-view.ts`): add a row of toggleable type filter buttons above the search results: **Chat**, **Embedding**, **Reranker**, **STT**, **Image**, **TTS**. Today the search hardcodes `filter: "gguf"` (`hf-api.ts:161`); each active type toggle adds the corresponding HF `pipeline_tag` to the search query (`text-generation`, `feature-extraction` / `sentence-similarity`, `automatic-speech-recognition`, `text-to-image`, `text-to-speech`). Multiple toggles are OR-ed by issuing one query per active pipeline tag and deduplicating by repository id; live validation on 2026-08-30 showed that Hugging Face intersects repeated `pipeline_tag` parameters and returns no Chat-plus-STT rows. The GGUF chip stays as an additional filter. Default: Chat on, rest off (matching the most common use case). When the operator adds a model whose returned `pipeline_tag` is `automatic-speech-recognition`, pre-fill the entry as `[[stt_model]]` instead of `[[local_model]]`
- Profiles view (`ui/src/views/profiles-view.ts`): delete the include-chain editor, include drill-in, and provenance badges. Each profile editor is a **dual-list shuttle**: an **Available** list (catalog models not in the profile) and a **Chosen** list (the profile's `models` array), with controls to move entries back and forth; membership in Chosen is the entire profile definition. Entries carry kind badges (Cpu vs Cloud vs Mic) so local, remote, and STT are visually distinct in both lists. Chosen preserves catalog order; no manual reordering in this step. Set Active writes `active_profile` to the pending shadow like any other config edit - the actual switch happens on Apply, same as model additions or setting changes. This keeps one atomic Apply for all pending changes including the profile switch; New Profile offers Empty / Copy of (duplicates the Chosen list). Accessibility per research (`cabinet/_research/2026-08-30-plan-research-profile-ux.md`): follow the APG rearrangeable-listbox model - each pane is `role="listbox"` with `role="option"` items, one Tab stop per pane with roving tabindex, arrow/Home/End/typeahead navigation, real `<button>` move controls that are disabled (not inert) when the move is impossible, and `aria-live="polite"` announcements of completed moves; PatternFly-style per-list counts and search when the catalog is long. Unchosen models stay visible in the management UI and list caches invalidate on every toggle - Open WebUI's vanishing-disabled-models bugs are the cautionary tale
- `config-store.ts`: drop `includeChain` and inherited/copy-into-leaf logic (`buildConfigPayload` no longer strips boot sections for leaf shadows - there are no leaves); the dirty/pending/applied model stays
- Delete `PUT /admin/include` client code and its route
- Profile VRAM estimate: each profile shows its estimated VRAM usage - the sum of `vram_gb` over the local and STT models in its Chosen list - updating live as entries move between Available and Chosen. Where chosen models bind to a local dominion with a `vram_gb` budget, show the sum against that budget (e.g. "14.5 / 24 GB") with three states driven by a named `VRAM_WARN_FRACTION = 0.8` constant in the UI (not a config key): under 80% normal, at or above 80% a warning icon (headroom for KV cache and runtime overhead), over 100% an over-budget error state - so the operator sees the violation while editing rather than as a load-time validation error after. Chosen models with no `vram_gb` estimate are surfaced as "unknown" contributors, not silently treated as zero. A tooltip breaks the estimate into weights vs KV cache vs overhead - context length is the hidden multiplier practitioners miss (LM Studio's traffic-light fit badges are the mental model; the 80% warn matches practitioner headroom guidance). STT models are ordinary entries here - Step 4 made their accounting real, so there is no separate voice line item and no filename guessing
- Settings view STT section (renamed from voice): the model path/source fields are gone with the old keys; capture tuning (window, interval, vocabulary) remains, plus a **Restore recommended models** button that writes the Step 2 recommended pair into pending config as `[[stt_model]]` entries (creating them, or resetting source/sha256/vram_gb on name match)
- Model deletion consistency: deleting a model (including an STT model) that appears in any profile's Chosen list raises a confirmation dialog (`confirm-modal.ts` exists) naming the affected profiles; on confirm, the save payload removes the model from the catalog **and** from every profile's `models` list in one pending write, so the config never passes through an invalid dangling-reference state (Step 2's validation rejects dangling profile names - that is the backstop, not the UX). Declining the dialog cancels the delete entirely
- UI tests in the existing `.test.mjs` harness: moving an entry between Available and Chosen writes the right `models` list, Set Active round-trips, tabs render the right subsets, STT entries carry the Mic badge, restore-recommended writes the pair, deleting a profiled model strips it from every profile in the same payload, canceling the dialog leaves config untouched, VRAM sums track Chosen membership, the 80% warning icon and over-budget error states fire at the right thresholds

## Part 2: chat template catalog

### Step 6: template catalog module (commit 5)

New module `chat_templates/` in `promptforge-gateway-local` (name describes the concern, not the source):

- Bundled `.jinja` assets for the Step 1 families, embedded with `include_str!`
- `Family` enum with alias resolution (e.g. `qwen25` = `qwen-2.5`) and per-family metadata: default system message (or none), stop tokens
- `family_for_model(hint: &str) -> Option<Family>`: lowercase exact-match table from HF ids to family, ported from Step 1
- `KNOWN_OVERRIDES`: ported from the Studio policy table, but **keyed on the embedded template's content hash, with model id as a secondary key** - Ollama's `template/index.json` validates this pattern (the same broken template ships in many GGUFs; names lie). Reading the embedded template uses the `gguf.rs` parser extension (the review section's optional scope becomes load-bearing here) or the sidecar's recorded `chat_template`
- Tests: every alias resolves; every asset is non-empty and contains a generation prompt marker; mapper hits for the ported ids; **oracle tests** - each bundled template rendered against the Step 1 message fixtures must produce byte-identical output to the Step 1 golden strings. This needs a Jinja engine: add `minijinja` as a **dev-dependency only** (the runtime never renders Jinja - llama-server does, via its embedded minja engine, which is why `--jinja` / `--chat-template-file` need no runtime dependency). Rulebook section 9 justification to state in the commit: a conformant Jinja engine is far past the 100-line bar, minijinja is the maintained ecosystem default (by Jinja2's own author; used for LLM chat templates by HF TGI and mistral.rs), and dev-dependency scoping keeps it out of the published package
- Oracle configuration, from research (`cabinet/_research/2026-08-30-plan-research-minijinja.md`): never use a bare minijinja `Environment` - enable `trim_blocks` / `lstrip_blocks` (transformers enables them; the defaults silently diverge on newlines), `preserve_order`, `minijinja-contrib` pycompat (Python methods), Python-compatible `tojson`, and a pinned `strftime_now`. Candle's bare-minijinja failures on Qwen3/QwQ/Command-R/Granite/Zephyr were exactly these gaps. Evaluate depending on or vendoring `hf-chat-template` (the transformers compat layer + 20-model/68-case byte-parity corpus, adopted by candle) instead of hand-rolling the compat layer. No unwraps in custom filters; cap `tojson` indent (mistral.rs panic precedent)
- **Engine divergence backstop**: the oracle proves the port matches unsloth, but production rendering is minja (llama.cpp's Jinja subset), not minijinja - documented minja divergences include slice steps, `startswith`, and Undefined-vs-None semantics, and Qwen3's official template was once unparseable by minja (llama.cpp #13178). Add an ignored live test (reason string per the rulebook, following the `live_cuda` pattern) that launches a real staged llama-server per bundled family and renders the fixture conversation through it, run on the self-hosted CUDA runner
- **Version-gate the catalog**: record the llama.cpp build (b10082) the bundled templates and overrides were validated against; llama.cpp's own `common/chat.cpp` workaround layer changes between builds and has regressed previously-working GGUFs (#19130), so the catalog states its validated build range
- Record module ceilings in the crate's ratchet if one governs this crate (check for `module-ceilings.toml`); doc comments on all public items per AGENTS.md; module layout follows the rulebook (`chat_templates.rs` plus `chat_templates/` children, no `mod.rs`; split the catalog data from the resolution logic)

Justification per the do-more-with-less rule: the existing `chat_template_file` config key requires per-model operator action and ships zero templates; the sidecar/dialect evidence gathers family markers but cannot select a launch template. Neither can carry a versioned catalog.

### Step 7: auto-selection at launch (commit 6)

Wire the catalog into launch, conservatively. Because operators never edit TOML, the `KNOWN_OVERRIDES` path is what actually protects users - it must work with zero configuration:

- Stage family assets to disk at startup via the existing `ArtifactStore` machinery (llama-server needs a file path): `chat-templates/<family>.jinja` under the cache root, reuse `artifacts/verified.rs` markers
- Resolution in `runtime.rs` `launch_options` (now at 315), precedence: explicit `chat_template_file` path > `chat_template_file = "builtin:<family>"` (extends the existing key's resolution path instead of adding a new config key; the Config UI writes this value) > `KNOWN_OVERRIDES` match (embedded-template content hash first, model id second) > embedded GGUF template. **Never fall back silently**: when no template is usable (no embedded template, no override, no config), refuse the launch with an error naming the model and the fix - Ollama's silent `{{ .Prompt }}` passthrough produces the classic "model never stops" bug, and vLLM hard-errors on templateless chat; both argue for loud failure (research: `cabinet/_research/2026-08-30-plan-research-chat-templates.md`)
- The model source id comes from the existing sidecar metadata (`sidecar.rs`); no new evidence gathering
- Tests: precedence ordering, no override when embedded is fine, `builtin:` resolution, unknown family error naming valid families

### Step 8: Config UI - template selection (commit 7)

Make the catalog visible and operable in the Config UI, building on the Step 5 tab layout. The UI today exposes the raw `chat_template_file` key as a text setting (`ui/src/components/settings-registry.ts:299`); this step replaces that with a structured control. Model settings are declared as data in `LOCAL_MODEL_SETTINGS` and rendered generically, so the control is one registry entry plus a data source.

- Gateway admin surface: expose the catalog to the UI - family list with display labels, and per-local-model effective resolution (embedded / known-override / builtin / custom, plus the family detected). Follow the existing `GET /admin/config` family of routes (`promptforge-gateway/src/config_pending.rs` shows the pattern: bearer-authed, serialized config shape). One new read-only endpoint is acceptable; extending the config payload is preferred if it carries the data without a second fetch.
- Local Models tab: replace the raw `chat_template_file` text entry with a dropdown (`dropdown-control.ts` exists): `Auto (embedded)` / one option per catalog family (writes `builtin:<family>`) / custom path (falls back to the text field). Show the effective resolution and its reason (e.g. "override: known-broken embedded template") as read-only detail.
- Discover view: when the operator adds a model whose HF repo id matches the Step 6 mapper, pre-fill the template selection with the mapped family.
- UI tests in the existing `.test.mjs` harness against `gatewayStub`: dropdown writes the right values, effective-resolution display, Discover pre-fill.

## Part 3: injection defense

### Step 9: control-markup neutralization (commit 8)

Extend `promptforge-core-support/src/untrusted.rs`. The attack literature justifies the work: special-token injection is documented as ChatInject (arXiv 2509.22830), ChatBug (arXiv 2406.12935), and Virtual Context (EMNLP 2024, ~40% jailbreak boost), with a CWE submission in flight (research: `cabinet/_research/2026-08-30-plan-research-injection-defense.md`).

- Add the Step 1 delimiter inventory as a static table, grouped by family, with a `neutralize` pass applied inside `encode` after `<` escaping: space the opener of any inventory delimiter found in untrusted content. The inventory must include **non-special structural tokens** too - HF's own sanitization work (transformers PR #47386) found not all structural tokens are flagged "special", so an inventory keyed only on the special-token flag misses real delimiters
- Framing for the module docs: tokenizer-level separation (HF `split_special_tokens`, vLLM's per-origin tokenization, llama.cpp's jinja input marking) is the gold standard, but the gateway does not control tokenization on remote paths - this pass is the string-level layer of a defense-in-depth stack, and on the local path llama.cpp's input marking composes with it
- The neutralization must also catch the envelope's own nonce if it appears in content (delimiter mimicry is the documented counter-attack against nonce envelopes; the existing `<` escaping already covers the tag form)
- Deterministic and allocation-bounded; one pass; documented as defense in depth (the module docs already disclaim security-boundary status - extend, don't rewrite)
- Preserve the byte-identical wrapping invariant (same input, same nonce, same output) that KV-cache prefix sharing depends on
- Scope: untrusted tool/Lua content only (the existing `wrap` call sites: `execute/tool_loop.rs:259-263`, `promptforge-lua/src/host.rs:99-107`). Assistant replay and tool_call wire payloads are model-generated and stay untouched - mutating them breaks the wire format, and role-differentiated treatment matches vLLM's per-origin tokenization and Zeph's trust levels (the closest Rust prior art)
- Tests: every inventory delimiter is neutralized; ordinary prose (including literal `[INST]` discussion text in already-escaped form) round-trips as documented; envelope invariants hold; a nonce-mimicry input is neutralized

## Step 10: docs and final verify (commit 9)

This step is a **multi-hour documentation run** using the full dokuman pipeline with heavy subagent parallelism. It is not a quick README edit - it is a thorough regeneration of all user-facing documentation from the operator's perspective.

- Rewrite the profile and voice/STT sections of `crates/promptforge-gateway/README.md` for the single-file layout and first-class STT models
- Regenerate the user guide from the **operator's perspective**, not the Rust API. Written in **ASD-STE100 Simplified Technical English**: short declarative sentences, active voice, one meaning per word, approved vocabulary only. Code symbols, config keys, CLI flags, file paths, and UI labels are exempt from the vocabulary rules and stay verbatim. The guide teaches four things:
  1. **Operating the Workshop** - the desktop app: chat, voice input, model menu, profile switching
  2. **Configuring the gateway** - the Config UI (tabs, profiles, models, secrets, settings), and the TOML schema for advanced users
  3. **Writing PromptForge prompts** - the markdown format: frontmatter, H1 title, H2 sections, Lua blocks, prose, tools, fanout, store
  4. **MCP integration** - exposing prompts to Cursor, Claude Code, and other agentic harnesses
  No Rust function names, no crate internals, no `cargo` commands. The reader is an operator or prompt author, not a Rust developer.
- Run `dokuman` on each crate group independently, one full pipeline run per group (recon, extract, consolidate, tier, verify, prepare, write, audit). The groups:
  - `promptforge-cli` (running prompts from the command line)
  - `promptforge-gateway` (serving, profiles, switching, admin API)
  - `promptforge-gateway-local` (local model management, downloads, cache)
  - `promptforge-gateway-config` (config schema, validation, profiles, STT models)
  - `promptforge-stt` (speech-to-text: voice input, transcription endpoint)
  - `promptforge-core` (prompt pipeline: markdown format, Lua, tools, store)
  - `promptforge-mcp-server` (MCP integration for agentic harnesses)
  - `promptforge-dev` (interactive prompt development)
  - `promptforge-tool-picker` (semantic tool binding)
  - `promptforge-webfetch` (web fetch tool)
  Each group gets its own dokuman pipeline run with its own subagents. Frame extraction targets at the operator level: what the user can DO, not what the code exposes. Run groups in parallel where independent (they are - each reads only its own crate's files).
- Update `make-user-guide/src/main.rs` `GUIDES` array to include all groups above (the current array has 7 entries from before the crate split; add the new crates in dependency order). Each crate's guide file follows the existing `user-guide-<crate>.md` naming convention. The `demote_headings` assembler logic is unchanged.
- Full workspace verify per the vibe rulebook

## Verification

- Focused tests per step: `npm run typecheck && npm test` in `crates/promptforge-gateway-config-ui/ui` (Step 0), `cargo test -p promptforge-gateway-config` (Step 2), `cargo test -p promptforge-gateway` (Steps 3-4), `cargo test -p promptforge-stt` and `-p promptforge-workshop-server` (Step 4), `npm run typecheck && npm test` in config-ui (Steps 5 and 8), `cargo test -p promptforge-gateway-local` (Steps 6-7), `cargo test -p promptforge-core-support` and `-p promptforge-core` (Step 9)
- `cargo build -p promptforge-gateway-local --features llama-cuda` compiles after Step 7; `cargo build -p promptforge-workshop` (default `cuda` feature) compiles after Step 4
- Full workspace verify per the vibe rulebook before the final commit
- The existing voice, tool-loop, and untrusted-envelope test suites stay green throughout; fix forward, never rewrite a test to pass

## Data flow

Step 1 supplies the template/mapper/override/delimiter data to Steps 6 and 9. Step 2 delivers the schema (profiles, `[[stt_model]]`, recommended pair, state-file format) and per-profile validation that Steps 3 and 4 rely on. Step 3 delivers the from-config profiles listing and switch endpoints Step 5's UI consumes. Step 4 delivers gateway-owned STT lifecycle and the ArtifactStore-provisioned STT artifacts that Step 5 renders as ordinary catalog entries. Step 5 delivers the tab layout Step 8's template dropdown lands in. Step 6 delivers the catalog consumed by Steps 7 and 8. Step 9 is independent of Parts 1-2 except for Step 1. Steps run sequentially - the vibe loop requires one reviewable commit at a time.

## Not in scope

Per-profile overrides of any field (rejected by design decision), migration tooling or compat loading (hard break), on-demand (lazy) local model loading, exact runtime VRAM measurement (voice and chat models carry declared `vram_gb` estimates; no live GPU polling), broken-template repair (render-diff `add_generation_prompt` fix), disk-space preflight, Windows process-tree kill, `serve --dry-run`, host probes, CI lanes, fan-out auto-sizing. No Ollama Modelfile generation; llama-server consumes Jinja directly.

Recorded follow-up candidates from research, not scheduled: adopting silero-vad in `promptforge-transcribe` (research considers energy-only VAD non-viable in production; the engine currently uses energy-based segmentation - an engine change, deliberately out of Step 4's scope); an SSE `stream=true` extension for `/v1/audio/transcriptions` (Speaches/LocalAI dialect); renaming the `/voice` route path to match the STT vocabulary.


---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: Subset profiles, chat templates, injection defense

## Origin and why

The plan grew out of an unsloth scavenger hunt: "spawn five subagents and explore @unsloth and find areas of functionality which are the same or similar to what we do in @promptforge and report an inventory of what we can steal". From the resulting inventory the user scoped the run to two packages (template catalog, injection defense) and then, while reviewing, expanded it. The user's mental model for the template catalog, stated as a confirming question: "so basically the chat template is to correct bugs in GGUF files after they are shipped?" - the catalog exists so known-bad embedded templates get fixed with zero operator action.

The governing product principle arrived mid-conversation and reshaped everything: "but the new thinking is that the operator must never have to touch the toml files. The Config UI should do it all". This is why template auto-selection is the primary path rather than a convenience, and why `builtin:<family>` exists as a UI-writable value rather than a hand-editing feature.

The profiles redesign was not in the original scope. The assistant proposed writing it as a separate plan; the user overrode that twice: "I wanted this as part of the templates plan". The user's own sketch of the redesign: "a Profile is just a sub-configuration which chooses which of the models are available. 'Checked' as in put a checkmark next to the ones you want. So a Profile at any given time, represents the selected subset of chosen models. Local models will be loaded onto the hardware. Remote models will be simply made available."

The STT move was motivated by an accounting defect the user spotted: "the workshop loads the transcription models directly so it is allocating VRAM that the gateway doesn't know about". The user then reasoned toward gateway ownership out loud: "I feel like it should because the gateway has the UI for selecting the voice models" and "the voice models are, are just like treated like any other models. And the profile determines whether they're, whether they're in, in there or not." The naming decision was delegated to convention: "is it called Voice usually or is it STT ? What do other projects use?" - research said STT, so the capability layer is STT and "voice" survives only in the route path and UI labels.

## Discarded alternatives

- **Option A: single file with `parent=` inheritance.** The user's first sketch included `parent=` instead of `include=[..]`, but accepted the analysis that inheritance "keeps the hard part" (paraphrase): merge semantics, provenance, and override rules survive, only the DAG dies. Rejected for Option B pure checklists.
- **Per-profile override whitelist** (a middle ground allowing e.g. `gpu_layers` per profile). Rejected with the pure-subset choice; the accepted cost is that "same model, different settings per profile" becomes duplicate catalog entries with distinct names.
- **Migration tooling / compat loader.** Hard break chosen instead; the plan records the reason: the old format shipped the same day and has no users to migrate.
- **A separate `enabled` flag for transcription.** An earlier draft had one; profile membership replaced it - transcription is on when the active profile's Chosen list contains STT models.
- **Immediate download from Discover.** Replaced after the user hit the orphan-on-revert bug live: "How about this: Download just queues it for download. The actual download does not happen until you press Apply."
- **Immediate profile switch on Set Active.** The user caught this in the UI and corrected it: "'which profile is active' should be deferred just like every other change".
- **Separate Cache/Downloads tab.** "or actually what if we get rid of the Cache tab and just incorporate this into the Local tab?"
- **Cloud icon for remote models** ("the cloud icon sucks I had no idea what it was"); telephone icon briefly considered; "globe it is".
- **Redefining existing vocabulary** for the new profile semantics: "redefining definitions is confusing as hell".

## Design details dictated by the user

- Dual-list shuttle: "For the profile lets do two lists. Available, and Chosen. You move a model back and forth between Available and Chosen."
- Deletion consistency: "you have to handle the case where the user deletes a model which belongs to a profile. Give them a confirmation dialog and dont forget to remove it from the profile."
- Live VRAM feedback: "as you shuttle items back and forth the usage updates, and you get a warning icon or something at 80% (to leave room for kvcache?)" - the 80% warn threshold originates here.
- Recommended STT pins with restore: "The gateway should have the recommended pins for both of the speech-to-text models. And you can just press a button and restore the recommended anytime."
- Tab order, verbatim: "Settings, Discover, Local, Remote, Profiles, Secrets".
- Discover type filters: "Discover tab needs a set of on/off buttons to filter by type: (Chat, Embedding, Reranker or whatever, STT, Image, TTS)".
- Step numbering style: "I don't want lettered steps. Number them from 0 an increment by 1."
- Research depth mandate: "spawn 8 subagents and research on the internet every step of this plan... I want a broad and deep dive to make this plan sparkle" - this is why the plan cites alacritty, llama-swap, LocalAI, Ollama, Speaches, minijinja, and minja prior art throughout.
- Docs vision: the guide is operator-facing, not API-facing - "the user guide should not be showing rust functions, it should be written from the perspective of operating the workshop, configuring and operating the gateway, writing PromptForge prompts, using the mcp server to export those prompts to other agent harnessses"; register: "write those docs in ASD-STE100 register"; effort: "the docs run could easily go for multiple hours. I want it thorough, and use plenty of subagents".

## Run deviations (from the execution chats)

- **Docs collapsed to one commit.** Mid-run the user edited the plan's docs step ("review the plan, I added docs") and then instructed "one commit." - the multi-commit docs sequence became a single commit.
- **A stray plan was force-merged into this run.** When the agent spun the whisper stderr collision fix into its own plan file, the user rejected the split: "NO WHY THE FUCK DID YOU MAKE A NEW PLAN? JUST PUT IT IN THIS ONE!" The fix was absorbed into this run's scope.
- **HF route consistency pass added.** After the README proxy route came up, the user probed the naming ("wouldn't /admin/hf/{repo} make more sense for EVERY hf path?"), caught the collision flaw themselves ("wait but then you can never have a repo named 'search' or 'model'"), and ordered "review all the hf routes make them consistent".
- **Live bug reports folded into the run:** no UI for adding endpoints, endpoint association failing to save ("no endpoint specified"), Discover READMEs not rendering, secret-field visibility icon flicker, stray indicatif output in the workshop terminal, CPU logical-core count display wrong. These were fixed forward during the run rather than deferred.
- Process note: the user asked the runner to "keep the plan todo's up to date, check the items completed" during execution.
