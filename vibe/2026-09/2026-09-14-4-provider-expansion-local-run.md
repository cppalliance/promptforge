---
name: Provider expansion and local sheet run
overview: "Expand shared-cloud-providers from the ten Prime providers to the full surveyed set (twelve Subprime plus the OpenRouter aggregator), make provider fetches concurrent, and get the executable working locally: the binary loads ~/.promptforge/cloud-provider-secrets.env with override semantics and a local run with real keys produces a schema-valid cloud-provider-models.json. The GitHub workflow in the aggregation repo is deferred."
todos:
  - id: descriptor-keyless
    content: "Descriptor extension: Provider.key_env to Option, fetch_models takes Option<&str>, adapt ten Prime files, keyless fetch path"
    status: pending
  - id: concurrent-fanout
    content: build_sheet_with fetches providers concurrently via FuturesUnordered; BTreeMap keeps output deterministic; in-flight-counter test proves overlap
    status: pending
  - id: secrets-loader
    content: cloud-provider-secrets.env loading (profile-dir path, override semantics) in the binary + .gitignore guard + subprocess test
    status: pending
  - id: subprime-openai-dialect
    content: "OpenAI-dialect Subprime files: minimax, stepfun, groq (STT kind) on openai_shape"
    status: pending
  - id: subprime-rich
    content: "Rich Subprime files: mistral, cohere (token pagination), baidu (CNY pricing), soniox, leonardo"
    status: pending
  - id: keyless-files
    content: "Keyless files: nvidia (no released_at from placeholder) and openrouter (pricing, modalities, deprecation)"
    status: pending
  - id: subprime-heavy
    content: "Heavy files: bedrock (SigV4 + region), foundry (endpoint env), azure_speech (region env)"
    status: pending
  - id: registry-completeness
    content: Registry completeness test extended to all 23 providers with tiers
    status: pending
  - id: local-run
    content: Run the binary locally against the real cloud-provider-secrets.env and verify the emitted cloud-provider-models.json parses as a valid Sheet
    status: pending
isProject: false
---

# Provider Expansion and Local Sheet Run

<product-contract>

## Product Requirements

Two threads join here. The provider-model-sheets run landed ten Prime providers under a prime-only scope selection; the operator corrected that scope to the full surveyed set - Prime + Subprime + OpenRouter, chat and media alike. And the binary currently reads keys only from the process environment; the operator wants local runs driven by a secrets file in the profile directory. This plan expands the registry from 10 to 23 providers, makes fetches concurrent, adds the secrets-file loader, and proves a real local run.

- Problem and users: the sheet covers only the Prime tier, a run with several hanging providers costs N x the 120-second client timeout in serial, and running the binary locally requires exporting keys into the shell by hand. The user is the operator, running the binary locally with real keys; in phase 2, Gateway operators consume the published sheet.
- Goals:
  - Twelve new Subprime provider files in `crates/shared-cloud-providers/src/providers/`: `mistral.rs`, `cohere.rs`, `baidu.rs`, `minimax.rs`, `stepfun.rs`, `bedrock.rs`, `foundry.rs`, `nvidia.rs`, `groq.rs`, `soniox.rs`, `azure_speech.rs`, `leonardo.rs`, plus one Aggregator file, `openrouter.rs`.
  - The `Provider` descriptor extended so keyless providers (NVIDIA, OpenRouter) and multi-credential or regional providers (Bedrock, Foundry, Azure Speech) fit without breaking the one-file-one-provider shape.
  - Provider fetches fan out concurrently; the emitted sheet stays byte-deterministic.
  - The binary loads `<home>/.promptforge/cloud-provider-secrets.env` when present, overriding ambient environment variables.
  - A local run with the real secrets file produces a schema-valid `cloud-provider-models.json`.
- Non-goals: the GitHub workflow in `cppalliance/promptforge-cloud-providers` (deferred - the operator narrowed scope to local execution); Niche static lists; SiliconFlow; any Gateway or UI consumption; changes to the ten landed Prime files beyond what the descriptor extension mechanically requires.
- Success criteria: the registry completeness test covers all 23 providers with their settled tiers; every new provider normalizes a fixture into `ModelEntry`; concurrent overlap is proven deterministically; a provider without a provisioned secret records `unavailable` and never fails the build; the override-proving subprocess test passes; a real local run exits zero with a sheet that parses as a valid `Sheet`.
- Constraints:
  - The secrets file must never be read, printed, copied, or committed by any person, agent, or tool. The binary loads it; tests use fixture files inside temp-dir homes only; nothing else touches it. This is an absolute operator directive (2026-09-14). It applies both to the new `~/.promptforge/cloud-provider-secrets.env` and to the repo-root `secrets.env` the operator already created, until that file is moved.
  - Endpoint facts come from the 2026-09-14 provider-landscape survey recorded in `vibe/2026-09-14-2-provider-model-sheets.md`, whose underlying evidence set is the 2026-09-14 research collection (per-provider model-list endpoint schemas from official docs, the Chinese-provider API survey, the image/STT/TTS provider surveys, and the three live payload captures - Anthropic, OpenRouter, NVIDIA). The collection lives in the workspace's research staging area (`cabinet/_research/`, filenames prefixed `2026-09-14-`); fixture authors should read those extractions before writing JSON by hand. The official docs govern on any conflict.
  - Tiers are a curated product opinion with a functional basis (settled in `vibe/2026-09-14-2-provider-model-sheets.md`): Prime and Subprime have working key-callable model-list endpoints, Niche have none (their slices are static compiled-in lists), Aggregators list many providers' models through one endpoint.
  - The binary contract from `crates/shared-cloud-providers/src/main.rs` stands: keys via each descriptor's `key_env`, previous-sheet URL via `MODELS_SHEET_PREVIOUS_URL` (optional), output to argv[1] (default `~/.promptforge/cloud-provider-models.json`), exit code signals success or failure, HTTP 404 on the previous-sheet URL means first run. The 404-versus-fatal split is deliberate - it is the DEBT-PMS-1 repair (commit `5945b706`): a configured-but-unreachable previous sheet must never silently publish a regressed one.
  - dotenvy is already a workspace dependency (`dotenvy = "0.15"` in `Cargo.toml`); the gateway's dotenv idiom at `crates/gateway/src/runner.rs` is the precedent. Home resolution follows the repo convention: `USERPROFILE` on Windows, `HOME` otherwise (ART-009, `crates/gateway-local/src/artifacts.rs`).
  - Workspace conventions apply: edition 2024, workspace lints, no file over 500 lines, tests beside the change; no secret material in the crate.
- Open questions: None

## Functional Specification

One actor: the operator running the binary locally (the scheduled workflow is deferred). The binary loads the profile-dir secrets file when present, fetches every registered provider concurrently, and writes the sheet. Everything else about the build pipeline - propagation, statuses, exit codes - is unchanged.

- Actors and workflows: the operator runs `cargo run -p shared-cloud-providers <output-path>` from any directory; the binary loads the secrets file, fans out fetches across all 23 registered providers, propagates last-known-good data for failures when history exists, and writes the merged sheet.
- Inputs and outputs: `~/.promptforge/cloud-provider-secrets.env` (local, outside the repo, override precedence) and the process environment in; one `cloud-provider-models.json` at the argv[1] path out; exit code and stderr notes as the run report.
- States and validation: the secrets file missing (or home unresolvable) is ignored with a stderr note - CI and bare-shell runs use the environment as-is; malformed earns a stderr warning and the run continues. Keyless providers never produce `MissingKey`. Providers requiring an endpoint or region env var that is absent record `unavailable`, exactly as a missing key would.
- Errors and recovery: unchanged from the landed propagation matrix - failed provider fetches record `stale` with history or `unavailable` without, and never fail the run; a failed previous-sheet download on a configured URL is fatal before any write; SigV4 signing failures are fetch errors like any other.
- Security and privacy behavior: keys remain env-only or in the profile-dir secrets file; `/secrets.env` stays in promptforge's `.gitignore` as a standing guard because the operator's original file sits at the repo root until moved and every commit path in this workspace stages with `git add -A`; SigV4 signs in memory; no secret reaches the sheet.
- Acceptance criteria: the expanded registry is green under nextest with fixture-backed tests per provider; the concurrency overlap test passes; the secrets-file override test passes; the real local run satisfies the success criteria above.

</product-contract>
<implementation-contract>

## Technical Design

One public descriptor extension, a concurrency change to the sheet builder, a secrets-file loader in the binary, and thirteen provider files. Everything provider-specific stays private to its file, reusing the landed helpers (`openai_shape` for the OpenAI-dialect providers, the pagination idioms from `anthropic.rs` and `gemini.rs`).

- Architecture:
  - `Provider.key_env` becomes `Option<&'static str>`: `None` marks a keyless provider (NVIDIA, OpenRouter), and `fetch_models` takes `key: Option<&str>`. The `build_sheet` keys closure already returns `Option<String>`, so the change is mechanical. This is a public-descriptor change to a crate with no external consumers yet (the Gateway links it in phase 2).
  - Provider fetches fan out concurrently: `build_sheet_with` in `crates/shared-cloud-providers/src/sheet.rs` currently awaits each provider serially. The loop becomes a `FuturesUnordered` fan-out on the single current task (no `tokio::spawn`, so the `&dyn Fn` seams need no `Send`/`Sync` bound), collecting into the same `BTreeMap` so the emitted sheet stays byte-deterministic regardless of completion order. Per-provider failure isolation and the propagation matrix are unchanged. Adds the `futures` workspace dependency if not already present.
  - Secrets loader: `crates/shared-cloud-providers/Cargo.toml` gains `dotenvy.workspace = true`. At the top of `main` in `crates/shared-cloud-providers/src/main.rs`, resolve the home directory (`USERPROFILE` on Windows, `HOME` otherwise) and load `<home>/.promptforge/cloud-provider-secrets.env` with `dotenvy::from_path_override`. Missing file or unresolvable home: stderr note, continue with the environment. Malformed file: stderr warning, continue. The resolution is a few lines in the binary; `shared-*` crates may not depend on `gateway-local`, so the convention is mirrored, not imported.
  - Multi-credential and regional providers keep the descriptor's public shape; the extra reads are private variance: Bedrock's descriptor declares `key_env: Some("AWS_ACCESS_KEY_ID")` while its file privately reads `AWS_SECRET_ACCESS_KEY` and `AWS_REGION` (default `us-east-1`); Foundry declares `AZURE_FOUNDRY_API_KEY` and privately reads `AZURE_FOUNDRY_ENDPOINT` (no default - absent means `unavailable`); Azure Speech declares `AZURE_SPEECH_KEY` and privately reads `AZURE_SPEECH_REGION`.
  - Bedrock signing: SigV4 for `GET https://bedrock.{region}.amazonaws.com/foundation-models`, hand-rolled HMAC-SHA256 (add `hmac` and `sha2` workspace deps if not already in the tree), verified against AWS's published SigV4 test-suite vectors. Normalizes `modelLifecycle` into `Deprecation`; the endpoint reports no context window.
  - OpenRouter (`GET https://openrouter.ai/api/v1/models`, keyless, verified live 2026-09-14): normalize `context_length`, `architecture` input/output modalities, `pricing` (USD per token to per-million-token), `top_provider.max_completion_tokens`, and `expiration_date` into `Deprecation`. Tier: `aggregator`.
  - NVIDIA (`GET https://integrate.api.nvidia.com/v1/models`, keyless, verified live): IDs only, namespaced; `created` is a constant placeholder and must not become `released_at`.
  - Mistral: capabilities object (chat/fim/function_calling/vision), `max_context_length`, `deprecation` with replacement. Cohere: `context_length`, `endpoints`, `features`, token pagination. Baidu: `context_length`, `max_tokens`, modality, pricing in CNY per 1k tokens normalized to per-million-token with `currency: "CNY"`. MiniMax, StepFun, Groq: plain OpenAI shape via the `openai_shape` helper; Groq's STT models get `kind: transcription`. Soniox: per-model languages (dropped - no sheet field - but the STT kind is set). Leonardo: `GET /platformModels`, image kind.
- Modules and interfaces: `Provider` and `fetch_models` signatures change as above; the registry completeness test in `lib.rs` extends to the full 23-provider set with tiers per the settled assignments (Subprime: Mistral, Cohere, Baidu, MiniMax, StepFun, Bedrock, Foundry, NVIDIA, Groq, Soniox, Azure Speech, Leonardo; Aggregator: OpenRouter). No new modules; the secrets loader sits in front of the existing env-var contract and adds no interface.
- File and public API changes: thirteen new files under `crates/shared-cloud-providers/src/providers/`; edits to `lib.rs` (registry, dispatch, completeness test), `sheet.rs` (key closure type, concurrent fan-out), `main.rs` (loader), `.gitignore` (`/secrets.env` guard), `Cargo.toml` files (`dotenvy`, possibly `futures`, `hmac`, `sha2`), and `crates/shared-cloud-providers/tests/sheet_binary.rs` (loader test).
- Data, persistence, failure, security, and privacy constraints: the sheet schema is untouched - every new provider normalizes into the existing `ModelEntry`; pricing normalization keeps the per-million-token rule with explicit currency; the local run writes its sheet to a caller-chosen path outside git's view; no state persists between runs unless `MODELS_SHEET_PREVIOUS_URL` is set, which a first local run does not need.

</implementation-contract>
<verification-contract>

## Testing Plan

Every new provider file proves its normalization against fixtures drawn from the 2026-09-14 research extractions; the descriptor extension is proven by keyless providers fetching with no credential; concurrency is proven by a deterministic overlap test; the loader is proven by a subprocess test with a temp-dir home; the whole is proven by the operator's real local run.

- Unit: per-provider fixture normalization (rich fields for Mistral, Cohere, Baidu, OpenRouter; conservative IDs-only for MiniMax, StepFun, Groq, NVIDIA); Cohere token pagination; OpenRouter pricing unit math and modality mapping; Bedrock SigV4 known-answer tests against AWS's published vectors; Foundry and Azure Speech record `unavailable` when their endpoint/region env is absent. Concurrency: two stub providers whose fetches each increment a shared in-flight counter and park until both are in flight prove overlap deterministically (no timing assertions); the full propagation matrix suite still passes against the fan-out.
- Integration and end-to-end: in `crates/shared-cloud-providers/tests/sheet_binary.rs`, run the binary as a subprocess with `HOME`/`USERPROFILE` pointed at a temp dir whose `.promptforge/cloud-provider-secrets.env` sets `MODELS_SHEET_PREVIOUS_URL` to a loopback stub serving a previous sheet, while the process environment holds a deliberately wrong `MODELS_SHEET_PREVIOUS_URL`; assert the emitted sheet contains the propagated `stale` slice, proving the file both loaded and overrode the environment (the gateway's boot tests use the same temp-home idiom). The existing binary integration suite runs with all keys stripped; the expanded registry appears as `unavailable` slices, never failures. End-to-end: the real local run against the operator's secrets file, output parsed as a `Sheet`, per-provider statuses eyeballed.
- Regression, security, and performance: the ten Prime providers' tests stay green through the `Option` adaptation; no secrets in fixtures; no tool or test reads the real secrets file; OpenRouter's 718 KB payload is bounded by the client timeout, and fixture tests use trimmed excerpts.
- Exit criteria: `cargo nextest run -p shared-cloud-providers`, clippy `-D warnings`, `cargo fmt --check` green; registry completeness covers all 23 providers; the real local run exits zero with a schema-valid sheet.

</verification-contract>
<decision-record>

## Decision Record

- Decisions:
  - Scope is Prime + Subprime + OpenRouter, the full surveyed set: the operator's words - "I did not choose Prime-only, I chose Prime and Subprime" and "everything, obviously, speech, voice, transcript, images, all the results of the research" (2026-09-14), superseding the morning's prime-only selection under which the first ten files landed.
  - The two sibling plans combine into this one: the operator's words - "one combined plan" (2026-09-14). The local-run affordance and the registry expansion land together.
  - Provider fetches run concurrently via `FuturesUnordered` on the current task: the operator's words - "I want it" (2026-09-14), after the serial loop's N x 120-second worst-case timeout cost was laid out. Determinism is preserved by collecting into the `BTreeMap`; no `spawn`, so the injected seams keep their `&dyn Fn` shape.
  - `Provider.key_env` becomes `Option<&'static str>` and `fetch_models` takes `Option<&str>`: the cleanest fit for keyless listing endpoints (NVIDIA, OpenRouter) - without it, a keyless provider would hit `MissingKey` and record `unavailable` despite being reachable. The crate has no external consumers yet, so the public-descriptor change is cheap now and expensive later.
  - The secrets file is `cloud-provider-secrets.env` in the profile directory `<home>/.promptforge`, never cwd-relative: the operator's words - "lets make it ~/.promptforge/cloud-provider-secrets.env" (2026-09-14). It overrides the environment via dotenvy's override variant: "secrets.env is just for local builds. on gha there will be no file, so it will go to the environment. the secrets.env just overrides the environment."
  - No agent or tool ever reads the secrets file: the operator's words - "DO NOT under any circumstances read that file" (2026-09-14). Tests use temp-dir fixtures; the binary alone loads the real file.
  - Multi-credential and regional env reads are private variance inside the provider file, not new descriptor fields: the descriptor keeps its five-field public shape; Bedrock, Foundry, and Azure Speech document their extra env vars in their file docs.
  - SigV4 is hand-rolled over an AWS signing crate: two small HMAC-SHA256 helpers with known-answer vectors beat a heavy AWS SDK dependency for one GET endpoint.
  - MiniMax, StepFun, and Groq reuse the `openai_shape` helper rather than each growing a private parser.
- Rejected alternatives:
  - A `keyless: bool` flag beside a required `key_env`: rejected; `Option` makes the absence type-safe and self-documenting. Revisit never.
  - dotenvy's default non-overriding load: rejected; the operator requires the file to win locally. Revisit never.
  - A cwd-relative or CLI-arg secrets path: rejected in favor of the fixed profile-dir location. Revisit never.
  - Waiting to expand until all Subprime secrets are provisioned: rejected; missing keys safely record `unavailable`, so files can land before consoles. Revisit never.
  - Committing an example `secrets.env.example`: rejected; the key names live in the provider descriptors. Revisit if a second operator appears.
- Assumptions, risks, and notes:
  - The operator's secrets file currently sits at the promptforge repo root and moves to `~/.promptforge/cloud-provider-secrets.env` before the local run; a key absent from the file simply records that provider `unavailable`.
  - The operator's provisioned keys as of 2026-09-14 (their own list): the ten Prime providers plus `MISTRAL_API_KEY` and `COHERE_API_KEY`; NVIDIA and OpenRouter need no key. The first local run should therefore show `ok` slices for those twelve and `unavailable` for Baidu, MiniMax, StepFun, Bedrock, Foundry, Groq, Soniox, Azure Speech, and Leonardo until their consoles are provisioned - that expectation is what "sensible per-provider statuses" means in step 9.
  - Fixture shapes for providers without live captures rest on the documented 2026-09-14 survey; a docs-to-reality drift surfaces as a stale/normalization fix later, never as a build failure.
  - Bedrock entries will be thinner than peers (no context window on the endpoint) until a curation pass adds static data.
  - `crates/shared-cloud-providers/src/sheet.rs` is already near the workspace's 500-line file limit (enforced by `build-xtask`); if the fan-out pushes it over, extract the propagation and static-slice helpers into a sibling module rather than weakening the limit.
  - Listing endpoints are free per the 2026-09-14 survey, so the local run costs nothing.
  - The deferred aggregation workflow's env contract already names every key and configuration variable this expansion reads, so no workflow edit is needed when this lands.

### Deferred and Out of Scope

- Deferred: the GitHub workflow in `cppalliance/promptforge-cloud-providers` (the repo exists, empty, secrets configured). The settled design, self-contained: triggers `workflow_dispatch` plus a weekly cron; checkout `cppalliance/promptforge`, stable Rust toolchain with a build cache, `cargo build --release --locked -p shared-cloud-providers`, run the binary with the full env contract (the ten Prime keys; `MISTRAL_API_KEY`, `COHERE_API_KEY`, `BAIDU_API_KEY`, `MINIMAX_API_KEY`, `STEPFUN_API_KEY`, `GROQ_API_KEY`, `SONIOX_API_KEY`, `LEONARDO_API_KEY`, `AZURE_FOUNDRY_API_KEY`, `AZURE_SPEECH_KEY`, `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`; configuration variables `AZURE_FOUNDRY_ENDPOINT`, `AZURE_SPEECH_REGION`, `AWS_REGION`; `MODELS_SHEET_PREVIOUS_URL` pointing at the stable asset URL), a gate that fails the run if any secret value appears fixed-string in the sheet, and publication as a rolling `models` release with the asset overwritten so `https://github.com/cppalliance/promptforge-cloud-providers/releases/download/models/models.json` never changes. The operator narrowed scope to local execution on 2026-09-14. Revisit when the operator wants scheduled aggregation.
- Deferred: Niche static lists (one compiled-in `.json` per provider). Revisit when the operator curates the first list.
- Deferred: SiliconFlow aggregator. Revisit if OpenRouter coverage proves insufficient for Chinese providers.
- Deferred: Gateway sheet-consumption and config-UI integration (phase 2). Revisit when reliable sheets exist.
- Out of scope: changes to the ten Prime provider files beyond the mechanical `Option` adaptation; managing secret values.

</decision-record>
<project-survey>

## Project Survey

- Status: complete
- Build: `cargo build` (default-members builds only the gateway; desktop app is explicit via `cargo build -p workshop`)
- Focused test command: `cargo nextest run -p <crate> [test-name-filter]`
- Component test command: `cargo nextest run -p <crate>`; boundary/structural harness: `cargo test -p build-xtask`
- Full-suite test command: `cargo nextest run --locked --workspace --exclude workshop --exclude workshop-server --all-features`, then doctests via `cargo test --workspace --exclude workshop --exclude workshop-server --all-features --doc`; workshop crates separately: `cargo nextest run --locked -p workshop -p workshop-server`
- Linter: `cargo clippy --workspace --exclude workshop --exclude workshop-server --all-targets --all-features -- -D warnings` (workshop: `cargo clippy -p workshop -p workshop-server --all-targets -- -D warnings`)
- Formatter check: `cargo fmt --all --check`
- Docs: `cargo doc --workspace --no-deps --all-features --exclude workshop --exclude workshop-server` with `RUSTDOCFLAGS="-D warnings"`; user guide: `mdbook build guide`
- Test placement and naming: integration tests in `crates/<name>/tests/*.rs` (e.g. `gateway-stt-engine/tests/engine_contract.rs`); unit tests inline in `src/` files behind `#[cfg(test)]` modules; nextest profiles in `.config/nextest.toml` with a `heavy` test-group (max-threads 2) for tensor/FFI-heavy suites
- Directory map: `crates/` holds all workspace members (one directory per crate, `shared-ui` excluded as a TypeScript+CSS package); `guide/` is the mdbook user guide; `prompts/` prompt pipelines; `tools/` helper tooling; `vibe/` planning and architecture docs (incl. `archdoc.md`); `images/` assets; `local/` local config; `target/` and `target-msrv/` build outputs; `.githooks/`, `.github/` CI
- Component boundaries: three products - PromptForge (`promptforge-*` crates: parser, lua, api, store, vfs, webfetch, web, model-client, tool-picker, web-search), Gateway (`gateway-*` crates: config, local, protocol, routing, stt family, web-search, whisper-ffi), Workshop (`workshop-*` crates plus the Tauri `workshop` shell); `shared-*` crates hold the public API surface and depend on no product crates; `build-*` crates build outputs. Dependency rules: workshop never depends on gateway; gateway never depends on promptforge or workshop; promptforge never depends on gateway or workshop; outsiders reach PromptForge only through `promptforge-api` (one door). Archdoc components: executor -> gateway/store/Lua VM boundary/shared substrate; gateway standalone; CLI and workshop UI -> executor + gateway + store; VFS layer and shared substrate depend on nothing
- Conventions summary: Rust 2024 edition workspace, BSL-1.0; unsafe forbidden workspace-wide except explicitly owned boundaries with documented invariants; clippy `all` denied, pedantic warn, `unwrap_used`/`expect_used` denied; no file exceeds 500 lines (enforced by `build-xtask`); behavior changes ship with tests in the same change; error messages written for model consumption (concise, factual, self-contained); long-running work reports through `shared-progress`; features gate real constraints, not product shape; workshop crates open lib.rs with a `## Invariants` doc; SPA CSS lives beside its TypeScript using `--ws-*` tokens

</project-survey>
<execution-plan>

## Execution Instructions

Absolute constraint on every step: no tool, test, or agent ever reads the real secrets file; tests use temp-dir fixture homes only. Dependency order: the descriptor extension (step 1) precedes the keyless files (step 6); the secrets loader (step 3) precedes the local run (step 9); the completeness test (step 8) follows all provider files. Per the operator's decomposition guidance (2026-09-14), provider files are grouped into a few large steps and verification stays light until step 9.

<step-1>

### Step 1: Descriptor extension for keyless providers [completed]

- Component: sheet-core

In `crates/shared-cloud-providers/src/lib.rs`, change `Provider.key_env` to `Option<&'static str>` and `fetch_models` to take `key: Option<&str>`; `None` marks a keyless provider and the keyless fetch path never produces `MissingKey`. Mechanically adapt the ten Prime files under `crates/shared-cloud-providers/src/providers/` to the new signatures and adjust the `build_sheet_with` keys closure in `crates/shared-cloud-providers/src/sheet.rs` (it already returns `Option<String>`). Tests: the existing per-provider Prime tests stay green through the `Option` adaptation; add a unit test proving a descriptor with `key_env: None` fetches with no credential present. One commit.

</step-1>

<step-2>

### Step 2: Concurrent provider fan-out [completed]

- Component: sheet-core

In `crates/shared-cloud-providers/src/sheet.rs`, rewrite the serial provider loop in `build_sheet_with` as a `FuturesUnordered` fan-out on the current task (no `tokio::spawn`, so the `&dyn Fn` seams keep their shape), collecting into the same `BTreeMap` so the emitted sheet stays byte-deterministic regardless of completion order; per-provider failure isolation and the propagation matrix are unchanged. Add the `futures` workspace dependency if absent. Tests: two stub providers whose fetches each increment a shared in-flight counter and park until both are in flight prove overlap deterministically (no timing assertions); the full propagation-matrix suite still passes against the fan-out. One commit.

</step-2>

<step-3>

### Step 3: Profile-dir secrets loader [completed]

- Component: sheet-core

Add `dotenvy.workspace = true` to `crates/shared-cloud-providers/Cargo.toml`. At the top of `main` in `crates/shared-cloud-providers/src/main.rs`, resolve the home directory (`USERPROFILE` on Windows, `HOME` otherwise, mirroring the ART-009 convention rather than importing `gateway-local`) and load `<home>/.promptforge/cloud-provider-secrets.env` with `dotenvy::from_path_override`; a missing file or unresolvable home earns a stderr note and a malformed file a stderr warning, and the run continues either way. Add `/secrets.env` to `.gitignore` as a standing guard. Tests: in `crates/shared-cloud-providers/tests/sheet_binary.rs`, run the binary as a subprocess with `HOME`/`USERPROFILE` pointed at a temp dir whose `.promptforge/cloud-provider-secrets.env` sets `MODELS_SHEET_PREVIOUS_URL` to a loopback stub serving a previous sheet, while the process environment holds a deliberately wrong value; assert the emitted sheet contains the propagated `stale` slice, proving the file both loaded and overrode the environment. One commit.

</step-3>

<step-4>

### Step 4: OpenAI-dialect Subprime providers [completed]

- Component: subprime-providers

Add `minimax.rs`, `stepfun.rs`, and `groq.rs` under `crates/shared-cloud-providers/src/providers/`, each built on the `openai_shape` helper with conservative IDs-only normalization; Groq's STT models get `kind: transcription`. Register all three in `crates/shared-cloud-providers/src/lib.rs`. Tests: per-provider fixture normalization, fixtures drawn from the 2026-09-14 research extractions. One commit.

</step-4>

<step-5>

### Step 5: Rich Subprime providers [completed]

- Component: subprime-providers

Add under `crates/shared-cloud-providers/src/providers/` and register in `lib.rs`: `mistral.rs` (capabilities object chat/fim/function_calling/vision, `max_context_length`, `deprecation` with replacement), `cohere.rs` (`context_length`, `endpoints`, `features`, token pagination), `baidu.rs` (`context_length`, `max_tokens`, modality, CNY per-1k pricing normalized to per-million-token with `currency: "CNY"`), `soniox.rs` (STT kind set; per-model languages dropped - no sheet field), and `leonardo.rs` (`GET /platformModels`, image kind). Tests: rich-field fixture normalization per provider plus a Cohere token-pagination test. One commit.

</step-5>

<step-6>

### Step 6: Keyless providers [completed]

- Component: subprime-providers

Add under `crates/shared-cloud-providers/src/providers/` and register in `lib.rs`: `nvidia.rs` (keyless, `GET https://integrate.api.nvidia.com/v1/models`; IDs only, namespaced; the constant placeholder `created` never becomes `released_at`) and `openrouter.rs` (keyless, `GET https://openrouter.ai/api/v1/models`; normalize `context_length`, `architecture` input/output modalities, `pricing` USD per-token to per-million-token, `top_provider.max_completion_tokens`, and `expiration_date` into `Deprecation`; tier `aggregator`). Tests: fixture normalization for both (trimmed excerpts of the 718 KB OpenRouter payload), OpenRouter pricing unit math and modality mapping, and the no-credential fetch path. One commit.

</step-6>

<step-7>

### Step 7: Heavy providers [completed]

- Component: subprime-providers

Add under `crates/shared-cloud-providers/src/providers/` and register in `lib.rs`: `bedrock.rs` (descriptor declares `key_env: Some("AWS_ACCESS_KEY_ID")`; the file privately reads `AWS_SECRET_ACCESS_KEY` and `AWS_REGION` with default `us-east-1`; hand-rolled SigV4 HMAC-SHA256 over `GET https://bedrock.{region}.amazonaws.com/foundation-models`, adding `hmac` and `sha2` workspace deps if absent; `modelLifecycle` normalized into `Deprecation`; no context window on the endpoint), `foundry.rs` (declares `AZURE_FOUNDRY_API_KEY`, privately reads `AZURE_FOUNDRY_ENDPOINT` with no default - absent records `unavailable`), and `azure_speech.rs` (declares `AZURE_SPEECH_KEY`, privately reads `AZURE_SPEECH_REGION` - absent records `unavailable`). Each file documents its extra env vars in its file docs. Tests: SigV4 known-answer tests against AWS's published test-suite vectors; Foundry and Azure Speech `unavailable`-when-env-absent tests; fixture normalization per provider. One commit.

</step-7>

<step-8>

### Step 8: Registry completeness for 23 providers [completed]

- Component: finalization

Extend the registry completeness test in `crates/shared-cloud-providers/src/lib.rs` to the full 23-provider set with the settled tier assignments (Prime: the ten landed providers; Subprime: Mistral, Cohere, Baidu, MiniMax, StepFun, Bedrock, Foundry, NVIDIA, Groq, Soniox, Azure Speech, Leonardo; Aggregator: OpenRouter). Run the existing binary integration suite with all keys stripped: the expanded registry appears as `unavailable` slices, never failures. One commit.

</step-8>

<step-9>

### Step 9: Real local run and final verification [completed]

- Component: finalization

Move the operator's repo-root `secrets.env` to `~/.promptforge/cloud-provider-secrets.env` with a plain `Move-Item` (contents never read by any tool or agent). Run `cargo run -p shared-cloud-providers <output-path>` from any directory with the output path outside git's view; verify exit code zero and parse the emitted `cloud-provider-models.json` as a valid `Sheet`, eyeballing per-provider statuses. Final gate: `cargo nextest run -p shared-cloud-providers`, clippy with `-D warnings`, and `cargo fmt --check` all green. One commit for any fixes the run surfaces.

</step-9>

</execution-plan>
