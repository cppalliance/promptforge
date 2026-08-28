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

Configure endpoints, models, and credentials in the TOML catalog. The gateway accepts `POST /v1/chat/completions` and serves a model catalog at `GET /v1/models`.

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

Two feature flags exist:

- `workshop` - compiles the hosted workshop in: the `promptforge-ws-server` crate and system-browser opening.
- `workshop-cuda` - implies `workshop` and builds the whisper voice engine with CUDA acceleration.

The default feature set is empty, so a headless gateway build never pulls the workshop's toolchain into the graph: Node/esbuild (the workshop UI bundle) and whisper enter the gateway build only with `--features workshop`.

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

### Derived client credentials

There is no `[workshop.gateway]` sub-table. The hosted workshop reaches the gateway through its own HTTP client, and that client's `base_url` and `api_key` derive from the boot `[server]` section: the URL is the `[server]` bind with an unspecified address swapped for loopback (`0.0.0.0` becomes `127.0.0.1`, `[::]` becomes `[::1]`), and the bearer key is the `[server]` `api_key` itself. No credential is duplicated in `[workshop]`, so none can drift.

### The boot-only rule

Like `[server]`, the `[workshop]` section is owned by the boot config. A profile whose merged `[workshop]` differs from the boot file's is refused, and one-sided presence - the section in only one of the two files - is refused the same way. At boot the refusal fails startup; on a mid-run profile switch it reaches the caller as the switch stream's terminal SSE error event and leaves the running state untouched. The workshop's listener, tape, and voice settings are therefore fixed for the process lifetime.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
