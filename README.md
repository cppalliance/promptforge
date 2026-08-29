[![CI](https://github.com/cppalliance/promptforge/actions/workflows/ci.yml/badge.svg)](https://github.com/cppalliance/promptforge/actions/workflows/ci.yml)
[![Crates.io](https://img.shields.io/crates/v/promptforge-cli.svg)](https://crates.io/crates/promptforge-cli)
[![License](https://img.shields.io/badge/license-BSL--1.0-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/rust-1.89%2B-orange.svg)](https://www.rust-lang.org/)

# PromptForge

A runtime that executes AI prompt pipelines defined in a single markdown file. The markdown is the program, the model is the CPU. YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions, and a credential-holding gateway that keeps vendor keys off the prompt process. Write a prompt, run it, get a result.

![Workshop](images/banner-01.png)

## What you get

- 📄 **Markdown prompts** - frontmatter, one H1, H2 sections that run top to bottom
- 🔧 **Lua control** - bind tools and models, compute values, write the store, fan out work
- 🌐 **Tools that ship** - local `web_fetch`, gateway-backed `web_search`, semantic capability binding
- 🔌 **Inference gateway** - OpenAI-shaped chat, bearer auth, catalog at `GET /v1/models`
- 🛰️ **MCP server** - run prompts from an agentic harness over streamable HTTP or stdio

![Android heads](images/banner-02.png)

## Quick example

````markdown
---
name: greet
description: Greet the named input using a Lua-computed value
promptforge: 1
---

# Greet

```lua
models.default("writer", "A model suited for careful analysis, coding, and general assistance")
```

## Main

```lua
var.greeting = "Hello, " .. args .. "!"
```

Repeat exactly, with no extra words: {{ var.greeting }}
````

Prose goes to the model. Lua sets up the turn. The response is the run's result.

![Holographic code](images/banner-03.png)

## Quick start

```bash
cargo install promptforge-cli promptforge-gateway
promptforge-gateway serve gateway.toml --profile main &
promptforge run prompts/hello.md
```

Building from source:

```bash
git clone git@github.com:cppalliance/promptforge.git
cd promptforge
cargo build
```

The first build downloads the tool picker's embedding model (~130MB from Hugging Face, pinned and checksummed). Later builds reuse the cache.

Two processes: the gateway holds the vendor credential; the client points at it.

```bash
export ANTHROPIC_API_KEY=sk-ant-...
export PROMPTFORGE_GATEWAY_API_KEY=dev-secret
cargo run -p promptforge-gateway -- serve gateway.toml --profile main &

export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
cargo run -p promptforge-cli -- run prompts/hello.md
```

Interactive prompt work against an already-running gateway:

```bash
cargo run -p promptforge-dev -- prompts/greet.md "world" --watch
```

![Gloves and sparks](images/banner-04.png)

## How it works

Parse a promptforge markdown file, bind the tools and models it needs, then execute each H2 section in order. Section Lua prepares state; prose becomes a model turn (with a tool loop when tools are in scope); results land in the store or become the run output.

```mermaid
flowchart LR
  MD[Markdown prompt] --> Parse[Parse and bind]
  Parse --> Sec[H2 sections]
  Sec --> Lua[Lua blocks]
  Lua --> Model[Model turn]
  Model --> Tools[Tools via gateway or local]
  Model --> Store[Store artifacts]
  Store --> Out[Run result]
```

![Robot internals](images/banner-05.png)

## Crates

| Crate | Description | crates.io |
| --- | --- | --- |
| [promptforge-core](crates/promptforge-core) | Parser, executor, Lua runtime, store, gateway client | [![Crates.io](https://img.shields.io/crates/v/promptforge-core.svg)](https://crates.io/crates/promptforge-core) |
| [promptforge-cli](crates/promptforge-cli) | `promptforge run` command-line binary | [![Crates.io](https://img.shields.io/crates/v/promptforge-cli.svg)](https://crates.io/crates/promptforge-cli) |
| [promptforge-gateway](crates/promptforge-gateway) | Inference gateway with model catalog and credential isolation | [![Crates.io](https://img.shields.io/crates/v/promptforge-gateway.svg)](https://crates.io/crates/promptforge-gateway) |
| [promptforge-mcp-server](crates/promptforge-mcp-server) | MCP server for agentic harnesses (Cursor, Claude Code) | [![Crates.io](https://img.shields.io/crates/v/promptforge-mcp-server.svg)](https://crates.io/crates/promptforge-mcp-server) |
| [promptforge-tool-picker](crates/promptforge-tool-picker) | Semantic tool resolution via sentence embeddings | [![Crates.io](https://img.shields.io/crates/v/promptforge-tool-picker.svg)](https://crates.io/crates/promptforge-tool-picker) |
| [promptforge-tools](crates/promptforge-tools) | Runtime-agnostic tool contract: `Tool`, `ToolCatalog`, `ToolId` | [![Crates.io](https://img.shields.io/crates/v/promptforge-tools.svg)](https://crates.io/crates/promptforge-tools) |
| [promptforge-webfetch](crates/promptforge-webfetch) | SSRF-safe web fetch tool for model-supplied URLs | [![Crates.io](https://img.shields.io/crates/v/promptforge-webfetch.svg)](https://crates.io/crates/promptforge-webfetch) |
| [promptforge-web-search](crates/promptforge-web-search) | Web search tool proxying through the gateway with credential isolation | [![Crates.io](https://img.shields.io/crates/v/promptforge-web-search.svg)](https://crates.io/crates/promptforge-web-search) |
| [promptforge-dev](crates/promptforge-dev) | Interactive prompt development with watch mode | [![Crates.io](https://img.shields.io/crates/v/promptforge-dev.svg)](https://crates.io/crates/promptforge-dev) |
| [promptforge-transcribe](crates/promptforge-transcribe) | Whisper transcription engine: inference workers, segmentation, silence gating | not published |
| [promptforge-ws-server](crates/promptforge-ws-server) | Workshop HTTP server: chat relay, session tape, voice transcription | not published |
| [promptforge-desktop-shell](crates/promptforge-desktop-shell) | Workshop desktop shell: windowing, WebView, IPC, platform bridges (wry/tao) | not published |
| [promptforge-ws](crates/promptforge-ws) | Workshop desktop app: boots the gateway and opens the window | not published |

## Documentation

- [PromptForge User Guide](https://cppalliance.github.io/promptforge/) - full documentation
- [User Guide](guide/promptforge-user-guide.md) - progressive tutorial for writing prompts
- [design-core.md](design/design-core.md) - core design notes

![Filing cabinets](images/banner-06.png)

## Minimum Rust Version

Rust 1.89 or later.

## Contributing

Build, format, and test before you open a PR. CI runs `cargo fmt --check`, `clippy -D warnings`, and `cargo test --workspace`.

![Creator](images/promptforge-portrait.png)

## License

Distributed under the [Boost Software License 1.0](LICENSE).
