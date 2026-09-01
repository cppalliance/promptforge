# promptforge-gateway

[![Crates.io](https://img.shields.io/crates/v/promptforge-gateway.svg)](https://crates.io/crates/promptforge-gateway)
[![docs.rs](https://img.shields.io/docsrs/promptforge-gateway)](https://docs.rs/promptforge-gateway)
[![License](https://img.shields.io/crates/l/promptforge-gateway)](LICENSE)

The credential-holding inference gateway for PromptForge. It serves an OpenAI-compatible HTTP API that routes chat completions to configured backends, holds every credential, manages a model catalog, runs a built-in web search tool, and optionally spawns local `llama-server` processes for GGUF models. Nothing above it holds a vendor key. A key rotation touches one file on one host.

## Installation

```bash
cargo install promptforge-gateway
```

## Usage

```bash
promptforge-gateway serve gateway.toml --profile main
```

Boot requires a config path and a selected profile. The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable (the CLI argument wins). The profile comes from `--profile NAME`, the `PROMPTFORGE_PROFILE` environment variable, or the sibling state file, in that precedence; with none set, startup refuses and lists the profiles the config defines.

Configure endpoints, models, and credentials in the TOML catalog. The gateway accepts `POST /v1/chat/completions`, serves a model catalog at `GET /v1/models`, and, with `workshop`, accepts OpenAI-compatible multipart transcription at `POST /v1/audio/transcriptions`.

Embedding hosts use the library API instead of the binary: `spawn` starts the gateway on a dedicated thread with its own runtime and blocks until the listener is bound, returning a `GatewayHandle` that carries the bound URL and a graceful-shutdown switch (`url()`, `shutdown()`, `join()`).

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Profiles

One `gateway.toml` holds the entire catalog, opened by `config-version = 2`: global sections once (`[server]`, `[workshop]`, `[local]`, `[tools]`, `[[endpoint]]`, `[[dominion]]`), remote models as `[[model]]`, local models as `[[local_model]]`, and speech-to-text models as `[[stt_model]]`. Profiles are named checklists over that catalog:

```toml
[[profile]]
name = "work"
models = ["gpt-5", "qwen3-local", "whisper-base-en", "whisper-small-en"]

[[profile]]
name = "travel"
models = ["qwen3-local"]
```

The active profile filters the catalog before validation and spawn: checked remote models route, checked local models spawn `llama-server` children, checked STT models load into the gateway-owned transcription engine. Membership is the entire definition; profiles carry no per-field overrides.

The active profile is not a config key. It lives in a sibling state file - `gateway.toml` maps to `gateway.state.toml` - with one canonical key:

```toml
active_profile = "work"
```

Every profile is validated at load: names are unique and legal, every listed model exists in the catalog, and each profile's local and STT subset is checked against dominion VRAM budgets, so a live switch can never land on an invalid profile. A state file naming a profile that no longer exists is a startup error naming the stale value.

Switching runs from the already-loaded catalog through `POST /admin/switch-profile`, streaming its stages (`loading-profile`, `stopping-models`, `starting-models`, one terminal event) over SSE. In-flight inference requests get a bounded drain - up to 30 seconds - before stragglers are cancelled and local children stop; the new subset then spawns and routing swaps atomically, and the choice persists to the state file. The Config UI stages `active_profile` as pending state like any other edit, so the switch lands atomically on Apply.

The pre-2 layout is a hard break with no compat loader: a config carrying an `include` key, a top-level `models` allowlist, or the old `[workshop.voice]` model keys, a missing or wrong `config-version`, or a sibling `profiles/` directory fails to load with an error naming the file, key, and line, plus the replacement to use.

## The `[server]` section

The boot config's `[server]` section is required and has no defaults. Both fields accept `${VAR}` interpolation from the process environment.

| Field | Default | Meaning |
|---|---|---|
| `bind` | required | Socket address the gateway listener binds. |
| `api_key` | required | Shared bearer key every `/v1/*` request must present. |

Like `[workshop]`, the section is process-owned: a pending edit that changes it is promoted to disk on Apply but takes effect only on restart, reported as `restart_required` in the apply response.

## The `[local]` section

| Field | Default | Meaning |
|---|---|---|
| `cache_dir` | `~/.promptforge` | Root directory for GGUF files and the pinned `llama-server` installs. |
| `llama_backend` | `"auto"` | Which `llama-server` build to download on Windows x86-64: `auto` picks from the host's GPUs (Blackwell gets the PromptForge CUDA build, any other NVIDIA GPU the upstream CUDA 13 build, anything else Vulkan); `cuda-blackwell`, `cuda`, and `vulkan` force the row. Consulted only on Windows x86-64. |
| `llama_server_path` | none | Explicit `llama-server` executable path, skipping the managed download entirely. |

The `llama-server` executable resolves in a fixed order: `llama_server_path` from the config, then the `PROMPTFORGE_LLAMA_SERVER` environment variable, then the managed download under the cache directory. Both CUDA builds ship their runtime DLLs, so the host needs only the NVIDIA driver. When a download fails and an older install is already in the cache, the gateway uses the cached one and logs a warning rather than failing to start.

## Hosting the workshop

Built with the `workshop` feature, the gateway can host the PromptForge Workshop UI server on a second, loopback-only listener in the same process. Hosting is switched on by a `[workshop]` section in the boot config; without the section (or without the feature) the gateway runs headless.

```bash
cargo build -p promptforge-gateway --features workshop
```

Five feature flags exist:

- `local` (default) - compiles in gateway-owned local inference via the `promptforge-gateway-local` crate: GGUF provisioning, managed `llama-server` children, the blob cache behind the `/v1/cache` routes, the `GET /admin/orphans` listing of cache files no loaded `[[local_model]]` entry references (sizes from the filesystem, digests only from cache sidecars - multi-gigabyte blobs are never re-hashed), the `GET /admin/model-info?path=` GGUF-header readout of a cache file's architecture, layer count, and parameter count (the `path` must stay inside the artifact cache; only the header is read, never tensor data), and the bearer-authenticated `GET /admin/chat-templates` catalog used by the Config UI. A `--no-default-features` build is headless of local inference: it links neither the archive/extraction stack nor a blocking HTTP client, and it refuses a configuration declaring `[[local_model]]` at startup and on profile switch.
- `web-search` (default) - compiles in the Brave-powered `POST /v1/tools/web_search` tool service via the `promptforge-web-search-service` crate. A `--no-default-features` build omits the route entirely.
- `workshop` - compiles the hosted workshop and gateway-owned `promptforge-stt` runtime, including `/voice` on the workshop listener and `/v1/audio/transcriptions` on the gateway listener.
- `config-ui` (default) - compiles in the embedded config SPA via the `promptforge-gateway-config-ui` crate and serves it at `/config/` on the gateway's own port (no second listener); `GET /config` redirects to `/config/`. The routes are loopback-only and carry no bearer auth (the SPA shell holds no secrets); Node/esbuild and `rust-embed` enter the build only with this feature: Node 22 is needed on the build machine for the UI bundle's esbuild step, not for Rust itself, and a `--no-default-features` build needs no Node at all. Regardless of the feature, the admin config endpoints (config read/write, env, pending state, apply/revert, orphans, system, model-info, chat templates, the HF proxy, profile create/delete, reveal) sit behind the shared loopback wall from the always-on `promptforge-gateway-loopback` crate: a non-loopback peer gets 403 before bearer auth even runs.
- `workshop-cuda` - implies `workshop` and enables `promptforge-stt/cuda`: the whisper CUDA backend for speech-to-text, which needs the CUDA Toolkit on the build machine. Local inference CUDA is a run-time concern instead: on Windows the gateway downloads a CUDA `llama-server` build when the host GPU calls for it (see below).

The workshop's toolchain stays opt-in: Node/esbuild (the workshop UI bundle) and whisper enter the gateway build only with `--features workshop`.

### The `[workshop]` section

| Field | Default | Meaning |
|---|---|---|
| `bind` | `127.0.0.1:7910` | Socket address of the workshop listener. Must be a loopback address; a non-loopback bind is refused at startup. |
| `open_browser` | `false` | Open the system browser at the workshop URL once it is serving. Meant for running the gateway (or hosting the UI) without the desktop shell; a browser that fails to open is logged, never fatal. |

### Speech-to-text models

Speech-to-text models are first-class catalog entries, provisioned through the same pinned, digest-verified cache machinery as local chat models and governed by profile membership: transcription is on when the active profile's list contains STT models, off otherwise. A gateway with no `[workshop]` section refuses an active profile that selects STT models, same as a `--no-default-features` build refuses `[[local_model]]`.

```toml
[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0
```

| Field | Default | Meaning |
|---|---|---|
| `name` | required | Catalog name that `[[profile]].models` entries reference. |
| `role` | required | `interim` (the streaming passes while a take records) or `final` (the crystallizing pass at stop). |
| `source` | required | `https` download URL or operator-controlled local path; plaintext `http` is rejected. |
| `sha256` | none | Lowercase hex SHA-256 pin, enforced after download when set. |
| `vram_gb` | required | Estimated VRAM use in gibibytes; counts toward a bound dominion's budget. |
| `dominion` | none | Local dominion that accounts for this model's VRAM. |

A profile may select at most one interim and one final STT model. Interim without final is allowed as a degraded mode: nothing crystallizes mid-take and the final pass falls back to one interim decode at stop. Final without interim is a validation error naming the fix. The config crate ships a digest-pinned recommended pair - `whisper-base-en` (interim) and `whisper-small-en` (final) from the whisper.cpp Hugging Face repo - and the Config UI's **Restore recommended models** button writes both entries into the pending config.

`[workshop.stt]` (optional) configures push-to-talk capture tuning. Model
sources, pins, and interim/final roles live in the global `[[stt_model]]`
entries above; the active profile enables them by catalog name.

| Field | Default | Meaning |
|---|---|---|
| `window_seconds` | `15` | Seconds of trailing audio each interim pass transcribes. |
| `interval_ms` | `500` | Milliseconds between interim passes while a take is recording. |
| `vocabulary` | `[]` | Domain terms whisper is biased toward. Empty disables biasing. |

`[workshop.tape]` (optional) is accepted for compatibility and ignored: the workshop no longer records a session tape. Agent sessions persist their event logs as JSONL under the workshop's state directory (`sessions/` beside the boot config).

## Local model companions

A chat `[[local_model]]` can declare two companions, each provisioned through the same pinned, digest-verified cache machinery as the main model:

```toml
[[local_model]]
name = "gemma-4"
description = "Gemma 4 E2B instruct with MTP drafting and vision"
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-UD-Q4_K_XL.gguf"
sha256 = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32"
context = 131072

[local_model.speculative]
type = "draft-mtp"
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mtp-gemma-4-E2B-it.gguf"
sha256 = "9eba819938efccfd6044f8af84e3bbfddc639a2bcf32ebc36420e6a649191919"
draft_max = 2

[local_model.multimodal_projector]
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf"
sha256 = "140be8d7849741f88c50757d529b84373ee8e27052cc2236855b537f4a8215fa"
```

`[local_model.speculative]` attaches a multi-token-prediction drafter: the child launches with `--spec-draft-model`, `--spec-type draft-mtp`, and `--spec-draft-n-max` (`draft_max`, bounded to `1..=16`). `[local_model.multimodal_projector]` attaches a vision projector (`--mmproj`) so the model accepts image inputs, and the catalog advertises `images = true` for it. Companion sources follow the main source's rules: an `https` URL requires a `sha256` pin, a local path may go unpinned, and plaintext `http` is rejected. Both companions are chat-only and validated at load. The resolved paths live in the child's launch state, so a respawn re-emits the exact verified artifacts, and a model without companions gets the same command line as before companions existed.

### Chat templates

A local chat model's template is chosen at launch with a fixed precedence: an explicit `chat_template_file` path, then `chat_template_file = "builtin:<family>"` (a bundled, versioned template staged into the cache), then a known-override match (the gateway ships a table of known-broken embedded templates, matched by embedded-template content hash first and model id second), then the GGUF's embedded template under the always-present `--jinja` flag. When none of these yields a usable template the launch is refused with an error naming the model and the fix; there is no silent passthrough. The Config UI surfaces this on each local model as a dropdown - Auto, one option per bundled family, or a custom path - with a read-only summary of the effective source, the detected family, and the reason behind the decision.

### Derived client credentials

There is no `[workshop.gateway]` sub-table. The hosted workshop reaches the gateway through its own HTTP client, and that client's `base_url` and `api_key` derive from the `[server]` section: the URL is the `[server]` bind with an unspecified address swapped for loopback (`0.0.0.0` becomes `127.0.0.1`, `[::]` becomes `[::1]`), and the bearer key is the `[server]` `api_key` itself. No credential is duplicated in `[workshop]`, so none can drift.

### Process-owned sections

`[server]` and `[workshop]` are process-owned. Apply promotes edits to them to disk and answers `restart_required: true`; the running process keeps its booted listener and STT capture settings until restart. A profile switch never changes them, because profiles are checklists over the model catalog and carry no sections.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
