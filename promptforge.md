# PromptForge Quickref

Load this file to write and run PromptForge prompts. The repo root is `c:\Users\Vinnie\src\cursor\promptforge`.

## Running a prompt

When the user says `promptforge <name> [input]`:

1. Check terminals for a running `promptforge-gateway`. If absent, start one in a background terminal from the repo root:
   ```
   cargo run -p promptforge-gateway -- serve gateway.toml
   ```
   Wait for the line containing "serving" before proceeding.
2. Check that the `user-promptforge` MCP server is connected (`GetMcpTools` server status). If it is in error state, check terminals for a running `promptforge-mcp-server`. If absent, start one in a background terminal from the repo root:
   ```
   cargo run -p promptforge-mcp-server -- serve local/prompts.toml
   ```
   Wait for the line containing "listening" before proceeding.
3. Run the prompt as an MCP tool call:
   ```
   CallMcpTool(server: "user-promptforge", toolName: "<name>", arguments: { "input": "<input>" })
   ```

The gateway and MCP server persist for the session. Skip steps 1-2 on subsequent runs.

Secrets (`ANTHROPIC_API_KEY`, `BRAVE_API_KEY`, etc.) go in `gateway.env` next to `gateway.toml`. The gateway loads it automatically before resolving `${VAR}` references in the config. The MCP server needs `PROMPTFORGE_MCP_TOKEN` and `PROMPTFORGE_GATEWAY_KEY` in its environment.

**NEVER read, open, cat, or load any `.env` file into context.** These files contain secrets. The gateway reads them at startup - the agent must not.

Prompts live in `local/prompts/`. Drop a `.md` file there and it becomes an MCP tool (with `--watch`, live).

## Components

| Component | Command | Secrets | Default bind |
|---|---|---|---|
| Gateway | `cargo run -p promptforge-gateway -- serve gateway.toml` | `gateway.env` (loaded automatically) | `127.0.0.1:8081` |
| MCP Server | `cargo run -p promptforge-mcp-server -- serve local/prompts.toml` | `PROMPTFORGE_MCP_TOKEN`, `PROMPTFORGE_GATEWAY_KEY` | `127.0.0.1:9310` |

Start the gateway first. The MCP server calls through it.

## `local/prompts.toml` (MCP server config)

The MCP server config lives at `local/prompts.toml` (gitignored). It serves `local/prompts/`.

- `[catalog]` required: `include` globs relative to `[paths].prompts`. Without it the server finds zero prompts.
- `[tools]` defaults to nothing enabled (true sandbox). Enable per prompt needs.
- `[prompts.NAME]` optional: per-prompt overrides (`enabled = false`).
- Full schema: `crates/promptforge-mcp-server/design-mcp-server.md`

## `gateway.toml` (gateway config)

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

- `[[local_model]]` for GGUF: `source`, `sha256`, `context`, `gpu_layers`, `flash_attention`.
- `[tools.web_search]`: `provider = "brave"`, `api_key = "${BRAVE_API_KEY}"`.
- `${VAR}` references resolve from a name-matched env file (`gateway.env` for `gateway.toml`), then from the process environment. When configs use `include`, the full include chain is walked and all found env files are loaded - root values take precedence, included configs supply defaults.
- Full schema: `crates/promptforge-gateway/design-gateway.md`

## YAML Frontmatter

| Field | Required | Type | Meaning |
|---|---|---|---|
| `name` | yes | string | Identifier, `^[a-z][a-z0-9_]{0,47}$` |
| `description` | yes | string | One-line for listings and retrieval |
| `promptforge` | yes | int | Engine major. Only `1` runs today |
| `default_return` | no | string | Returned on fall-through |
| `max_tool_iterations` | no | int 1-1000 | Tool-loop cap (default 24) |
| `input` | no | object | `path` + `description` of expected store input |
| `output` | no | object | `path` + `description` of produced store output |

Authoritative schema: `crates/promptforge-core/src/parser/build.rs`

## Prompt Structure

- Exactly one **H1** - title + H1 Lua (tool/model resolution)
- One or more **H2** sections - executed top-to-bottom
- ` ```lua ` fences for executable Lua; ` ```lua shared ` for shared library
- Prose outside fences becomes model turns
- Substitution: `{{ args }}`, `{{ var.x }}`, `{{ reply }}`
- Scalar `return` from Lua ends the run

## Lua API

| Function | Effect |
|---|---|
| `tools.need(alias, desc)` | Resolve a tool by description |
| `tools.add(alias...)` | Make resolved tools available to the model |
| `models.always(alias, desc)` | Bind a model for this section |
| `models.need(alias, desc, opts)` | Bind with options (thinking, context, temperature) |
| `models.use(alias)` | Switch to a previously bound model |
| `store.read(path)` | Read file verbatim |
| `store.write(path, content)` | Create or overwrite file |
| `store.append(path, content)` | Append to file |
| `store.read_lines(path)` | Read with line numbers |
| `store.str_replace(path, old, new)` | Edit by unique anchor |
| `store.delete(path)` | Remove file |
| `store.glob(pattern)` | List matching paths |
| `store.exists(path)` | Check existence |
| `store.inject(path)` | Read wrapped in untrusted envelope |
| `return value` | End run, return value |
| `jump("## Section")` | Transfer control |
| `execute("## Section", input?)` | Run section as subroutine |
| `var.x = ...` | Cross-section state |
| `log(msg)` | Emit to observer |

Full reference: `guide/src/prompt-files.md` (structure), `guide/src/lua.md` (API)

## Example

````markdown
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
````

More examples: `prompts/`
