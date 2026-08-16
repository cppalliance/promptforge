# promptforge-dev

[![Crates.io](https://img.shields.io/crates/v/promptforge-dev.svg)](https://crates.io/crates/promptforge-dev)
[![docs.rs](https://img.shields.io/docsrs/promptforge-dev)](https://docs.rs/promptforge-dev)
[![License](https://img.shields.io/crates/l/promptforge-dev)](LICENSE)

The edit-run-inspect loop for PromptForge prompts. Point it at a prompt file and it runs the prompt against your already-running gateway, dumps the store for inspection, and optionally watches for saves so every edit triggers a fresh run. No gateway management, no model downloads, no weight files - just the prompt and its output, tight enough that your iteration cycle is limited by how fast you can think.

## Installation

```bash
cargo install promptforge-dev
```

## Usage

```bash
promptforge-dev my-prompt.md "summarize this paragraph"
promptforge-dev --watch my-prompt.md
```

Requires a running `promptforge-gateway`. Set `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` before launching.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
