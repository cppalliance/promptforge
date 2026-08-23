---
name: Gateway program
overview: "The whole gateway program in one plan: extract promptforge-gateway-config; new boot rules (config path from CLI or PROMPTFORGE_GATEWAY_CONFIG, mandatory --profile, sibling profiles dir, boot-owned [server] as a hard error, two env files); dominions replace devices/lanes with truly shared queues and VRAM budgets; profile allowlist; then embeddings/classifiers/streaming; normalization last."
todos:
  - id: step-1-move-modules
    content: "Extract promptforge-gateway-config: move config/ + profile/ + error types, gateway depends on it, moved tests pass"
    status: completed
  - id: step-2-public-api
    content: "Harden public API: accessors replace pub fields, Secret opaque, RawConfig private, doc examples"
    status: completed
  - id: step-3-cli-parsing
    content: "main.rs: one positional (config path) + required --profile NAME, env fallback helper, USAGE, tests"
    status: completed
  - id: step-4-boot-semantics
    content: "runner.rs ServeOptions{config_path, profile}, load_startup boot rules, [server] equality check, env two-file loading; profile.rs/lib.rs/api_error.rs cleanup"
    status: completed
  - id: step-5-boot-docs
    content: "User guide + README for boot rules; end-of-phase obsolescence sweep"
    status: completed
  - id: step-6-dominion-config
    content: "Add DominionConfig + [[dominion]] array + validation; additive dominion/parallel/vram_gb fields on endpoints and local models"
    status: completed
  - id: step-7-dominion-queue
    content: "queue.rs: EndpointLane becomes DominionQueue; reject policy; always round-robin, no new abstraction"
    status: completed
  - id: step-8-shared-queues
    content: "routing.rs: one shared Arc<DominionQueue> per dominion, endpoints bind by id; merge.rs dominion keyed array; shared-limit test"
    status: completed
  - id: step-9-local-wiring
    content: "local/mod.rs: parallel from LocalModelConfig feeds --parallel and queue limit; local endpoints bind dominion queues"
    status: completed
  - id: step-10-vram-check
    content: "VRAM co-residency validation: per-dominion sums, overflow and incomplete-budget errors"
    status: completed
  - id: step-11-profile-allowlist
    content: "Profile models allowlist: post-merge filter, stored on Config, unknown names error, admin status reporting"
    status: completed
  - id: step-12-delete-legacy
    content: "Delete devices/lanes/[queue] and legacy code paths; migrate examples, user guide, README, crate docs"
    status: completed
  - id: persist-research
    content: "Persist chat-only research and decisions to citable research files (rulebook rule 5) before execution"
    status: completed
  - id: verify
    content: cargo fmt/clippy/test/doc per rust-rulebook local loop; manual checks of shared cap, reject policy, VRAM validation
    status: completed
  - id: step-13-model-kinds
    content: "E1: ModelKind (chat/embedding/classifier) on config + kind-scoped validation; ModelInfo carries kind"
    status: pending
  - id: step-14-embeddings-route
    content: "E2a: POST /v1/embeddings wire types + route + remote passthrough through dominion queues"
    status: pending
  - id: step-15-local-embeddings
    content: "E2b: local kind=embedding launches llama-server --embeddings"
    status: pending
  - id: step-16-rerank
    content: "E3: POST /v1/rerank route + local --reranking launch + remote passthrough of the rerank shape"
    status: pending
  - id: n1-catalog-enrichment
    content: "Zed-derived catalog fields: max_output, default_temperature, images, parallel_tool_calls, effort_levels/default_effort, adaptive_thinking; validation"
    status: pending
  - id: n2-dialect-config
    content: "Move tool_dialect/tools_mode to gateway-internal per-model translation config; drop from catalog response"
    status: pending
  - id: n3-request-normalization
    content: "Outbound translation: effort mapping, emulated tool-schema injection, param filtering; Anthropic upstream step series"
    status: pending
  - id: n4-response-normalization
    content: "Inbound translation: emulated tool-call parsing, thinking normalization, finish reasons, Anthropic responses"
    status: pending
  - id: n5-core-dialect-removal
    content: "Delete promptforge-core client-side dialect machinery; integration suite proves zero-dialects"
    status: pending
  - id: step-17-streaming-relay
    content: "S1a: stream:true SSE relay - ChatChunk wire types, Upstream::stream, permit held for stream lifetime"
    status: pending
  - id: step-18-streaming-lifecycle
    content: "S1b: client-disconnect cancels upstream stream; per-chunk minimal shape validation"
    status: pending
isProject: false
---

# Gateway Program

## Context and sequencing

Ordering decision 2026-08-23: extract the config crate FIRST as a behavior-preserving move with the current structs, get it tested and green, then reshape inside the extracted crate. Churn on the fresh public API is free - the gateway is the only consumer - and every later change lands in the new crate with its test suite already in place.

This is the ONLY plan for the gateway program. Phases:

1. **Extract `promptforge-gateway-config`** (steps 1-2): mechanical move, then public API hardening.
2. **Config-path resolution and boot rules** (steps 3-5): CLI positional / `PROMPTFORGE_GATEWAY_CONFIG` / sibling `profiles/` / mandatory `--profile NAME` at boot - the boot file is the catalog, the named profile is the initial loaded set.
3. **Dominions and the profile allowlist** (steps 6-12): reshape the structs inside the new crate, expand-migrate-contract so every commit stays green.
4. **Embeddings, classifiers, and streaming** (components E1-E3, S1): level-4 step decomposition happens when this phase starts.
5. **Normalization layer** (components N1-N5, last section of this document): zero-dialect OpenAI normalization. Always last.

Execution follows `tools-public/rulebooks/vibe-rulebook.md`: each numbered step is one commit carrying code + test + docs; coder and review-and-fix subagents per step; Verify on the rulebook schedule.

Prior art established by research (2026-08-23): Triton's rate limiter is the direct ancestor (named counted resources, `global` shared pools, priority as relative weight); Ray Serve / BentoML / TGI / Kubernetes APF converge on `max_concurrency` + bounded `max_queue` with reject-on-full; DRR / APF seat-seconds / VTC converge on cost-based fairness counters (future upgrade, not v1); llama-swap and Ollama/ModelMesh define the model-packs vocabulary (explicitly out of scope here); llama.cpp router mode validates the one-child-per-model architecture.

Key verified facts about the CURRENT code that motivate this refactor:

- Devices and lanes only supply a concurrency number; they do not create a shared limit. `Routing::from_config` builds one `EndpointLane` per endpoint ([routing.rs:105](promptforge/crates/promptforge-gateway/src/routing.rs)); two endpoints referencing the same remote device each get their own independent queue with the full device concurrency.
- A lane's concurrency value is used in two places: as the llama-server child's `--parallel N` argument and as that model's queue limit ([local/mod.rs:82-105](promptforge/crates/promptforge-gateway/src/local/mod.rs)). Two models sharing a lane each get their own queue with the full lane concurrency.
- VRAM is modeled nowhere; co-residency failures surface as child-process OOMs.

## Phase 1: extract `promptforge-gateway-config` (steps 1-2)

Behavior-preserving extraction with the CURRENT structs (devices, lanes, and all). The reshape happens in phase 3, inside the extracted crate. Decision recorded 2026-08-23: extraction over widening the gateway's lib surface or moving types into `promptforge-core` - the gateway crate is an application (its own docs say so), and consumers (IDE tooling, config editors, `promptforge-cli`) should not pull axum/reqwest to read a TOML file.

**Step 1: move the modules.** New crate `crates/promptforge-gateway-config` receives `config/` (Config, RawConfig DTO, validate, interpolate), `profile/` (ProfileName, include resolution, merge, list/load), and the config error types (`ConfigError` from error.rs and the `ConfigError`/`ConfigErrorKind` portion of api_error.rs; `GatewayError`, `ServeError`, `StartupError` stay in the gateway). Two cross-module dependencies move with them: `QueueConfig` (defined in queue.rs, imported by config.rs) moves into the config crate while the rest of queue.rs stays, and `default_promptforge_root` (local/artifacts) moves too because profile.rs and cache-dir resolution need it. Visibility changes are the minimum needed to compile: section fields become `pub` for now, and step 2 replaces them with accessors. Note: `load_env_chain` and the dotenvy usage move with `profile/` in this step (the move is behavior-preserving); phase 2 later removes env-file loading from the crate entirely, since interpolating from the process environment is the crate's job and populating that environment is the binary's. `promptforge-gateway` depends on the crate and re-exports what its own API needs (`Config`, `Secret`, `ProfileName`); `Gateway`, `ServeOptions`, `run` are unchanged. Manifest and layout follow the rust-rulebook: `edition`/`rust-version`/`license`/`repository` via `*.workspace = true`, `[lints] workspace = true`, the dependency edge declared once in `[workspace.dependencies]` with both `path` and `version`, `unsafe_code = "forbid"` (the crate has no unsafe), a facade `lib.rs` (crate docs, `mod`, `pub use`, no logic), the existing `config.rs` + `config/` layout kept (no `mod.rs`), `pub(crate)` by default. Test: the moved unit tests run in the new crate; the gateway suite passes unchanged. Docs: crate README, module doc paths, Cargo.toml metadata (description, keywords, categories, readme, documentation).

**Step 2: harden the public API.** Read accessors replace the public fields step 1 created on the config section structs (this touches the same gateway call sites twice - accepted, because keeping step 1 a purely mechanical move is worth more); `Secret` stays opaque; `RawConfig` stays private; `Config` remains unconstructable without validation. Accessor style follows the rust-rulebook: no `get_` prefix, borrowed returns (`&str`, `&[ModelConfig]`), `#[must_use]` on constructors and pure accessors, `#[non_exhaustive]` on every public struct and enum. The moved error types are audited against the error rules: thiserror derives, `#[non_exhaustive]` on the public enums, lowercase Display messages with no trailing period and no `failed to` prefix, `#[source]` chains intact. Test: doc examples for `load` / `load_profile` / `from_toml_str` and each accessor. Docs: crate-level docs with a load example; every public item documented with `# Errors` where it applies.

Verify runs at end of phase (rulebook schedule).

## Target config shape

```toml
[[dominion]]
id = "runpod-pool"
kind = "remote"
max_concurrency = 4
max_queue = 50            # bounded wait, then 429-class rejection; default 100
policy = "queue"          # "queue" | "reject" (TGI-style fail-fast); default "queue"
fair_scheduling = true    # per-client round-robin; default true

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24              # local kind only; co-residency budget

[[endpoint]]
id = "runpod-a"
protocol = "openai"
base_url = "https://..."
api_key = "${RUNPOD_KEY}"
dominion = "runpod-pool"  # optional; absent = unlimited private pass-through

[[local_model]]
name = "qwen3.8-local"
source = "https://huggingface.co/..."
sha256 = "..."
context = 32768
dominion = "gpu0"         # optional; local kind required when present
parallel = 4              # child --parallel AND gateway queue limit (preserves today's invariant)
vram_gb = 14              # footprint estimate for the co-residency check
# ...
```

## Target struct shape (`promptforge-gateway-config`)

The public API of the extracted crate. Fields shown for review; public access is via read accessors, and construction is only possible through validation. This is the END STATE after phases 3-5, not what step 1 extracts. Field arrival by phase: `dominion`/`parallel`/`vram_gb` in step 6; `kind` in E1; the `Capabilities` enrichment fields (`max_output`, `adaptive_thinking`, `effort_levels`, `default_effort`, `default_temperature`, `images`, `parallel_tool_calls`) in N1; `dialect` in N2. All public types carry `#[non_exhaustive]` per the rust-rulebook. `Capabilities` is shared by remote and local models via `#[serde(flatten)]` so the TOML stays flat and the two model types cannot drift apart.

```rust
pub struct Config {
    server: ServerConfig,
    local: LocalConfig,
    dominions: Vec<DominionConfig>,
    endpoints: Vec<EndpointConfig>,
    models: Vec<ModelConfig>,            // remote-backed models
    local_models: Vec<LocalModelConfig>, // gateway-hosted GGUF models
    model_allowlist: Option<Vec<String>>, // the profile's selection, if any
    tools: Option<ToolsConfig>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError>;
    pub fn load_profile(dir: &Path, name: &ProfileName) -> Result<Config, ConfigError>;
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError>;
    // + read accessors: server(), dominions(), endpoints(), models(), ...
}

pub struct ServerConfig { pub bind: SocketAddr, pub api_key: Secret }
pub struct LocalConfig { pub cache_dir: Option<String> } // default ~/.promptforge

pub struct DominionConfig {
    pub id: String,
    pub kind: DominionKind,              // Remote | Local
    pub max_concurrency: Option<usize>,  // absent = unlimited
    pub max_queue: usize,                // default 100; full = wait or reject
    pub policy: QueuePolicy,             // Queue (default) | Reject
    pub fair_scheduling: bool,           // default true; per-client round-robin
    pub vram_gb: Option<u32>,            // local kind only
}

pub struct EndpointConfig {
    pub id: String,
    pub protocol: Protocol,              // Openai (only variant for now)
    pub base_url: String,
    pub api_key: Secret,
    pub dominion: Option<String>,        // must name a remote dominion
}

pub struct Capabilities {
    pub description: String,
    pub context: u32,
    pub max_output: Option<u32>,
    pub thinking: ThinkingMode,          // Never (default) | Always | Switchable
    pub adaptive_thinking: bool,
    pub effort_levels: Vec<String>,
    pub default_effort: Option<String>,  // must name a listed rung
    pub default_temperature: Option<f32>,
    pub images: bool,
    pub parallel_tool_calls: bool,
}

pub struct ModelConfig {
    pub name: String,
    pub kind: ModelKind,                 // Chat (default) | Embedding | Classifier
    #[serde(flatten)] pub capabilities: Capabilities,
    pub default_max_tokens: Option<u32>,
    pub upstream: String,
    pub endpoints: Vec<String>,          // first is used; rest reserved
    pub dialect: Option<DialectConfig>,  // gateway-internal translation
}

pub struct LocalModelConfig {
    pub name: String,
    pub kind: ModelKind,
    #[serde(flatten)] pub capabilities: Capabilities,
    pub source: String,                  // https URL or local path
    pub sha256: Option<String>,          // required for remote sources
    pub dominion: Option<String>,        // must name a local dominion
    pub parallel: u32,                   // default 1; child --parallel + queue limit
    pub vram_gb: Option<u32>,            // required when dominion has a budget
    pub gpu_layers: u32,                 // default 99
    pub flash_attention: bool,           // default true
    pub cache_type_k: String,            // default q8_0
    pub cache_type_v: String,            // default q4_0
    pub n_predict: u32,                  // default 8192
    pub chat_template_file: Option<String>,
    pub dialect: Option<DialectConfig>,
}

/// How the gateway talks to this backend: never leaves the gateway,
/// never appears in the catalog response.
pub struct DialectConfig {
    pub tools: ToolsDialect,             // Native | Emulated(JsonMode)
    pub effort: EffortMapping,           // ReasoningEffort | BudgetTokens{map} | EnableThinking
    pub drop_params: Vec<String>,        // explicit unsupported-param drops
}

pub enum DominionKind { Remote, Local }
pub enum QueuePolicy { Queue, Reject }
pub enum ModelKind { Chat, Embedding, Classifier }
pub enum ThinkingMode { Never, Always, Switchable }
pub enum Protocol { Openai }
```

Notes:

- The profile allowlist is stored on `Config` as `model_allowlist: Option<Vec<String>>` (one field; lets `/admin/status` report what a profile selected). The filter still runs during `from_value`, before validation; the filtered vectors plus the stored list are the result.
- `DialectConfig` lives in the config crate but never appears on the wire; `ModelInfo` (stays in the gateway) is built from name/kind/`Capabilities` only.
- Gone: `QueueConfig` (absorbed into `DominionConfig`), `DeviceConfig`/`LaneConfig`, `endpoint.concurrency`, all device/lane references.

## Settled design decisions

- **Delete** `[[device]]`, `[[device.lane]]`, `[queue]` (global section), `endpoint.concurrency`, `endpoint.device`, `local_model.device`, `local_model.lane`.
- **One binding rule**: everything binds by explicit id. No positional TOML nesting, no orphan-attachment machinery.
- **Dominions are truly shared**: one runtime queue instance per dominion, `Arc`-shared by every bound endpoint. This is NEW behavior (today nothing is shared) and is the main behavior change in this refactor.
- **No inline `endpoint.concurrency` sugar**: one way to cap. An endpoint without `dominion` is unlimited, as today when no cap is set.
- **Explicit `kind` on dominions** (better error messages over structural inference). Kind-incompatible payloads rejected, same spirit as CFG-004: `vram_gb` forbidden on remote, lane-style fields gone entirely.
- **Fairness v1**: per-client round-robin (`X-PromptForge-Client`) is the only discipline, with no abstraction layer for future ones (decision 2026-08-23: a one-variant enum is ceremony; the DRR/token-cost change, if it ever unparks, is contained in queue.rs). Document that the fairness key is a self-asserted header - trusted-host callers only.
- **VRAM co-residency check**: at validate time, sum `local_model.vram_gb` per local dominion and reject when the sum exceeds the dominion's `vram_gb`. Runs for free at profile switch because switch loads and validates the full config before committing. Models without an estimate bound to a budgeted dominion: validation error (budgets must be complete to be meaningful).
- **"Model packs" is not a future feature - it is profile hot-swapping, which already ships** (`POST /admin/switch-profile` swaps the whole resident set atomically). The deferred-list mention in the crate docs is stale and gets corrected in this refactor.
- **REJECTED, not deferred: demand-driven per-model load/unload** (llama-swap-style). Decision 2026-08-23: too much mechanism (swap serialization, in-flight drain, eviction policy, co-residency solving) for too little gain. The VRAM-constrained single-GPU operator manually chooses a profile per workload; that is the supported workflow, and a demand-loaded request would eat the model's cold-start latency anyway. Do not re-propose without a new use case that profile switching cannot serve.

## Phase 2: config-path resolution and boot rules (steps 3-5)

Boot rules (all ruled 2026-08-23):

- Boot requires two things: a config path and a profile name. `promptforge-gateway serve gateway.toml --profile analytics`.
- The config path comes from the positional argument or `PROMPTFORGE_GATEWAY_CONFIG`; the CLI path wins. Neither set: usage error naming both sources.
- The profile name comes from `--profile` only (no env var). It is required; there is no anonymous boot. Every gateway has at least one profile; the initial loaded set always has a name.
- Profiles dir is always `<config-file-parent>/profiles` - never independently configurable, no `~/.promptforge/profiles` default.
- The boot file is the CATALOG and infrastructure; it is not loaded as the runtime config directly. The named profile is loaded with include resolution - it typically contains `include = ["../gateway.toml"]` plus a `models = [...]` allowlist - and becomes the initial config. (The allowlist key does not exist until step 11; during phase 2 every profile loads the full catalog.)
- Boot with an unknown profile name: startup error listing the available profiles. Boot when the sibling `profiles/` dir is missing or lacks the named file: startup error.
- The single-file user creates one minimal profile, e.g. `profiles/main.toml` containing only `include = ["../gateway.toml"]`, which loads the full catalog.
- `--profiles-dir` is removed. `--profile` changes meaning: previously an alternative to the config path, now required alongside it.
- Includes remain free-form (PROFILE-009): a profile may include a different file than the boot path (e.g. `../gateway2.toml`), or be self-contained. The boot path only anchors the profiles dir; its content loads only if the profile's include chain contains it. At startup the gateway logs the resolved include chain (already available from `collect_config_chain`), plus a note when the boot file is not in the chain - the likely-mistake case (operator edits the boot file, nothing changes). The chain itself is not an error (multi-catalog directories are legitimate), but the `[server]` rule below still applies: a self-contained profile must replicate the boot file's `[server]` exactly or fail.
- `[server]` is owned by the boot file, enforced as a hard error: after include resolution, the profile's merged `[server]` must equal the boot file's `[server]` exactly - bind address and api_key. Any mismatch fails the boot (or the profile switch) with an error naming both values; revised 2026-08-23 (step 4): an api_key mismatch redacts both keys and names only the profile and the field, because the error reaches logs and the admin API response and `Secret` opacity forbids printing credentials - a bind mismatch still names both addresses. The conventional setup passes by construction because profiles include the boot file. Consequence: the socket and the gateway bearer key are fixed for the process lifetime; the key no longer rotates on profile switch.
- The check is VALUE equality, not path provenance: both sides are compared after `${VAR}` interpolation, so a profile that replicates the block verbatim passes and a genuinely different bind or key fails. Provenance tracking was rejected: the merge machinery does not record per-table origins, the boot file's own `[server]` may itself come from an include, and identical values are identical behavior.
- Env files are cut to two: precedence is the process environment, then `<profile>.env`, then the boot file's sibling env file (e.g. `gateway.env`); dotenvy never overrides, so earlier wins. Included files' env files are no longer loaded. The config crate does no env-file loading at all: `Config::load` / `load_profile` interpolate from the process environment, and populating it from those two files is the binary's job - a library that reads a config must not mutate the process environment as a side effect. At boot the binary loads `<profile>.env` then `<boot>.env`, then interpolates and compares `[server]` in that shared environment. At switch, only the new profile's env file is loaded (the boot env file is already in the process).

**Step 3: CLI parsing (main.rs).** USAGE becomes `usage: promptforge-gateway serve [config.toml] --profile NAME` plus a line documenting the env fallback. `parse_args`: at most one positional (the config path); `--profile NAME` required, parsed into a `ProfileName` at parse time; drop `--profiles-dir`; keep `-h/--help` and unknown-flag rejection. Path resolution goes through a pure, testable helper so tests never touch the process environment (edition 2024 makes `set_var` unsafe):

```rust
fn resolve_config_path(
    cli: Option<PathBuf>,
    env: Option<OsString>,
) -> Result<PathBuf, ParseError> {
    match (cli, env) {
        (Some(path), _) => Ok(path),
        (None, Some(value)) => Ok(PathBuf::from(value)),
        (None, None) => Err(ParseError::Usage(
            "provide a config.toml path or set PROMPTFORGE_GATEWAY_CONFIG".into(),
        )),
    }
}
```

`parse_args` calls it with `std::env::var_os("PROMPTFORGE_GATEWAY_CONFIG")`. Test: CLI-wins, env-fallback, neither-path-set, missing `--profile`, invalid profile name, traversal profile name rejected.

**Step 4: boot semantics (runner.rs, profile.rs, lib.rs, api_error.rs).** `ConfigSource` is deleted. `ServeOptions` becomes:

```rust
pub struct ServeOptions {
    /// Path to the boot TOML (the catalog); profiles dir is its sibling `profiles/`.
    pub config_path: PathBuf,
    /// The profile to boot into; the initial loaded set.
    pub profile: ProfileName,
}
```

Add `profiles_dir_for(config_path)` (parent join `profiles`; a bare filename resolves to `./profiles`). Rewrite `load_startup`: derive the profiles dir, load the two env files via dotenvy (profile first, boot second), resolve the profile via `collect_config_chain` + `Config::from_value` (not the `load_named` shorthand, so the chain is available for the startup log line and the not-in-chain note), extract the boot file's `[server]` by resolving the boot file's own include chain without full validation, and compare. The config crate exposes the entry points this flow needs: a chain-aware profile load and a `load_server(path)` that resolves includes and interpolation but skips full validation (the bare catalog may legitimately fail checks like VRAM). `active = Some(profile)`. `load_env_chain` is deleted from the profile module; dotenvy moves to the binary's startup path. `admin_switch_profile` gains the `[server]` equality check (the boot `[server]` is retained in process state) and loads the new profile's env file before loading. profile.rs: delete `default_profiles_dir` and update module docs. lib.rs: drop the `default_profiles_dir` and `ConfigSource` exports. api_error.rs: rewrite the doctest for the new `ServeOptions::new(config_path, profile)` signature. Test: sibling derivation, boot into a profile with active reported, `[server]` mismatch fails boot and switch, unknown profile error lists available.

**Step 5: boot docs and sweep.** User guide: new invocation, env fallback, sibling `profiles/` convention, catalog-vs-selection split, the minimal starter profile, the `[server]` ownership rule, the secrets rule (process env, then `<profile>.env`, then `gateway.env`; included files' env files are never loaded). README: new invocation + env fallback. End-of-phase obsolescence sweep over the crate (dead arg-parsing arms, unused helpers, unused dependencies via `cargo machete`, stale doc references to the old flags) in its own commit. Verify runs at end of phase.

## Phase 3: dominions and the profile allowlist (steps 6-12)

Expand-migrate-contract: new fields and code paths land additively first, legacy config is deleted last, so every commit compiles and passes. All config-struct work happens in the `promptforge-gateway-config` crate extracted in phase 1. File homes after phase 1: config.rs, validate.rs, and merge.rs live in the config crate; queue.rs, routing.rs, and local/ remain in the gateway.

**Step 6: add dominions to config.** Add `DominionConfig` / `DominionKind` / `QueuePolicy` and the `[[dominion]]` array: `DominionConfig { id, kind, max_concurrency: Option<usize>, max_queue: usize (default 100), policy: QueuePolicy (default Queue), fair_scheduling: bool (default true), vram_gb: Option<u32> }` with `deny_unknown_fields`. `EndpointConfig` gains `dominion: Option<String>`; `LocalModelConfig` gains `dominion: Option<String>`, `parallel: Option<u32>`, `vram_gb: Option<u32>` - all additive; the legacy fields they replace are deleted in step 12. (`parallel` is `Option` during the transition so an unset field cannot override a legacy lane's concurrency with the default; it becomes `u32` with default 1 in step 12.) Validation: unique non-empty dominion ids; `max_concurrency` >= 1 when present; `max_queue` >= 1; `vram_gb` only on local kind; endpoint `dominion` must name a remote dominion; local-model `dominion` must name a local one; `local_model.parallel` >= 1 when present (added at execution 2026-08-23: the list omitted it, but the crate applies a >= 1 check to every concurrency knob and `parallel` feeds the child's `--parallel` and the queue limit, so 0 is meaningless). Devices, lanes, and `[queue]` still parse in this step. Test: dominion validation cases. Docs: user-guide dominion section.

**Step 7: the queue type.** queue.rs: `EndpointLane` becomes `DominionQueue` with the reject-vs-queue policy (reject returns immediately with a 429-mapped error at capacity instead of enqueueing). The discipline is always per-client round-robin when `fair_scheduling` is on; no new abstraction type (decision 2026-08-23: a one-variant enum is ceremony, and if DRR/token-cost fairness ever unparks, introducing a discipline enum then is a change contained in this file). No call-site behavior changes in this step. Test: reject returns at capacity; round-robin behavior unchanged.

**Step 8: shared queues in routing.** routing.rs: build one `Arc<DominionQueue>` per configured dominion; endpoints bind by cloning the Arc; endpoints without a dominion get `DominionQueue::unlimited()` as today; legacy `concurrency`/`device` fields still honored in this step. merge.rs: `dominion` becomes a plain keyed array (key `id`), same handling as `endpoint`/`model`. Test: two endpoints on one dominion share one limit (fill the queue through one endpoint, observe the other blocked).

**Step 9: local model wiring.** local/mod.rs: `parallel` comes from `LocalModelConfig::parallel` when set (lane lookup still works for legacy configs) and feeds both the child's `--parallel` and the model's queue limit, preserving the existing invariant; local endpoints bind to their dominion's shared queue when configured. Test: a local model with `parallel = 3` launches its child with `--parallel 3` and admits at most 3 concurrent requests.

**Step 10: VRAM co-residency check.** validate.rs: per local dominion with `vram_gb`, sum the bound models' estimates; overflow is a located validation error naming the dominion and the overflow amount; a model without an estimate bound to a budgeted dominion is an error. Runs for free at profile switch because switch loads and validates the full config before committing. Test: overflow, incomplete budget, exact fit.

**Step 11: profile allowlist.** Requirement ruled 2026-08-23: the split between "available models" (the catalog) and "loaded models" (the profile) is a first-class feature. A profile file may declare a top-level `models = ["name", ...]` allowlist (the `models` key - an array of names - coexists with the `[[model]]` definition array; RawConfig maps them to distinct fields). After include-merge, the merged document's `[[model]]` and `[[local_model]]` arrays filter to the listed names - BOTH remote and local, so the loaded set IS the catalog and `/v1/models` shows exactly the subset. Absent key means everything (all existing configs unchanged). An unknown name in the list is a hard validation error. Endpoints and dominions referenced only by filtered-out models may remain defined. The filter runs in `Config::from_value` after merge and before validation, so reference validation and the VRAM check operate on the loaded set only. The list is stored on `Config` as `model_allowlist: Option<Vec<String>>` so `/admin/status` can report what the profile selected. Test: subset selection, unknown-name error, absent key = full catalog, profile switch changing the loaded set (including the VRAM check running on the new set).

**Step 12: delete the legacy config.** Remove `DeviceConfig`/`LaneConfig`/`DeviceKind`, the `[queue]` section, `endpoint.concurrency`/`device`, and `local_model.device`/`lane`; `parallel` loses its `Option` and becomes `u32` with default 1; delete the legacy-only code paths in routing.rs and local/mod.rs kept alive by steps 8-9. merge.rs: delete `merge_device_overlay`, `attach_orphan_device_lanes`, `merge_device_entry`, and the `lane` special case in `merge_tables` (the PROFILE-001 located errors stay for the remaining keyed arrays). Migrate `gateway.local.example.toml` and other example configs, the user guide (queue/device sections rewritten around dominions, fairness-key trust caveat, reject policy), README, and the crate docs (feature paragraph; `admin_status` queue note text). The crate-docs deferred list loses only "model packs" in this step; "streaming" is removed by S1 and the Anthropic shim entry by N3 when those ship, not before. Test: legacy keys rejected by `deny_unknown_fields`; integration fixtures (`tests/it/queue.rs`, `profiles.rs`, `local.rs`) migrated; full suite green.

## Middle phase: embeddings, classifiers, and streaming

New model kinds beside chat, with their own routes and wire shapes, plus the streaming chat path. Built on the reshaped config structs; lands before normalization so the translation machinery (when it arrives) covers every kind from the start.

### Component E1: model kinds in config and catalog

- Model entries gain a kind: `chat` (default), `embedding`, `classifier`. Exact spelling at the level-4 pass (a `kind` field vs. separate `[[embedding_model]]` arrays - lean: one `kind` field, since dominion binding, catalog listing, and validation all want one code path).
- Kind-scoped validation: embedding/classifier models reject chat-only fields (tools, effort, thinking); `context` still applies.
- `ModelInfo` carries the kind so catalog consumers can filter.

### Component E2: embeddings route

- `POST /v1/embeddings` in the OpenAI shape: `input` (string or array), `model`, `encoding_format`; response `data[{embedding, index}]` + `usage`.
- Remote endpoints: passthrough for OpenAI-compatible backends.
- Local: `[[local_model]]` with `kind = "embedding"` launches `llama-server` with `--embeddings`; artifact download/digest unchanged.
- Dominion binding is unchanged - one binding rule holds for every kind.

### Component E3: classifiers via the rerank convention

- No OpenAI standard exists for classifiers; adopt the rerank shape that llama-server (`--reranking`), vLLM, and Jina already share: query + documents in, ranked scores out. Route spelling (`/v1/rerank`) decided at the level-4 pass.
- Local classifier models launch with `--reranking`; remote support limited to providers speaking that shape.

### Component S1: streaming chat path

Requirement ruled 2026-08-23: streaming is a requirement, not a deferral. Streaming and non-streaming are both first-class.

- `POST /v1/chat/completions` honors `stream: true` and relays SSE: upstream streaming responses forwarded chunk-by-chunk; non-streaming behavior unchanged.
- The dominion queue permit is held for the stream's whole lifetime (a stream occupies a slot until it ends).
- Client disconnect cancels the upstream stream - cancellation propagates, no orphaned generation burning backend capacity.
- Streamed chunks get the same minimal shape validation as whole responses (choice objects with index + delta), applied per chunk without buffering the body.
- Wire gains the chunk types (`ChatChunk` with `delta`); the `Upstream` trait gains a streaming method beside `send`.
- Local llama.cpp children and remote OpenAI-compatible backends both stream natively, so S1 is relay + lifecycle, not translation. Per-dialect stream translation arrives with normalization (N3/N4 streaming arms).

### Level-4 decomposition (decided 2026-08-23, phase start)

Deferred spellings settled here, each with its falsifier:

- **E1 uses one `kind` field** on `[[model]]`/`[[local_model]]` (`chat` default, `embedding`, `classifier`), not separate arrays - dominion binding, catalog listing, and validation all want one code path. Falsifier: a kind arrives whose config shape shares nothing with chat.
- **E3 route is `POST /v1/rerank`** with the llama-server/vLLM/Jina rerank shape (query + documents in, ranked scores out). Falsifier: an OpenAI-standard classifier endpoint emerges.
- Steps: 13 (E1: `ModelKind` + kind-scoped validation + `ModelInfo` carries kind), 14 (E2a: `/v1/embeddings` wire types + route + remote passthrough through dominion queues), 15 (E2b: local `kind = "embedding"` launches `llama-server --embeddings`), 16 (E3: `/v1/rerank` + local `--reranking` + remote passthrough), 17 (S1a: `stream: true` SSE relay - `ChatChunk` wire types, `Upstream::stream`, permit held for stream lifetime), 18 (S1b: client-disconnect cancels the upstream stream; per-chunk minimal shape validation).
- Verify schedule continues: every 3rd step (15, 18), end of each component (13, 15, 16, 18), and each step's own commit carries code + test + docs.

## Final phase: normalization layer (zero dialects)

Everything below executes LAST, after the config architecture is stable and public. Until this phase completes, `promptforge-core`'s client-side dialect machinery (`dialects.rs`, `normalize.rs`) stays alive - there must be no window where neither side owns tool-call formatting.

**The goal**: clients speak pure OpenAI chat-completions shape. Whatever the backend is - OpenAI-compatible remote, llama.cpp with an exotic chat template, a model whose tool calls exist only as prompt conventions, eventually Anthropic - the gateway translates in both directions. The catalog tells clients *what a model can do*; the gateway alone decides *how to say it*. Zed's provider crates are the reference implementations for the per-backend mappings; Zed's capability vocabulary is adopted, its client-side capability mechanism is exactly what this abolishes for our clients.

### Component N1: catalog enrichment (Zed-derived)

Config (`ModelConfig`, `LocalModelConfig`) and wire (`ModelInfo`) gain capability facts clients need to construct valid requests:

- `max_output: Option<u32>` - output ceiling beside `context`.
- `default_temperature: Option<f32>`.
- `images: bool` (default false).
- `parallel_tool_calls: bool` (default false).
- `effort_levels: Vec<String>` + `default_effort: Option<String>` - supported rungs of the reasoning effort ladder plus the default rung.
- `adaptive_thinking: bool` (default false) - model can self-select thinking depth.

The existing `thinking` tri-state (never/always/switchable) stays. Validation: `effort_levels` non-empty when `default_effort` is set; `default_effort` must name a listed level; effort fields rejected when `thinking = "never"`; `max_output <= context` when both set. Not taken from Zed: fast mode, server-side compaction, cache anchors, billing/policy fields.

### Component N2: gateway-internal dialect config

`tool_dialect` and `tools_mode` change meaning: from client-facing catalog fields to gateway-internal translation configuration.

- **One translator trait per backend dialect**, shaped on LiteLLM's `BaseConfig` (verified 2026-08-23, `litellm/llms/base_llm/chat/transformation.py`): supported-params list, param mapping, request transform, response transform, error mapping - with transport kept OUT of the translator. The trait is the only registry. Avoid LiteLLM's structure: a giant `get_optional_params` if/elif ladder plus a second registry that drifts out of sync with it.
- Per-model dialect config: how to express tools to this backend (`native` | named emulated template), how to express effort (OpenAI `reasoning_effort` string | Anthropic `budget_tokens` rung-to-tokens map | `chat_template_kwargs.enable_thinking` bool collapse | Gemini level), system-message quirks. Exact TOML spelling decided at the level-4 pass.
- **Adopt LiteLLM's shared effort-to-budget table** (`litellm/constants.py`): one canonical rung-to-token-budget map with per-backend emitters. It is the de-facto interop table; per-model overrides live in the dialect config.
- `ModelInfo` drops `tool_dialect`/`tools_mode` from the catalog response (breaking wire change; promptforge-core stops reading them in N5). The wire change and core's reader update must land together, or the fields stay populated until N5 - decide at the level-4 pass, but do not break core between components.

### Component N3: request normalization (outbound)

- One uniform effort knob in (aligned with OpenAI `reasoning_effort`), mapped per the model's dialect config.
- Emulated tools: prefer structured-output emulation (JSON mode / forced-tool) with tool calls parsed out of message content - LiteLLM's working example is Ollama (`format=json` + content parsing); their XML prompt-convention path is dead code with no callers, so prompt-template tool injection is NOT the approach.
- **Strict unsupported-param policy** (LiteLLM semantics): a param the backend can't honor is an error by default; dropping is an explicit per-model opt-in in the dialect config. No silent mutation of client intent.
- Strip or translate parameters the backend rejects (the passthrough `rest` map gets a per-dialect filter).
- **Streaming arm**: per-dialect translation of streamed chunks (SSE in the backend's shape, OpenAI chunks out), building on S1's relay and lifecycle.
- Anthropic upstream (absorbs the deferred "Anthropic protocol shim"): full message/tool/system translation to the Messages API. Largest single translation target; its own step series.

### Component N4: response normalization (inbound)

- Parse emulated tool calls out of model text into OpenAI `tool_calls` objects; malformed tool output becomes well-formed empty content plus a gateway warning field, never a client-visible parse failure.
- Thinking content normalized to one field shape regardless of backend.
- Finish reasons mapped to the OpenAI vocabulary.
- **Streaming arm**: the same translations applied incrementally to chunk streams, including tool-call assembly across chunks (LiteLLM's `CustomStreamWrapper` is the reference for accumulating partial tool calls).
- Anthropic upstream: Messages API response to chat-completions shape, streaming included.

### Component N5: promptforge-core dialect removal (final step)

Once the gateway covers every dialect promptforge-core handles, delete the client-side machinery: `dialects.rs`, the dialect-handling parts of `normalize.rs`, `ToolDialectId`, and the catalog fields that fed them. `ModelDescriptor`/`ModelCatalog` consume the enriched catalog fields instead. This is the step that proves the goal: the client compiles with no dialect code and the integration suite still passes.

### Normalization testing strategy

- Per-dialect golden round-trip tests (canonical to backend-shaped, backend response to canonical, malformed-tool-call recovery).
- Integration tests against mock upstreams per dialect, using the existing `tests/it` harness pattern.
- A conformance matrix doc: models x features (tools native/emulated, effort, thinking, images) with expected normalized behavior.

### Normalization exclusions

- The wire-types export crate (separate parked discussion).

## Rulebook adoption notes (pre-execution)

Per `tools-public/rulebooks/vibe-rulebook.md` (execution flow) and `tools-public/rulebooks/rust-rulebook.md` (all code); both load at execution time.

- Phases 1-3 are decomposed into numbered steps above (each step = one commit: code + test + docs). Phases 4-5 (embeddings/classifiers/streaming, normalization) remain at component level and get their level-4 step decomposition when those phases start.
- Rust-rulebook constraints already folded into the steps: crate manifest and layout rules (step 1), public API and error-design rules (step 2), no abstraction before a second implementation exists (step 7: always round-robin, no discipline type), the full local verification loop (Verification section).
- **Obsolescence sweeps** (requested 2026-08-23): on the Verify schedule - every 3rd step, end of each phase, and the final step - a review subagent sweeps the WHOLE crate surface, not just the step's diff, for code our changes have made obsolete: functions and helpers with no remaining caller, config fields nothing reads, dead code paths, unused dependencies (`cargo machete`), doc comments and guide sections referencing removed concepts, orphaned test fixtures and helpers, public items with no consumer. Removals land in their own commit named for what became obsolete and why (vibe-rulebook rule 7 discipline) - never folded silently into a feature step.
- Rule 5 (plans stand alone): satisfied. Everything decision-relevant is persisted at `promptforge/research/2026-08-23-gateway-design-decisions.md` (all rulings with reasons), `promptforge/research/2026-08-23-prior-art-llm-serving-gateways.md` (Triton, vLLM, TGI, SGLang, LiteLLM, Ollama, llama-swap, llama.cpp router mode, APF, DRR, VTC), `promptforge/research/2026-08-23-zed-litellm-codebase-exploration.md` (Zed capability inventory, LiteLLM translator layer, litellm-rust), and `promptforge/research/2026-08-23-gateway-current-code-findings.md` (verified pre-refactor code facts with file:line references).
- Each step commits with code + test + docs; review-and-fix runs once per step; Verify on the rulebook schedule.

## Explicitly NOT in this plan

- Demand-driven per-model residency (load/evict, TTL, in-flight drain) - REJECTED as a feature, not merely out of scope; see the design-decisions section.
- DRR / VTC token-cost fairness (parked; no scaffolding is built for it - the change, if it ever arrives, is contained in queue.rs).
- Priority weights between binders (Triton-style); revisit only if workload classes become a requirement.
- Fallback chains, cooldown registries, and deployment `order` tiers (LiteLLM has all three; they are parked future features. When fallbacks arrive, the foundation is the Connect-vs-Http error split from litellm-rust: "never reached the provider" is retryable, "upstream already called" is not).
- Multi-instance shared admission (Redis-style counters, LiteLLM proxy v3 pattern). Ruled NOT NEEDED 2026-08-23: dominions are single-process by design; the pattern is recorded here only so a future second-instance requirement knows where to look.
- Anthropic *inbound* shim (accepting Anthropic-shaped requests from clients) - REJECTED 2026-08-23, not deferred: two ingress dialects create more work for clients, and the gateway's purpose is exactly one way of doing things. Clients are always OpenAI-shaped.

## Verification

The rust-rulebook local loop, run per the vibe-rulebook Verify schedule:

- `cargo fmt --all --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --locked --workspace --all-features` (unit + integration) and `cargo test --doc` (the new crate's doctests)
- `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps --all-features`
- Manual checks: two endpoints on one dominion saturate at the shared limit; a local profile that over-books `gpu0`'s VRAM fails validation with a located error; `policy = "reject"` returns promptly at capacity.

---

## Recovered rationale

Recovered from the producing chat sessions by the plan ledger on 2026-09-04. Everything below this heading is derived annotation, not part of the original plan.

# Enrichment: dominion_refactor_ae33684d (Gateway Program)

## Origin of the catalog/profile split

The boot-rules design grew out of a chat with Sean Parsons that Vinnie pasted into the creator chat (2026-08-23). The decisive statement, verbatim: "we want two different things: to configure the list of available models / to configure the current set of loaded models... we dont want every profile to duplicate all the model information... so there would be a split: the regular config file binds model descriptors to identifiers, and a profile defines a set of identifiers which reflects loaded models". Sean contributed the framing analogy: "There's a single config file that defines everything and then you specify which part of the config file you want active. Very similar to AWS profiles." The boot invocation itself was dictated verbatim: "The gateway takes the command line path to the TOML. And it expects a profiles/ dir as a sibling to that path. If the command line does not have the parameter then it looks to PROMPTFORGE_GATEWAY_CONFIG".

Two later refinements also trace to user challenges. The boot-owned [server] rule exists because of the objection "but then the port could change", and the value-equality-vs-provenance decision was the user's framing: "how will you manage this, by comparing the values in [server] or by actually making sure that the root toml file is the same path as the boot root?" The cut from hierarchical env files down to two traces to: "I'm wondering if the hierarchical env is pulling its own weight. why would we need this?"

## Origin of dominions

From the user's riff reacting to the existing config: "we don't operate in terms of devices, we operate in terms of services". On remote queues: "we know that vLLM's queuing capabilities kinda suck... We need to be able to define a queue, and then we need to group the machines in it... all the resources that have that Q, named Q, will, will, they'll all share it. That's how it needs to be." The unification itself was the user's proposal: "should we combine queues and devices into just one concept: dominion".

Budget priority, verbatim: "The VRAM is the primary budget, the max concurrency is the secondary budget." (Paraphrase of the same message: concurrency exists for fairness - to stop a fast workload of many small requests from starving a slow workload that holds its memory reservation for a long time.) The decision to have no scheduling-discipline enum came from a user challenge: "why have the enum at all why not always round-robin?"

## Discarded alternatives (creator chat)

- `gateway add-config` CLI tool, verbatim: "gateway add-config is a non-starter because if we are going to have Claude maintain a command line tool that adds configs, then why dont we just use Claude to add the config directly when we need it instead of asking it to maintain a tool which adds the configs? if a model is smart enough to create the command line tool then it is smart enough to edit the config when asked... on the balance I rather not have to maintain an extra command line tool"
- SQLite-backed data-driven config (Sean's suggestion): dropped as overkill; TOML files suffice. (Paraphrase.)
- Demand-driven model load/unload, verbatim: "I dont think we should ever do model-grained demand load/unload. Its too much try-hard for too little gains. The case for the local user who is tightly budgeted on VRAM (e.g. a single 24GB nVIDIA 4090) has to accept that they will manually choose a profile for their workload each time"
- Anthropic inbound plus hard requirements, verbatim: "anthropic inbound I dont see a point that just creates more work for clients. the whole point of the gateway is not to have two or more ways of doing things. streaming is a requirement we must have streaming and regular. profile allowlist is also a requirement. redis-style shared counters NOT NEEDED for now."

## Why the config crate exists, and the sequencing

Verbatim (disfluencies preserved; the intent is the IDE consumer): "We want PromptForge Gateway to have like, to export... the configuration file is like generally useful... I can build my IDE, and then my IDE can talk to the gateway, and it can get the information about the models, and then it has the right struct, so it can perfectly reflect the metadata." Extract-first sequencing was the user's question-turned-order: "should we factor out the config crate first? Get it well tested, fully working, and so on, and then we can work on gateway piecemeal?" Phase ordering was dictated: "I want the normalization to all come at the very end. the last steps. and I want the beginning steps to focus on the config architecture changes. the structs. the API for people who want to access the config/profile stuff as Rust".

## The zero-dialect goal

Verbatim: "The goal is zero dialects. We're gonna normalize whatever is on the other end, whatever model is on the other end, we're gonna squeeze it into an OpenAI API shaped hole."

## Process constraints imposed in the creator chat

- One plan only, enforced twice: "NO!! god damnit I only wanted one plan".
- Plain-language rule for the plan text: "edit the plan so everything reads in clear simple terms without using abstract metaphors or analogies" and "Just say the fucking thing dont say how you are saying the thing!"
- Obsolescence sweeps, verbatim: "during the vibe, I want the code base periodically reviewed for whatever we can remove for it becomes obsolete because of our chnages" [sic].

## Deviations and additions from the run chats

- Execution was interrupted mid-run; the user ordered: "discard partial work, decompose the remaining work into finer grained steps into a new plan file", then had the plan rewritten in place. Reaction to the result: "I am a little shocked we need 16 steps for this... how did we get so many steps?"
- Normalization scope was deliberately narrowed, verbatim: "if we look at PromptForge Core, it only got these little dialects. So why do we, how much normalization are we doing?... I was thinking we just add them as needed, we don't try to cover every possible model at once." (Paraphrase: dialect coverage is incremental and demand-driven, not upfront-complete.)
- The semantic-blur constraint was added during the run, verbatim: "the point of semantic blur is that when you edit the code I dont want you to bloat it. I want you to make the edits in a way that reverses entropy" - with entropy-reversal rules baked into the plan as an XML-delimited section that step subagents grep for and read in isolation, never reading the whole plan.
- Housekeeping deviations: the four research files survived only after the user asked "are those 4 research files any use? commit them if they are truly useful otherwise delete"; a recover-rationale mechanism was removed on direct order ("I want that recover-rationale thingy deleted").
- Post-execution reflection: 37 commits with roughly 5k net lines added, and the user openly asked "was it worth it? 5k net increase in lines" - the net growth is on record as questioned, not celebrated.
- From the third run chat (mostly an unrelated skillgate session): a downstream motivation for streaming plus thinking normalization - PromptForge Workbench must distinguish thinking from reply blocks, and "my idea was to have the gateway normalize always, so every client gets a clean stream with distinguished blocks".
- The second run chat (step-5 coder subagent) holds no deviations; it confirms steps 1-4 landed as planned.
