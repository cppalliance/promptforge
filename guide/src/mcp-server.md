# MCP Server

promptforge-mcp-server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. It puts a prompt catalog behind four fixed MCP tools rather than publishing each prompt as its own tool, which means `tools/list` never changes and a prompt saved ten seconds ago is callable with no reconnect. You point it at a `prompts.toml`, it resolves your prompts, connects to a gateway, and serves - over HTTP with bearer auth, or over stdio for a local spawn.

## Starting the Server

Bind the streamable-HTTP transport:

```bash
promptforge-mcp-server serve prompts.toml
```

This serves at `http://127.0.0.1:9310/mcp`. Every request to `/mcp` must carry an `Authorization: Bearer <token>` header matching `[server].token`.

For a harness that spawns the server as a child process:

```bash
promptforge-mcp-server serve --stdio prompts.toml
```

Stdio speaks JSON-RPC over standard input and output, binds no port, and ignores `[server].token` entirely. Logs go to stderr so they do not corrupt the wire.

## Configuration

A single `prompts.toml` carries everything the server needs.

### Minimal Configuration

```toml
[server]
token = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
key = "gateway-bearer"
```

Every string value supports `${VAR}` interpolation from the process environment. Use `$$` for a literal dollar. An unset variable fails the load everywhere except `[server].token`, where it drops the token silently so a stdio install can boot without a credential its transport never reads.

### Full Configuration

```toml
[server]
bind = "127.0.0.1:9310"
token = "${PROMPTFORGE_TOKEN}"
max_concurrent_runs = 4
admission_timeout = "30s"
reply_deadline = "240s"
retain_completed = "1h"
watch = true
watch_debounce = "500ms"
allowed_hosts = ["example.com", "example.com:8080"]

[paths]
prompts = "prompts"

[gateway]
url = "http://127.0.0.1:8081/v1"
key = "${GATEWAY_KEY}"

[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["_*.md", "drafts/**"]

[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
```

### Defaults

| Key | Default | Notes |
|-----|---------|-------|
| `bind` | `127.0.0.1:9310` | |
| `max_concurrent_runs` | `4` | |
| `admission_timeout` | `30s` | |
| `reply_deadline` | `240s` | Inside Cursor's 300s call ceiling |
| `retain_completed` | `1h` | |
| `watch` | `true` | |
| `watch_debounce` | `500ms` | |
| `paths.prompts` | `prompts` | Relative to working directory |

Durations use humantime format: `"30s"`, `"5m"`, `"1h"`, `"500ms"`.

Unknown keys are rejected outright - a misspelled key fails the load rather than being silently ignored.

### Sections

**`[server]`** - Bind address, shared bearer token, concurrency limits, timing, and reload settings. `allowed_hosts` controls DNS-rebinding protection: on a loopback bind an empty list defaults to `localhost`, `127.0.0.1`, `::1`; a non-loopback bind with no hosts is refused.

**`[paths]`** - The prompts directory. Catalog patterns and `[prompts.NAME].file` paths are both relative to it.

**`[gateway]`** - The model gateway every run goes through. `url` must be a valid http/https URL with a host. `key` is the bearer credential sent on every model call.

**`[catalog]`** - Glob patterns that assemble the catalog. `include` names what to resolve; `exclude` subtracts from it. `*` does not cross a separator, `**` does.

**`[prompts.NAME]`** - Per-prompt overrides keyed by the prompt's frontmatter name. Set `enabled = false` to drop one the globs caught. Set `file = "path.md"` to add a file no glob matches. The key must match the prompt-name shape: `^[a-z][a-z0-9_]{0,47}$`.

## The Tool Surface

The server publishes a fixed set of built-in tools. No prompt appears in `tools/list` - a prompt is reached only by naming it to `run_prompt`.

| Tool | Purpose |
|------|---------|
| `list_prompts` | Report every enabled prompt: name, description, and any problem stopping it |
| `run_prompt` | Execute a named prompt and return its artifact |
| `check_run` | Collect a run that outlived its call |
| `need_prompt` | Discover prompts by semantic similarity (requires `picker` feature) |

The `picker` feature is on by default. Without it the server publishes three tools and `need_prompt` is absent. A build without `picker` is smaller and removes the embedding model weights.

## Running a Prompt

Call `run_prompt` with `prompt` (required) and `args` (optional):

```json
{
  "prompt": "research_person",
  "args": "Herb Sutter, ABI stability positions"
}
```

### File Parameters

Three additional parameters support file-based input and output:

- `input_file` - filesystem path; the server reads this file and places its content in the store at the prompt's declared input path
- `input_text` - literal text; the server places it in the store directly at the prompt's declared input path
- `output_file` - filesystem path; after the run completes, the server writes the output store file here

`input_file` and `input_text` are mutually exclusive. Providing both is an error.

If `output_file` is omitted, the output content is returned inline in the result's `value` field as usual.

The prompt itself never touches the real filesystem. It reads and writes through MemStore only - the server handles marshalling between the filesystem and the store boundary.

```json
{
  "prompt": "gate_paper",
  "input_file": "/home/user/papers/p2996r7.md",
  "output_file": "/home/user/reports/p2996-gate.md"
}
```

`list_prompts` shows which prompts declare inputs and outputs, so callers know which file parameters apply.

### What Happens

1. **Name resolution** - The name is matched case-normalized against the catalog. An unresolvable name returns all enabled names nearest-first so the model can correct itself.

2. **Admission** - The call waits for one of `max_concurrent_runs` slots. If none comes free within `admission_timeout`, the call gets a retryable refusal: "every run slot is busy and none came free within 30s. Retry in a moment."

3. **Execution** - The prompt runs against the gateway. Progress notifications stream to the client if it supplied a `progressToken`.

4. **Reply deadline** - If the run finishes in time, the result comes back inline. If it exceeds `reply_deadline`, the call returns immediately with status `running` and a `run_id`.

### Background Runs

A run that outlives its call continues in background. Collect it with `check_run`:

```json
{
  "run_id": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"
}
```

A finished run stays collectable for `retain_completed` (default 1 hour), then is evicted.

If the client disconnects while a run is in progress, the run is cancelled cooperatively.

### Result Format

Every result carries structured content:

```json
{
  "run_id": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
  "prompt": "research_person",
  "status": "completed",
  "value": "The full artifact text...",
  "turns": 3,
  "elapsed_ms": 42000,
  "error": null
}
```

Status is one of `running`, `completed`, or `failed`. A completed run carries `value`; a failed run carries `error`; a running run carries neither.

## Discovering Prompts

### list_prompts

Browse the catalog with optional pagination:

```json
{ "cursor": "100" }
```

Returns up to 100 entries per page:

```json
{
  "prompts": [
    { "name": "research_person", "description": "Build a stakeholder profile...", "problem": null },
    { "name": "broken_one", "description": "", "problem": "parse error at line 3" }
  ],
  "next_cursor": "200"
}
```

A broken prompt appears in the listing with its problem visible, so the operator knows what to fix.

### need_prompt

When you have a capability description rather than a name:

```json
{ "capability": "Build a stakeholder position report for one entity." }
```

Returns up to three candidates ranked best-first:

```json
{
  "prompts": [
    { "name": "research_person", "description": "Build a stakeholder profile..." },
    { "name": "staker", "description": "Assess positions on a proposal..." }
  ]
}
```

State the capability the way a tool author would document it: an imperative phrase naming the operation and what it acts on. Conversational phrasing resolves less reliably.

If retrieval is unavailable (model failed to load), `need_prompt` reports it and points you at `list_prompts` instead.

## Live Reload

With `watch = true` (the default), saving a prompt file or `prompts.toml` triggers a re-resolution after the debounce window settles. The catalog and its retrieval index are published together as one atomic generation - no reader ever sees a torn pair.

What reload does:

- A healthy edit updates the catalog immediately. The tool list stays the same because tools are fixed; only the catalog behind `run_prompt` changes.
- A broken edit (parse error, bad name) retains the prompt as a listed entry carrying its problem rather than freezing the whole catalog.
- An edit to a prompt's body alone (no name or description change) carries the previous retrieval index forward without rebuilding it.
- A broken platform watch is re-registered on the next settled window rather than permanently losing live reload.

Set `watch = false` to serve a static catalog for the life of the process.

## Transport and Security

### HTTP

The streamable-HTTP transport puts MCP at `/mcp` and a liveness probe at `/healthz`. The bearer check wraps `/mcp` only - `/healthz` is unauthenticated by design.

Authentication is per-request, not per-session. The token is fixed for the life of the server, but the check happens on every HTTP request rather than once at initialization - so a session that already completed the MCP handshake is still refused if its credential does not match. The comparison is constant-time.

SSE keep-alive is 15 seconds, so a run that thinks between sections does not look dead to a proxy.

`allowed_hosts` is the DNS-rebinding defence. On a loopback bind it defaults to `localhost`, `127.0.0.1`, `::1`. On a non-loopback bind you must enumerate the authorities explicitly or the server refuses to start.

### Stdio

Stdio binds no port, checks no token, and has a bounded line reader so a peer without newlines costs a fixed buffer rather than the process. The harness that spawned it is the only thing that can talk to it.

### Shutdown

Ctrl-C triggers graceful shutdown on both transports. The SSE streams are closed, in-flight calls drain, and the watcher stops before the process exits. No late reload can publish after the shutdown signal.

## Boot Sequence and Gateway

At startup the server:

1. Loads and validates `prompts.toml`
2. Resolves the catalog (refuses to start on any fault)
3. Builds the retrieval index over the catalog (if `picker` feature is present; a failure is logged and the server continues)
4. Fetches the gateway model catalog via `GET /v1/models`
5. Builds the live tool registry (`web_fetch`, `web_search`) and the semantic tool picker
6. Starts the filesystem watcher
7. Serves the chosen transport

The gateway fetch distinguishes transient failures from fatal ones. A connection timeout or a 5xx is transient: the server warns and serves with an empty model catalog, so prompts without `models.need` keep working. A 401, a bad URL, or a malformed response is fatal: the server refuses to boot rather than hiding a misconfiguration behind runtime failures.
