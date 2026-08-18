# PromptForge Quickref

Load this file to write and run PromptForge prompts. The repo root is `c:\Users\Vinnie\src\cursor\promptforge`.

## Running a prompt

When the user says `promptforge <name> [input]`:

1. **Gateway.** Check terminals for a running `promptforge-gateway`. If absent, start one in a background terminal from the repo root:
   ```
   cargo run -p promptforge-gateway -- serve local/gateway.toml
   ```
   Wait for the line containing "serving" before proceeding.
2. **MCP server.** Call `GetMcpTools(server: "user-promptforge")`. If status is not `ready`, check terminals for a running `promptforge-mcp-server`. If absent, start one in a background terminal from the repo root:
   ```
   cargo run -p promptforge-mcp-server -- serve local/mcp-service.toml
   ```
   Wait for the line containing "serving", then re-check `GetMcpTools` until status is `ready`.
3. **Discovery (once per session).** Call `GetMcpTools(server: "user-promptforge")` to learn the tool schemas. Then call `list_prompts` to get prompt names and their input/output metadata. Both stay in context for the session. Only re-call `list_prompts` if the user says something changed.
4. **Call.** Call `run_prompt` with the name from the user's command. Map the user's input to `run_prompt` parameters based on what `list_prompts` reported about the prompt. Do not explore the filesystem, pre-validate, or check whether the prompt exists. Call it. Report whatever comes back.

Steps 1-3 are one-time. Subsequent `promptforge <name> [input]` calls go straight to step 4.

**NEVER read, open, cat, or load any `.env` file into context.** These files contain secrets. The servers load them at startup - the agent must not.

Secrets live in name-matched `.env` files next to their `.toml` configs in `local/`:
- Gateway: `local/gateway.env` (`ANTHROPIC_API_KEY`, `BRAVE_API_KEY`, `PROMPTFORGE_GATEWAY_API_KEY`). The gateway loads env files hierarchically through its `include` chain - root values take precedence.
- MCP server: `local/mcp-service.env` (`PROMPTFORGE_MCP_SERVER_API_KEY`, `PROMPTFORGE_GATEWAY_API_KEY`). Flat - one toml, one env file, no chain.

## Components

| Component | Command | Env file | Default bind |
|---|---|---|---|
| Gateway | `cargo run -p promptforge-gateway -- serve local/gateway.toml` | `local/gateway.env` (hierarchical, follows include chain) | `127.0.0.1:8081` |
| MCP Server | `cargo run -p promptforge-mcp-server -- serve local/mcp-service.toml` | `local/mcp-service.env` (flat, one file) | `127.0.0.1:9310` |

Start the gateway first. The MCP server calls through it.

## `local/mcp-service.toml` (MCP server config)

The MCP server config lives at `local/mcp-service.toml` (gitignored). It serves the directory named in `[paths].prompts`.

- `[catalog]` required: `include` globs relative to `[paths].prompts`. Without it the server finds zero prompts.
- `[tools]` defaults to nothing enabled (true sandbox). Enable per prompt needs.
- `[prompts.NAME]` optional: per-prompt overrides (`enabled = false`).
- Full schema: `crates/promptforge-mcp-server/design-mcp-server.md`

## `local/gateway.toml` (gateway config)

```toml
[server]
bind = "127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

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
- `${VAR}` references resolve from a name-matched env file (`local/gateway.env` for `local/gateway.toml`), then from the process environment. The gateway supports `include` for config inheritance - when configs use it, the full include chain is walked and all found env files are loaded, root values taking precedence.
- Full schema: `crates/promptforge-gateway/design-gateway.md`

## YAML Frontmatter

| Field | Required | Type | Meaning |
|---|---|---|---|
| `name` | yes | string | Identifier, `^[a-z][a-z0-9_]{0,47}$` |
| `description` | yes | string | One-line for listings and retrieval |
| `promptforge` | yes | int | Engine major. Only `1` runs today |
| `max_tool_iterations` | no | int 1-1000 | Tool-loop cap (default 24) |
| `input` | no | object | `path` + `description` of expected store input |
| `output` | no | object | `path` + `description` of produced store output |

Authoritative schema: `crates/promptforge-core/src/parser/build.rs`

## Prompt Structure

- Exactly one **H1** - title + H1 Lua (tool/model resolution)
- Zero or more **H2** sections - executed top-to-bottom
- ` ```lua ` fences for executable Lua; ` ```lua shared ` for shared library
- Prose outside fences becomes model turns
- Substitution: `{{ args }}`, `{{ var.x }}`, `{{ reply }}`
- Scalar `return` from Lua ends the run

## Lua API

| Function | Effect |
|---|---|
| `tools.need(alias, desc)` | Resolve a tool by description |
| `tools.add(alias...)` | Make resolved tools available to the model |
| `tools.add_local(alias, desc, params, handler)` | Declare a Lua-backed tool (H2 only) |
| `models.default(alias, desc)` | Declare and set the prompt-wide baseline model |
| `models.need(alias, desc, opts)` | Bind with options (thinking, context, temperature) |
| `models.use(alias)` | Select a declared model for this section; returns its handle |
| `models.get(alias)` | Return a declared model's handle without changing the section model |
| `models.infer(prompt)` | One tool-free inference round on the section's current model |
| `store.read(path[, start[, end]])` | Read file verbatim, optionally lines `start` to `end` (1-based, inclusive) |
| `store.read_numbered(path[, start[, end]])` | Read with absolute line numbers, optionally lines `start` to `end` (1-based, inclusive) |
| `store.write(path, content)` | Create or overwrite file |
| `store.append(path, content)` | Append to file |
| `store.read_lines(path)` | Read with line numbers |
| `store.str_replace(path, old, new)` | Edit by unique anchor |
| `store.delete(path)` | Remove file |
| `store.glob(pattern)` | List matching paths |
| `store.exists(path)` | Check existence |
| `store.inject(path)` | Read wrapped in untrusted envelope |
| `untrusted(s)` | Wrap a string in the untrusted guard envelope |
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
models.default("researcher", "A model suited for careful analysis")
```

## Research

```lua
tools.add("search", "fetch")
```

Research this person using live web tools. Return a factual summary.

{{ args }}
````

More examples: `prompts/`
