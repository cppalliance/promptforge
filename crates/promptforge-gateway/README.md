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

Boot requires two things: a config path and a profile name. The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable (the CLI argument wins). The profile is required and is loaded from the `profiles/` directory beside the boot file; a minimal `profiles/main.toml` containing only `include = ["../gateway.toml"]` loads the full catalog.

Configure endpoints, models, and credentials in the TOML catalog. The gateway accepts `POST /v1/chat/completions` and serves a model catalog at `GET /v1/models`. Bearer-authed admin routes cover profile switching and status, `GET /admin/config` renders the running configuration as JSON (secrets redacted, each entry annotated with the file it came from, the profile leaf's own `include` array carried verbatim and ordered), `GET /admin/system` reports a live host-metrics snapshot - CPU, RAM, the drive holding the artifact cache, and the NVIDIA GPU when an NVML-capable driver is present (the GPU and disk fields are absent, never errors, when unavailable) - `GET /admin/hf/search` and `GET /admin/hf/model/{repo}` proxy the Hugging Face hub's model search and detail APIs verbatim (attaching the process `HF_TOKEN` when set, so public repos work without one and the browser never holds the token), and `GET /admin/progress` streams the process's live operation progress as server-sent events. Shadow-file write routes stage pending config edits beside the real files without touching them or reloading anything: `PUT /admin/config` writes the active profile leaf's `.toml.next`, `PUT /admin/boot-config` the boot config's, `PUT /admin/include/{path}` an included file's (confined to the profiles directory and the active profile's pending include chain), and `PUT /admin/env` an env file's shadow (the active profile's by default, the boot config's with `?scope=boot`), each validating the merged pending configuration before any byte lands and preserving `"***"`-redacted secrets from the current files; `GET /admin/env` returns the real boot and profile `.env` files parsed, values included, plus a `references` map naming which pending-config fields reference each variable through `${VAR}` (scanned from the raw pre-interpolation chain, shadows preferred, since a loaded config interpolates references away and redacts secrets). Pending-state reads report the staged edits back: `GET /admin/config-pending` renders the merged pending view (the include chain resolved with shadows preferred, same shape as `GET /admin/config`, secrets `"***"`, provenance naming the `.next` files, and a distinct `boot` side for the restart-required banner), and `GET /admin/config-dirty` returns `{dirty, pending_files, changed_sections}` from shadow existence and real-versus-pending comparison, `.env` shadows included. An explicit apply promotes the staged state: `POST /admin/config-apply` renames every shadow over its real file (atomic per file) and reloads the active profile when a profile-scoped or profile `.env` shadow was promoted - a promoted boot config or boot `.env` shadow reports `restart_required` instead, because boot-owned state loads only at the next startup - and `POST /admin/config-revert` deletes every shadow, leaving the real files untouched. Profile files themselves are managed directly (creating a profile is not a pending edit): `POST /admin/profiles/{name}` creates `profiles/{name}.toml` atomically as an empty file, a verbatim copy of another profile, or an include leaf whose only content is `include = ["<from>.toml"]` (refusing an existing name, an invalid or Windows-reserved name, a missing `from`, and a self-reference), and `DELETE /admin/profiles/{name}` deletes the file plus its shadow, refusing the active profile. `POST /admin/reveal` opens the host OS file manager at a named path - the UI's reveal-in-folder button for model files and config files - accepting only loopback callers (403 otherwise, bearer key still required) and only paths that canonicalize to strictly inside the artifact cache or the profiles directory (400 outside or at a root itself, 404 when missing); the file manager is spawned directly with separate arguments, never through a shell, and the route replies 204 without waiting for it.

Embedding hosts use the library API instead of the binary: `spawn` starts the gateway on a dedicated thread with its own runtime and blocks until the listener is bound, returning a `GatewayHandle` that carries the bound URL and a graceful-shutdown switch (`url()`, `shutdown()`, `join()`).

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## The `[server]` section

The boot config's `[server]` section is required and has no defaults. Both fields accept `${VAR}` interpolation from the process environment.

| Field | Default | Meaning |
|---|---|---|
| `bind` | required | Socket address the gateway listener binds. |
| `api_key` | required | Shared bearer key every `/v1/*` request must present. |

Like `[workshop]`, the section is owned by the boot config: a profile whose merged `[server]` differs from the boot file's is refused at startup or, on a mid-run switch, with the running state left untouched.

## Hosting the workshop

Built with the `workshop` feature, the gateway can host the PromptForge Workshop UI server on a second, loopback-only listener in the same process. Hosting is switched on by a `[workshop]` section in the boot config; without the section (or without the feature) the gateway runs headless.

```bash
cargo build -p promptforge-gateway --features workshop
```

Six feature flags exist:

- `local` (default) - compiles in gateway-owned local inference via the `promptforge-gateway-local` crate: GGUF provisioning, managed `llama-server` children, the blob cache behind the `/v1/cache` routes, the `GET /admin/orphans` listing of cache files no loaded `[[local_model]]` entry references (sizes from the filesystem, digests only from cache sidecars - multi-gigabyte blobs are never re-hashed), and the `GET /admin/model-info?path=` GGUF-header readout of a cache file's architecture, layer count, and parameter count (the `path` must stay inside the artifact cache; only the header is read, never tensor data). A `--no-default-features` build is headless of local inference: it links neither the archive/extraction stack nor a blocking HTTP client, and it refuses a configuration declaring `[[local_model]]` at startup and on profile switch.
- `web-search` (default) - compiles in the Brave-powered `POST /v1/tools/web_search` tool service via the `promptforge-web-search-service` crate. A `--no-default-features` build omits the route entirely.
- `workshop` - compiles the hosted workshop in: the `promptforge-workshop-server` crate and system-browser opening.
- `config-ui` - compiles in the embedded config SPA via the `promptforge-gateway-config-ui` crate and serves it at `/config/` on the gateway's own port (no second listener); `GET /config` redirects to `/config/`. The routes are loopback-only and carry no bearer auth (the SPA shell holds no secrets); Node/esbuild and `rust-embed` enter the build only with this feature. Regardless of the feature, the admin config endpoints (config read/write, env, pending state, apply/revert, orphans, system, model-info, the HF proxy, profile create/delete, reveal) sit behind the shared loopback wall from the always-on `promptforge-gateway-loopback` crate: a non-loopback peer gets 403 before bearer auth even runs.
- `llama-cuda` - implies `local`; on a native Windows x86-64 build with CUDA Toolkit >= 12.8, compiles the pinned `third_party/llama.cpp` submodule during the Cargo build into a Release `llama-server` for the build machine's visible GPUs, and embeds the resulting bundle (manifest plus runtime files) into the gateway binary. A no-op on every other target, where the platform backend archive path is unchanged.
- `workshop-cuda` - implies `workshop` and `llama-cuda`, and builds the whisper voice engine with CUDA acceleration.

The workshop's toolchain stays opt-in: Node/esbuild (the workshop UI bundle) and whisper enter the gateway build only with `--features workshop`.

### CUDA llama-server builds

A `llama-cuda` build needs three things on the build machine: the pinned llama.cpp sources checked out (`git submodule update --init`), a Windows x86-64 host with CUDA Toolkit >= 12.8, and the NVIDIA GPUs the server should run on. The build detects every visible GPU's compute capability and compiles only those architectures; cross-compilation is rejected.

All native compilation happens during the Cargo build: the `promptforge-gateway-local` crate's build script (backed by the `promptforge-gateway-build` crate) compiles the submodule into a Release `llama-server`, records a versioned manifest (source commit, tool identities, architectures, per-file SHA-256), and embeds the manifest and runtime files into the gateway binary. At runtime the gateway never invokes a compiler or build tool: it validates the embedded payload against the manifest, checks that the host provides the declared CUDA Toolkit runtime DLLs, and atomically stages the files into the operator cache. A valid matching installation is reused without restaging, and a CUDA build never silently falls back to the Vulkan archive.

Build failures surface as Cargo build errors from the build script. Staging failures surface at gateway startup as a provisioning error naming the validation that failed (tampered payload, target mismatch, missing toolkit DLL). Embedding hosts can also read a bounded, credential-redacted tail of each child's captured stdout/stderr through `Gateway::local_diagnostics` - for example to confirm the child reported a CUDA device and offloaded its layers to the GPU.

On a suitable host, the ignored live integration test proves the whole path (embedded-bundle staging, CUDA device report, GPU-layer offload, digest pins, MTP acceptance, cache reuse, a tool call, and a projector completion):

```bash
cargo test -p promptforge-gateway --features llama-cuda -- --ignored live_cuda   # needs PROMPTFORGE_LIVE_CUDA=1
```

Without `llama-cuda`, the Windows/Linux Vulkan and macOS Metal archive provisioning path is unchanged.

### The `[workshop]` section

| Field | Default | Meaning |
|---|---|---|
| `bind` | `127.0.0.1:7910` | Socket address of the workshop listener. Must be a loopback address; a non-loopback bind is refused at startup. |
| `open_browser` | `false` | Open the system browser at the workshop URL once it is serving. Meant for running the gateway (or hosting the UI) without the desktop shell; a browser that fails to open is logged, never fatal. |

`[workshop.voice]` (optional) configures push-to-talk transcription:

| Field | Default | Meaning |
|---|---|---|
| `interim_model` | empty | Path to the whisper model for interim (streaming) transcription. Empty disables transcription. |
| `final_model` | empty | Path to the whisper model for the pipelined final pass. Empty disables the final pass. |
| `interim_source` | empty | URL the interim model can be downloaded from. Empty means no known source. |
| `final_source` | empty | URL the final-pass model can be downloaded from. Empty means no known source. |
| `window_seconds` | `15` | Seconds of trailing audio each interim pass transcribes. |
| `interval_ms` | `500` | Milliseconds between interim passes while a take is recording. |
| `vocabulary` | `[]` | Domain terms whisper is biased toward. Empty disables biasing. |

`[workshop.tape]` (optional) configures the session tape:

| Field | Default | Meaning |
|---|---|---|
| `path` | `tape.jsonl` | Path of the JSONL tape file. A relative path resolves against the directory holding the boot config, never the process current directory; an absolute path is used unchanged. An absent `[workshop.tape]` anchors the default `tape.jsonl` the same way. |

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

### Derived client credentials

There is no `[workshop.gateway]` sub-table. The hosted workshop reaches the gateway through its own HTTP client, and that client's `base_url` and `api_key` derive from the boot `[server]` section: the URL is the `[server]` bind with an unspecified address swapped for loopback (`0.0.0.0` becomes `127.0.0.1`, `[::]` becomes `[::1]`), and the bearer key is the `[server]` `api_key` itself. No credential is duplicated in `[workshop]`, so none can drift.

### The boot-only rule

Like `[server]`, the `[workshop]` section is owned by the boot config. A profile whose merged `[workshop]` differs from the boot file's is refused, and one-sided presence - the section in only one of the two files - is refused the same way. At boot the refusal fails startup; on a mid-run profile switch it reaches the caller as the switch stream's terminal SSE error event and leaves the running state untouched. The workshop's listener, tape, and voice settings are therefore fixed for the process lifetime.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
