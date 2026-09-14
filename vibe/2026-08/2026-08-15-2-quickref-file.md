---
name: Promptforge quickref file
overview: Write a compressed LLM quickref into promptforge.md covering launch commands, config formats, YAML frontmatter, and prompt structure - minimal tokens, maximum utility for a fresh chat context.
todos:
  - id: tools-opt-in
    content: Make MCP server tools opt-in via [tools] config section instead of hardcoded
    status: completed
  - id: write-quickref
    content: Write the compressed quickref into promptforge/promptforge.md
    status: completed
isProject: false
---

# PromptForge Quickref

Write into `promptforge/promptforge.md`. The file's purpose: an LLM loads it at the start of a chat and immediately knows how to write and run PromptForge prompts. Compressed - no tutorial prose, no rationale, just the facts.

## Sections (mirrors the output file structure)

### 1. Gateway
- **What:** HTTP proxy that routes model completions to upstream providers (Anthropic, OpenAI, local llama-server). Every other component calls through it.
- **Start first.** The MCP server and dev runner both need a running gateway.
- Command: `cargo run -p promptforge-gateway -- serve [--profile NAME | config.toml]`
- Required env: `PROMPTFORGE_GATEWAY_KEY`, plus API keys per endpoint (e.g. `ANTHROPIC_API_KEY`)
- Listens on `127.0.0.1:8081` by default

### 2. MCP Server
- **What:** Exposes prompts as MCP tools to an orchestrator (Cursor, Claude Code). Resolves prompt names, runs them against the gateway, streams progress.
- **Requires:** a running gateway.
- Command: `cargo run -p promptforge-mcp-server -- serve [--stdio] <prompts.toml>`
- HTTP default: `127.0.0.1:9310/mcp`, bearer auth
- `--stdio`: JSON-RPC over stdin/stdout (for Cursor MCP config)
- Required env: `PROMPTFORGE_MCP_TOKEN`, `PROMPTFORGE_GATEWAY_KEY`

### 3. Dev Runner
- **What:** Single-prompt edit-run loop for authoring. Runs one prompt file, prints the result, optionally watches for changes.
- **Requires:** a running gateway.
- Command: `cargo run -p promptforge-dev -- [--watch] [--capture-raw] <prompt.md> [input]`
- Required env: `PROMPTFORGE_GATEWAY_URL`, `PROMPTFORGE_GATEWAY_KEY`
- `[input]` becomes `args` in the prompt
- `--watch` reruns on save

### 4. `prompts.toml` (MCP server config)
- Minimal example showing `[server]`, `[gateway]`, `[paths]`, `[catalog]`
- Key fields only
- Reference: `See crates/promptforge-mcp-server/design-mcp-server.md for full schema.`

### 5. `gateway.toml` (gateway config)
- Minimal example showing `[server]`, `[[endpoint]]`, `[[model]]`
- One remote endpoint, one model
- Reference: `See crates/promptforge-gateway/design-gateway.md for full schema.`

### 6. YAML Frontmatter
- One-line summary of each field (name, description, promptforge, default_return, max_tool_iterations, input, output)
- Reference: `See crates/promptforge-core/src/parser/build.rs for the authoritative schema.`

### 7. Prompt Structure and Lua API
- Brief summary (H1, H2 sections, lua fences, prose, substitution)
- Reference: `See guide/src/prompt-files.md for structure, guide/src/lua.md for the Lua API.`

### 8. One Complete Example
- A prompt with tools.need, models.always, store, and substitution - showing the full pattern in ~15 lines
- Reference: `See prompts/ for more examples.`

### 9. Local MCP prompts directory

- `prompts-mcp/` - gitignored directory for local prompts served by the MCP server
- `prompts-mcp.toml` - the TOML config that serves this directory (gitignored too)
- Move `briefer.md` into it (it becomes a local MCP prompt)
- The quickref's `prompts.toml` example references this setup

## Code change: make MCP server tools opt-in

Currently `live_tools()` in `crates/promptforge-mcp-server/src/server/bind.rs` hardcodes WebFetch and WebSearch. Change to:

- Default to **no tools** (empty vec) - true sandbox
- Add an optional `[tools]` section to `prompts.toml` config that enables them:

```toml
[tools]
web_fetch = true
web_search = true
```

Implementation:
- Add `ToolsConfig` struct to `config.rs` with `web_fetch: bool` and `web_search: bool`, both defaulting to `false`
- Add `pub(crate) tools: ToolsConfig` to `Config`
- Change `live_tools()` to read from config and only register what's enabled
- Update the existing `prompts.toml` in the repo to enable both (preserving current behavior for existing users)

This means `prompts-mcp.toml` can omit `[tools]` entirely for a pure sandbox, or explicitly enable what it needs.

## Additional file changes

- `.gitignore` - add `/prompts-mcp/` and `/prompts-mcp.toml`
- Create `prompts-mcp.toml`:

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

Per-prompt overrides still work via `[prompts.NAME]` blocks (e.g. `enabled = false`).

- Move `briefer.md` to `prompts-mcp/briefer.md`

## Execution References

Load these before building:
- `c:\Users\Vinnie\src\cursor\tools-public\rulebooks\rust-rulebook.md` - Rust coding standards
- `c:\Users\Vinnie\src\cursor\tools-public\rulebooks\vibe-rulebook.md` - Work loop: per-step commits, coder/review/verify subagents

## Constraints

- Target: under 300 lines total
- No prose paragraphs - bullet lists, tables, and code blocks only
- No rationale or "why" - just "what" and "how"
- One example per concept, not three
