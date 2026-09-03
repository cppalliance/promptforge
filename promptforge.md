# PromptForge Quickref

Load this file to write and run PromptForge prompts. The repo root is `c:\Users\Vinnie\src\cursor\promptforge`.

## Running a prompt

When the user says `promptforge <name> [input]`:

1. **Gateway.** Check terminals for a running `gateway`. If absent, start one in a background terminal from the repo root:
   ```
   cargo run -p gateway -- serve local/gateway.toml --profile main
   ```
   Wait for the line containing "serving" before proceeding.
2. **Run.** From the repo root, call the CLI against the prompt file under `prompts/`:
   ```
   cargo run -p promptforge-cli -- run prompts/<name>.md [input]
   ```
   Report whatever comes back. Do not explore the filesystem, pre-validate, or check whether the prompt exists.

Step 1 is one-time. Subsequent `promptforge <name> [input]` calls go straight to step 2.

**NEVER read, open, cat, or load any `.env` file into context.** These files contain secrets. The servers load them at startup - the agent must not.

Secrets live in name-matched `.env` files next to their `.toml` configs in `local/`:
- Gateway: `local/gateway.env` (`ANTHROPIC_API_KEY`, `BRAVE_API_KEY`, `PROMPTFORGE_GATEWAY_API_KEY`). The gateway loads at most two env files: the profile's (`local/profiles/main.env`) then the boot file's (`local/gateway.env`); the process environment wins over both, and included files' env files are never loaded.

## Components

| Component | Command | Env file | Default bind |
|---|---|---|---|
| Gateway | `cargo run -p gateway -- serve local/gateway.toml --profile main` | `local/profiles/main.env`, then `local/gateway.env` (process env wins) | `127.0.0.1:8081` |

## `local/gateway.toml` (gateway config)

```toml
[server]
bind = "127.0.0.1:8081"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[[dominion]]
id = "anthropic"
kind = "remote"
max_concurrency = 10

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"
dominion = "anthropic"

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
- `${VAR}` references resolve from the process environment first, then the profile's env file (`local/profiles/main.env`), then the boot file's env file (`local/gateway.env`); earlier sources win, and included files' env files are never loaded. The gateway supports `include` for config inheritance.
- Full schema: `design/design-gateway.md`

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
- Zero or more **H2** sections - executed top-to-bottom; children (H3+) never run by fall-through, only when addressed, and a jump to a child starts a child-level walk that resumes the parent after the jumper
- ` ```lua ` fences for executable Lua; ` ```lua shared ` for a shared library (replays as each section VM's first chunk with the full environment; `jump` excluded)
- Prose outside fences becomes model turns
- `---` as a section's first content marks it off-walk: skipped by the walk, runs only when addressed (`execute`/`jump`/`fanout`); content below the marker executes normally
- `---` anywhere else starts a reader-only comment region (no Lua, no model prose, no list items below it); after a prose line it needs a blank line before it
- Substitution: `{{ args }}`, `{{ var.x }}`, `{{ reply }}`, `{{ x }}` (a section-local bare global)
- Scalar `return` from Lua ends the run

## Lua API

| Function | Effect |
|---|---|
| `tools.bind(alias, desc, override?)` | Resolve a tool by description; `override` sets the model-facing description |
| `tools.always(alias, override?)` | Make a resolved tool available in every section |
| `tools.add(alias, override?)` | Make a resolved tool available in this section; `tools.add({"a", "b"})` for bulk |
| `tools.add_local(alias, desc, params, handler)` | Declare a Lua-backed tool (H2 only) |
| `models.default(alias, desc)` | Declare and set the prompt-wide baseline model |
| `models.bind(alias, desc, opts)` | Bind with options (thinking, context, temperature) |
| `models.use(alias)` | Select a declared model for this section; returns its handle |
| `models.get(alias)` | Return a declared model's handle without changing the section model |
| `models.infer(prompt)` | One tool-free inference round on the section's current model |
| `handle:infer(prompt)` | One tool-free inference round on the handle's model |
| `store.read(path[, start[, end]])` | Read file verbatim, optionally lines `start` to `end` (1-based, inclusive) |
| `store.read_numbered(path[, start[, end]])` | Read with absolute line numbers, optionally lines `start` to `end` (1-based, inclusive) |
| `store.write(path, content)` | Create or overwrite file |
| `store.append(path, content)` | Append to file |
| `store.str_replace(path, old, new)` | Edit by unique anchor |
| `store.delete(path)` | Remove file |
| `store.glob(pattern)` | List matching paths |
| `store.exists(path)` | Check existence |
| `untrusted(s)` | Wrap a string in the untrusted guard envelope |
| `return value` | End run, return value |
| `jump("## Section")` | Transfer control to a visible section (a sibling or a direct child); a child target starts a child-level walk |
| `execute("## Section", input?)` | Start a contained chain at a visible section (a sibling or a direct child); returns the chain's final reply |
| `fanout(worker, collection)` | Map a worker over a collection in parallel; array members arrive as `item`, hash members as pair tables |
| `list_from_section("## List")` | A list section's pre-parsed items as an array - feed it to `fanout` |
| `var.x = ...` | Walk-local state (JSON only); persists across sections on one walk, cloned into `execute`/`fanout` |
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
tools.bind("search", "Search the web and return results.")
tools.bind("fetch", "Fetch a web page as markdown.")
models.default("researcher", "A model suited for careful analysis")
```

## Research

```lua
tools.add({"search", "fetch"})
```

Research this person using live web tools. Return a factual summary.

{{ args }}
````

More examples: `prompts/`
