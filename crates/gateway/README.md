# gateway

[![Crates.io](https://img.shields.io/crates/v/gateway.svg)](https://crates.io/crates/gateway)
[![docs.rs](https://img.shields.io/docsrs/gateway)](https://docs.rs/gateway)
[![License](https://img.shields.io/crates/l/gateway)](LICENSE)

The credential-holding inference gateway for PromptForge. It serves an OpenAI-compatible HTTP API that routes chat completions to configured backends, holds every credential, manages a model catalog, runs a built-in web search tool, and optionally spawns local `llama-server` processes for GGUF models. Nothing above it holds a vendor key. A key rotation touches one file on one host.

## Installation

```bash
cargo install gateway
```

## Usage

```bash
promptforge-gateway serve gateway.toml --profile main
```

The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable (the CLI argument wins). With neither set, the gateway searches beside the executable, then the working directory, then the user profile's `.promptforge` directory; when no `gateway.toml` exists, first run writes a default there - loopback on an OS-assigned port, a fresh random bearer key, `trust_loopback = true` so same-machine callers need no key (with the shared-machine caveat and the `trust_loopback = false` opt-out noted in the file), the recommended STT pair unless the installer declined it - and boots from it. The profile comes from `--profile NAME`, the `PROMPTFORGE_PROFILE` environment variable, or the sibling state file, in that precedence; with none set, startup refuses and lists the profiles the config defines. The generated default writes its state file selecting `default`, so a bare first boot needs no flags.

Configure endpoints, models, and credentials in the TOML catalog. The gateway accepts `POST /v1/chat/completions`, serves a model catalog at `GET /v1/models`, and, with the default-on `stt` feature, serves streaming dictation at `/stt`, capability discovery at `GET /stt/capability`, and OpenAI-compatible multipart transcription at `POST /v1/audio/transcriptions`.

Embedding hosts use the library API instead of the binary: `spawn` starts the gateway on a dedicated thread with its own runtime and blocks until the listener is bound, returning a `GatewayHandle` that carries the bound URL and a graceful-shutdown switch (`url()`, `shutdown()`, `join()`).

## System tray

On Windows the binary's default main loop is the system tray: a hidden-window win32 message loop owns the main thread while serving stays on the gateway thread. The menu carries a disabled status line on top (gateway state plus served models and declared VRAM, refreshed on a timer from in-process state), then **Workshop** (launches `promptforge-workshop.exe` when the installer laid it beside the gateway; disabled on a Gateway-only install), **Settings** (opens the config SPA in the browser through the one-time `/auth?key=` handoff, as does double-clicking the icon), **Launch at Login** (a check item whose state is the HKCU Run key entry `PromptForgeGateway`, never local config), and **Quit** last, which fires the in-process shutdown signal directly. `--no-tray` keeps the headless Ctrl-C loop for servers and CI; the autostart entry's `"<exe>" serve --login` command line marks login launches, which never open a browser. `--browser` opens the Settings page in the default browser once the listener is bound - the installer's first run uses it. On macOS the NSApplication run loop owns the main thread, the icon is a template glyph, and Launch at Login registers through `SMAppService` when the gateway is its bundle's principal executable. On Linux the tray is a pure StatusNotifierItem over the session D-Bus (ksni; no GTK, no libappindicator): icon clicks carry no events there, so the menu is the only path, and Launch at Login writes `~/.config/autostart/promptforge-gateway.desktop` (a user-deleted entry is never resurrected). A desktop with no StatusNotifierWatcher - stock GNOME without the AppIndicator extension - keeps serving trayless, posts one first-run notification naming the Settings URL, and registers the tray automatically when a watcher appears. Two CLI affordances serve tray-less environments: `--print-url` prints the Settings handoff URL to stdout once bound and then serves headless, and a second `promptforge-gateway` launch while one is running never boots a duplicate - it hands off before any bind attempt, opening the running gateway's Settings page (printing its URL under `--print-url`, exiting quietly under `--login`). Platforms without a backend fall back to the headless loop with a warning.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Profiles

One `gateway.toml` holds the entire catalog, opened by `config-version = 2`: global sections once (`[server]`, `[local]`, `[tools]`, `[[endpoint]]`, `[[dominion]]`), remote models as `[[model]]`, local models as `[[local_model]]`, and speech-to-text models as `[[stt_model]]`. Profiles are named checklists over that catalog:

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

Switching runs from the already-loaded catalog through `POST /admin/switch-profile`, streaming its stages (`loading-profile`, `downloading-models` when the profile names local models, `stopping-models` when there are old children to stop, `starting-models`, one terminal event) over SSE. Inference is never blocked by a download or a spawn: the new local models' weights stage into the cache while the old children keep serving (or, on a cold boot or from a remote-only profile, after the new profile's remote models are already published), then in-flight inference requests get a bounded drain - up to 30 seconds - before stragglers are cancelled and the old children stop; the new subset then spawns, and the commit swaps the full routing table in and persists the choice to the state file. While a local model is downloading or spawning after the cut-over, a request for it gets `503` with code `model_loading` and `Retry-After: 5`, `GET /admin/status` lists it under `loading_models`, and `GET /v1/models` lists only routable models. The Config UI stages `active_profile` as pending state like any other edit, so the switch lands atomically on Apply.

Switches, boot provisioning, and model unloads run as commands on a serialized, debounced queue with per-command cancellation. `GET /admin/status` reports the queue (the active command's name, progress fraction, and start time, plus the pending commands) and one readiness entry per capability endpoint (`ready` when a loaded model serves it, `provisioning` when a command is loading its configured models). `POST /admin/queue/cancel` fires the active command's cancellation token; `POST /admin/queue/cancel-pending` drops a waiting command by index.

The pre-2 layout is a hard break with no compat loader: a config carrying an `include` key, a top-level `models` allowlist, or the old `[workshop.voice]` model keys, a missing or wrong `config-version`, or a sibling `profiles/` directory fails to load with an error naming the file, key, and line, plus the replacement to use.

## The `[server]` section

The boot config's `[server]` section is required; `bind` and `api_key` have no defaults and accept `${VAR}` interpolation from the process environment.

| Field | Default | Meaning |
|---|---|---|
| `bind` | required | Socket address the gateway listener binds. |
| `api_key` | required | Shared bearer key. Every request from a non-loopback peer must present it, and a presented key is always checked. |
| `trust_loopback` | `true` | Admit a loopback peer that presents no credential at all, on every route including the admin surface. On a shared machine this lets any other OS account on the same host use the gateway, including reading upstream API keys from the admin config surface; set `trust_loopback = false` (or bind off loopback) to require the bearer key from every caller. |

Loopback trust is deliberately narrow. It applies only when no `Authorization` header was sent - a presented-but-wrong bearer is still 401, even from loopback - and only when the request's fetch metadata allows ambient access: no `Sec-Fetch-Site` header (curl, the SDK, the workshop, any non-browser client) or `same-origin`/`none` (the config SPA on its own origin, a typed URL). A browser page on another origin sends `cross-site` and is refused, so loopback trust does not reopen the CSRF hole the bearer requirement closed; the Host allowlist below still closes DNS rebinding. A request without a peer address fails closed and needs the key. The SDK's `GatewayClient::from_env` reads the same rule: `PROMPTFORGE_GATEWAY_API_KEY` is optional when `PROMPTFORGE_GATEWAY_URL` is loopback, required otherwise.

The section is process-owned: a pending edit that changes it is promoted to disk on Apply but takes effect only on restart, reported as `restart_required` in the apply response.

## The `[local]` section

| Field | Default | Meaning |
|---|---|---|
| `cache_dir` | `~/.promptforge` | Root directory for GGUF files and the pinned `llama-server` installs. |
| `llama_backend` | `"auto"` | Which `llama-server` build to download on Windows x86-64: `auto` picks from the host's GPUs (Blackwell gets the PromptForge CUDA build, any other NVIDIA GPU the upstream CUDA 13 build, anything else Vulkan); `cuda-blackwell`, `cuda`, and `vulkan` force the row. Consulted only on Windows x86-64. |
| `llama_server_path` | none | Explicit `llama-server` executable path, skipping the managed download entirely. |

The `llama-server` executable resolves in a fixed order: `llama_server_path` from the config, then the `PROMPTFORGE_LLAMA_SERVER` environment variable, then the managed download under the cache directory. Both CUDA builds ship their runtime DLLs, so the host needs only the NVIDIA driver. When a download fails and an older install is already in the cache, the gateway uses the cached one and logs a warning rather than failing to start.

## Feature flags

Four feature flags exist:

- `local` (default) - compiles in gateway-owned local inference via the `gateway-local` crate: GGUF provisioning, managed `llama-server` children, the blob cache behind the `/v1/cache` routes, the `GET /admin/orphans` listing of cache files no loaded `[[local_model]]` entry references (sizes from the filesystem, digests only from cache sidecars - multi-gigabyte blobs are never re-hashed), the `GET /admin/model-info?path=` GGUF-header readout of a cache file's architecture, layer count, and parameter count (the `path` must stay inside the artifact cache; only the header is read, never tensor data), and the bearer-authenticated `GET /admin/chat-templates` catalog used by the Config UI. A `--no-default-features` build is headless of local inference: it links neither the archive/extraction stack nor a blocking HTTP client, and it refuses a configuration declaring `[[local_model]]` at startup and on profile switch.
- `web-search` (default) - compiles in the Brave-powered `POST /v1/tools/web_search` tool service via the `gateway-web-search` crate. A `--no-default-features` build omits the route entirely.
- `stt` (default) - compiles in gateway-owned speech-to-text via the `gateway-stt` crate: the transcription engine lifecycle, streaming `/stt` routes, and `POST /v1/audio/transcriptions` on the gateway listener. A `--no-default-features` build omits the routes and refuses a configuration declaring `[[stt_model]]` at startup and on profile switch.
- `config-ui` (default) - compiles in the embedded config SPA via the `gateway-config-ui` crate and serves it at `/config/` on the gateway's own port (no second listener); `GET /config` redirects to `/config/`. The routes are loopback-only and carry no bearer auth (the SPA shell holds no secrets); Node/esbuild and `rust-embed` enter the build only with this feature: Node 22 is needed on the build machine for the UI bundle's esbuild step, not for Rust itself, and a `--no-default-features` build needs no Node at all. With the feature, `GET /auth?key=` is the browser handoff onto the surface: it validates the bearer key, sets a session proof derived from it (SHA-256 over a process-lifetime salt and the key, so the cookie never carries the key and a restart or key rotation revokes it) as an HttpOnly `SameSite=Lax` session cookie, and 302-redirects to the key-free `/config/`, which accepts the cookie in place of the `Authorization` header - a tray or shell can open the UI without leaving the key in browser history. Because the cookie is ambient, the cookie path also requires `Sec-Fetch-Site: same-origin` or `none` fetch metadata, which browsers attach and a cross-origin page cannot strip. Regardless of the feature, the admin config endpoints (config read/write, env, pending state, apply/revert, orphans, system, model-info, chat templates, the HF proxy, profile create/delete, reveal) plus `POST /shutdown` and `GET /auth` sit behind the shared loopback wall from the always-on `shared-loopback` crate: a non-loopback peer gets 403 before bearer auth even runs. `POST /shutdown` is the bearer-authed graceful stop - the same drain Ctrl-C drives - answering 202 before the server goes down; the tray's Quit and the shell's Quit-everything call it. And whenever the listener is bound to a loopback address, every route sits behind the wall's second middleware, a host-authority allowlist that refuses with 403 any request whose `Host` is not the bound socket (`127.0.0.1:port`, `[::1]:port`, or `localhost:port`), closing DNS rebinding; a non-loopback bind enforces no allowlist.

The speech runtime itself is a pinned managed download selected for the host at run time. Note the build graph: the default-on `stt` feature's `gateway-stt` crate depends on `workshop-server` (the `/stt` socket attach API), whose build script bundles the workshop UI with esbuild - so default builds need Node 22 even though the gateway serves no workshop pages, and only a `--no-default-features` build drops that requirement. The gateway hosts no workshop UI: the desktop shell embeds the workshop server itself, and a boot config carrying a `[workshop]` section still parses but earns a deprecation warning at startup - its `bind` and `open_browser` settings are inert, while `[workshop.stt]` capture tuning still applies to the STT engine.

### Speech-to-text models

Speech-to-text models are first-class catalog entries, provisioned through the same pinned, digest-verified cache machinery as local chat models and governed by profile membership: transcription is on when the active profile's list contains STT models, off otherwise. A `--no-default-features` build (no `stt` feature) refuses an active profile that selects STT models, same as it refuses `[[local_model]]`.

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

An unspecified `[server]` bind IP becomes the matching loopback address in the derived client URL (`0.0.0.0` becomes `127.0.0.1`, `[::]` becomes `[::1]`), so a same-host consumer always gets a dialable URL.

### Process-owned sections

`[server]` is process-owned. Apply promotes edits to it to disk and answers `restart_required: true`; the running process keeps its booted listener and STT capture settings until restart. A profile switch never changes it, because profiles are checklists over the model catalog and carry no sections.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
