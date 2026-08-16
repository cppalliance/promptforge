# PromptForge Quickref

This file is loaded into an orchestration model's context so it understands how to write and run PromptForge prompts.

## 1. Gateway

- **What:** HTTP proxy routing model completions to upstream providers (Anthropic, OpenAI, local llama-server). Start first.
- Command: `cargo run -p promptforge-gateway -- serve gateway.toml`
- Env: `PROMPTFORGE_GATEWAY_KEY`, plus per-endpoint keys (e.g. `ANTHROPIC_API_KEY`)
- Default bind: `127.0.0.1:8081`

## 2. MCP Server

- **What:** Exposes prompts as MCP tools to an orchestrator (Cursor, Claude Code). Resolves names, runs prompts, streams progress.
- **Requires:** running gateway.
- Command: `cargo run -p promptforge-mcp-server -- serve [--stdio] <prompts.toml>`
- HTTP default: `127.0.0.1:9310/mcp`, bearer auth
- `--stdio`: JSON-RPC over stdin/stdout (for Cursor MCP config)
- Env: `PROMPTFORGE_MCP_TOKEN`, `PROMPTFORGE_GATEWAY_KEY`

## 3. Dev Runner

- **What:** Single-prompt edit-run loop for authoring. Runs one prompt, prints result, optionally watches.
- **Requires:** running gateway.
- Command: `cargo run -p promptforge-dev -- [--watch] [--capture-raw] <prompt.md> [input]`
- Env: `PROMPTFORGE_GATEWAY_URL`, `PROMPTFORGE_GATEWAY_KEY`
- `[input]` becomes `args` in the prompt; `--watch` reruns on save

## 4. `prompts.toml` (MCP server config)

Minimal:

```toml
[server]
token = "${PROMPTFORGE_MCP_TOKEN}"

[paths]
prompts = "prompts-mcp"

[gateway]
url = "http://127.0.0.1:8081/v1"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[tools]
web_fetch = true
web_search = true
```

- `[tools]` defaults to nothing enabled (true sandbox). Enable what the prompts need.
- `[catalog]` optional: `include`/`exclude` globs relative to `[paths].prompts`
- `[prompts.NAME]` optional: per-prompt overrides (`enabled = false`, `file = "path"`)
- Full schema: `crates/promptforge-mcp-server/design-mcp-server.md`

## 5. `gateway.toml` (gateway config)

Minimal remote:

```toml
[server]
bind = "127.0.0.1:8081"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"
concurrency = 10

[[model]]
name = "claude-sonnet-4-6"
description = "A model suited for careful analysis, coding, and general assistance"
context = 200000
thinking = "never"
upstream = "claude-sonnet-4-6"
endpoints = ["anthropic"]
```

- `[[local_model]]` for GGUF: `source`, `sha256`, `context`, `gpu_layers`, `flash_attention`
- `[tools.web_search]`: `provider = "brave"`, `api_key = "${BRAVE_API_KEY}"`
- Full schema: `crates/promptforge-gateway/design-gateway.md`

## 6. YAML Frontmatter

| Field | Required | Type | Meaning |
|---|---|---|---|
| `name` | yes | string | Identifier, `^[a-z][a-z0-9_]{0,47}$` |
| `description` | yes | string | One-line for listings and retrieval |
| `promptforge` | yes | int | Engine major version. Only `1` today |
| `default_return` | no | string | Returned on fall-through |
| `max_tool_iterations` | no | int 1-1000 | Tool-loop cap (default 24) |
| `input` | no | object | `path` + `description` of expected store input |
| `output` | no | object | `path` + `description` of produced store output |

Authoritative schema: `crates/promptforge-core/src/parser/build.rs`

## 7. Prompt Structure and Lua API

- Exactly one **H1** - title + H1 Lua (tool/model resolution)
- One or more **H2** sections - executed top-to-bottom (fall-through)
- ` ```lua ` fences for executable Lua; ` ```lua shared ` for shared library
- Prose becomes model turns; substitution: `{{ args }}`, `{{ var.x }}`, `{{ reply }}`
- Lua return (scalar) ends the run early

Key Lua API:
- `tools.need(alias, desc)` - resolve a tool by description
- `tools.add(alias...)` - make resolved tools available to the model
- `models.always(alias, desc)` / `models.need(alias, desc, opts)` / `models.use(alias)`
- `store.read(path)`, `store.write(path, content)`, `store.append(path, content)`
- `store.read_lines(path)`, `store.str_replace(path, old, new)`, `store.delete(path)`
- `store.glob(pattern)`, `store.exists(path)`, `store.inject(path)`
- `var.x = ...`, `args`, `reply`, `log(msg)`
- `return value`, `jump("## Section")`, `execute("## Section", input?)`

Full reference: `guide/src/prompt-files.md` (structure), `guide/src/lua.md` (Lua API)

## 8. Example

```markdown
---
name: research_person
description: Research a person from the open web
promptforge: 1
max_tool_iterations: 20
---

# Research a Person

```lua
tools.need("search", "Search the web and return results.")
tools.need("fetch", "Fetch a web page as markdown.")
models.always("researcher", "A model suited for careful analysis")
```

## Research

```lua
tools.add("search", "fetch")
```

Research this person using live web tools. Return a factual summary.

{{ args }}
```

More examples: `prompts/`

## 9. Local MCP Prompts

- `prompts-mcp/` - gitignored directory for local prompts served by the MCP server
- `prompts-mcp.toml` - config serving that directory (also gitignored)
- Drop any `.md` prompt file into `prompts-mcp/` and it's live (with `--watch`)
- Launch: `cargo run -p promptforge-mcp-server -- serve prompts-mcp.toml`
