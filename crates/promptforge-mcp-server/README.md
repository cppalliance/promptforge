# promptforge-mcp-server

[![Crates.io](https://img.shields.io/crates/v/promptforge-mcp-server.svg)](https://crates.io/crates/promptforge-mcp-server)
[![docs.rs](https://img.shields.io/docsrs/promptforge-mcp-server)](https://docs.rs/promptforge-mcp-server)
[![License](https://img.shields.io/crates/l/promptforge-mcp-server)](LICENSE)

An MCP server that runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. It puts a prompt catalog behind four fixed MCP tools rather than publishing each prompt as its own tool, which means `tools/list` never changes and a prompt saved ten seconds ago is callable with no reconnect. Serves over streamable HTTP with bearer auth, or over stdio for a local spawn.

## Installation

```bash
cargo install promptforge-mcp-server
```

## Usage

```bash
promptforge-mcp-server serve prompts.toml
promptforge-mcp-server serve --stdio prompts.toml
```

Configure your prompt catalog, gateway connection, and server settings in a single `prompts.toml` file.

See the [PromptForge User Guide](https://cppalliance.github.io/promptforge/) for full documentation.

## Minimum Rust Version

Rust 1.89 or later.

## License

Licensed under the [Boost Software License 1.0](LICENSE).
