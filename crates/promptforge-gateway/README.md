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

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
