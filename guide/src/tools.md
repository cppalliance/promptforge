# Tools

Tools are declared by capability description and resolved semantically at runtime via a picker. You control exactly what the model sees - declaring a tool does not automatically expose it.

## Declaring Tools

```lua
-- Declare a tool need
local search = tools.need("search", "web search capability")

-- Promote to prompt-wide availability (available in all sections)
tools.always("search")
```

A tool declared with `tools.need` is not exposed to the model unless `tools.always` or `tools.add` is called.

```lua
-- Section-local scoping
tools.add("search")            -- by alias string
tools.add(search)              -- by handle object
tools.add({"a", "b", tool_c}) -- arrays of strings or handles
```

`tools.add` calls are atomic: a failure rolls back all entries. An empty add is a no-op.

## Tool Properties

After `tools.need`, the returned handle exposes: `name`, `description`, `parameters` (JSON schema), `wire_name`, and `untrusted` flag. The model-facing description can be overridden:

```lua
local search = tools.need("search", "web search capability")
search.description = "Search the web for current information"
tools.add(search)
```

## Tool Dispatch Loop

The tool loop runs the model in a cycle: dispatch tool calls, feed results back, re-prompt until the model produces a final text reply or the iteration cap is reached (default 24 rounds, configurable via `max_tool_iterations` in frontmatter).

## Tool Safety

Untrusted tool output is wrapped with a CSPRNG nonce envelope before reaching the model, preventing prompt injection. Each round uses a fresh nonce. Trusted tool output passes verbatim. Trust marking is mandatory at construction time.

Near-duplicate tools available to the same section are detected and rejected before any model call, with similarity diagnostics. Tool calls for tools not available to the section produce a clear error distinguishing globally-declared-but-unavailable tools from truly unknown ones.

## Tool Call Counts

Per-alias call counts are tracked during execution. Read them from Lua to measure or assert model behavior:

```lua
tools.add("search")
```

After the prose block runs with the tool loop:

```lua
if tools.calls.search == 0 then
    log("model never searched")
end
```

Counts increment even when a tool call fails. Mistyped aliases produce a hard error with the available tools listed.

## Implementing Custom Tools

A custom tool requires:

- A stable `ToolId` (server + name pair)
- A wire name matching `[A-Za-z0-9_.-]`
- A description string
- A JSON-Schema parameters definition
- An async `call` method returning `ToolOutput` (marked trusted or untrusted)

Tools can run locally in-process or proxy through a remote gateway, both dispatched uniformly through the `Tool` trait:

```rust
use promptforge_core::{Tool, ToolId, ToolOutput};

#[async_trait]
impl Tool for MyTool {
    fn id(&self) -> &ToolId;
    fn wire_name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> &serde_json::Value;
    async fn call(&self, arguments: &str) -> Result<ToolOutput, ToolError>;
}
```

## Built-in Web Search

The web search tool sends queries through a gateway proxy so the search provider credential never leaves the server. Results are automatically marked as untrusted output.

### Parameters

- `count` - number of results (1-20)
- `freshness` - time filter: `pd` (past day), `pw` (past week), `pm` (past month), `py` (past year)
- `safe_search` - level: `off`, `moderate`, `strict`
- `domains_include` - allowlist (up to 20 domains)
- `domains_exclude` - blocklist (up to 20 domains)
- `country` - country code
- `language` - language code
