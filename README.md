# PromptForge

[![CI](https://github.com/cppalliance/promptforge/actions/workflows/ci.yml/badge.svg)](https://github.com/cppalliance/promptforge/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/promptforge-cli.svg)](https://crates.io/crates/promptforge-cli)
[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org/)

A runtime that executes AI prompt pipelines defined in a single markdown file. The markdown is the program, the model is the CPU. YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions, and a credential-holding gateway that keeps vendor keys off the prompt process. Write a prompt, run it, get a result.

## Crates

| Crate | Description | crates.io |
| --- | --- | --- |
| [promptforge-core](crates/promptforge-core) | Parser, executor, Lua runtime, store, gateway client | [![Crates.io](https://img.shields.io/crates/v/promptforge-core.svg)](https://crates.io/crates/promptforge-core) |
| [promptforge-cli](crates/promptforge-cli) | `promptforge run` command-line binary | [![Crates.io](https://img.shields.io/crates/v/promptforge-cli.svg)](https://crates.io/crates/promptforge-cli) |
| [promptforge-gateway](crates/promptforge-gateway) | Inference gateway with model catalog and credential isolation | [![Crates.io](https://img.shields.io/crates/v/promptforge-gateway.svg)](https://crates.io/crates/promptforge-gateway) |
| [promptforge-mcp-server](crates/promptforge-mcp-server) | MCP server for agentic harnesses (Cursor, Claude Code) | [![Crates.io](https://img.shields.io/crates/v/promptforge-mcp-server.svg)](https://crates.io/crates/promptforge-mcp-server) |
| [promptforge-tool-picker](crates/promptforge-tool-picker) | Semantic tool resolution via sentence embeddings | [![Crates.io](https://img.shields.io/crates/v/promptforge-tool-picker.svg)](https://crates.io/crates/promptforge-tool-picker) |
| [promptforge-webfetch](crates/promptforge-webfetch) | SSRF-safe web fetch tool for model-supplied URLs | [![Crates.io](https://img.shields.io/crates/v/promptforge-webfetch.svg)](https://crates.io/crates/promptforge-webfetch) |
| [promptforge-dev](crates/promptforge-dev) | Interactive prompt development with watch mode | [![Crates.io](https://img.shields.io/crates/v/promptforge-dev.svg)](https://crates.io/crates/promptforge-dev) |

## Quick Start

```bash
cargo install promptforge-cli promptforge-gateway
promptforge-gateway serve gateway.toml &
promptforge run prompts/hello.md
```

## Documentation

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## Contributing

Build, format, and test before you open a PR. CI runs `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace`. See [DEVELOPMENT.md](DEVELOPMENT.md) for details.

## License

Distributed under the [Boost Software License 1.0](LICENSE).
