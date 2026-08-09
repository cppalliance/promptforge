# PromptForge development

Technical internals for operators and contributors. Author-facing prompt language lives in `user-guide.md`. Interactive author workflow for a single prompt lives in [`crates/promptforge-dev/README.md`](crates/promptforge-dev/README.md).

## Architecture

Workspace crates (see root `Cargo.toml`):

| Crate | Role |
|---|---|
| `promptforge-core` | Library: prompt parser, gateway client, section execution, source-retaining `LuaProgram` compilation to process-local Lua 5.4 bytecode, synchronous four-outcome picker binding for tools and models (`tools.need` / `models.need`) with atomic one-to-one alias and stable-identity maps validated against the complete live registries, sendable persistent `SectionVm` lifecycle, deterministic Lua declaration binding and exact per-section replay with immutable H2 scope closure (`tools.add` / `models.use`), stable live-tool identity (`ToolId`, `ToolRegistry`, transport-only wire names), model catalog types (`ModelId`, `ModelCatalog`, `ModelBindings`), opt-in `DebugCapture` for raw turn JSON, and `observe` (`Observer`, `NullObserver`) for report-only progress |
| `promptforge-core-tests` | Unpublished binary and test harness: author-shaped valid/invalid/offline fixtures plus opt-in real-model scenarios against a temporary gateway |
| `promptforge-dev` | Unpublished binary for interactive prompt development against an already-running gateway. Never starts the gateway or `llama-server` |
| `promptforge-webfetch` | Library: in-process `web_fetch` (no credential; runs wherever the prompt runs) |
| `promptforge-cli` | Binary `promptforge`: `promptforge run <file.md> [input]` |
| `promptforge-gateway` | Binary `promptforge-gateway`: holds backend credentials, routes OpenAI-shaped chat completions, serves bearer-authed `GET /v1/models`, optional `POST /v1/tools/web_search`, managed local `llama-server` children, named profiles |
| `promptforge-mcp-server` | Binary `promptforge-mcp-server`: MCP surface over streamable HTTP or stdio (`run_prompt`, `list_prompts`, `need_prompt`, `check_run`) |
| `promptforge-tool-picker` | Library: plain-English capability need to tool (or model catalog entry) via embedding; four outcomes `Bind` / `Duplicate` / `Ambiguous` / `Absent`. No Lua, no MCP, no network |

Relationship in one line: hosts (`cli`, `dev`, `mcp-server`, `core-tests`) parse and bind through `promptforge-core`, pick capabilities with `promptforge-tool-picker`, call models through `promptforge-gateway`, and run `web_fetch` locally via `promptforge-webfetch`.

Built-in tools:

- `web_fetch` - URL to main content as markdown (readability, whole-page fallback). Always local.
- `web_search` - trimmed search hits via gateway `POST /v1/tools/web_search` (Brave key stays in the gateway).

Concrete tool names do not belong in YAML frontmatter. `bind::bind_prompt` resolves `tools.need` / `models.need` against a complete live registry and catalog; `execute::run` advertises under local aliases and dispatches by stable `ToolId`. Declared tools are never injected automatically. Binding has no reranker stage.

Default tool-call loop cap is 24 round trips per section when frontmatter omits `max_tool_iterations`.

## Development workflow

### Build and unit tests

```text
cargo build
cargo test
```

The first build downloads the tool picker's embedding model (about 130MB from the Hugging Face Hub, pinned and checksummed) and compiles it into the library. Later builds reuse the Hugging Face cache.

### Environment variables

| Variable | Used by | Purpose |
|---|---|---|
| `PROMPTFORGE_GATEWAY_URL` | CLI, dev, core (`GatewayClient::from_env`) | OpenAI-shaped base URL, typically `http://127.0.0.1:8081/v1` |
| `PROMPTFORGE_GATEWAY_KEY` | CLI, dev, gateway clients, MCP `[gateway].key` | Shared bearer for `/v1/*` |
| `PROMPTFORGE_MCP_TOKEN` | MCP `[server].token` | Bearer for HTTP `/mcp` (unread on stdio) |
| `ANTHROPIC_API_KEY` | Gateway only | Vendor credential for Anthropic endpoints |
| `BRAVE_API_KEY` | Gateway only | Brave Search subscription for `web_search` |
| `PROMPTFORGE_GATEWAY_BIN` | `promptforge-core-tests` scenarios | Override path to the gateway binary |

`GatewayClient::from_env` requires both URL and key (`Error::MissingEnv` if either is unset). The CLI is softer at launch: without a key the model catalog is empty and `web_search` is omitted; with a key it requires URL, hard-fails `GET /v1/models` on fetch error, and offers `web_search` when both URL and key are present. `promptforge-dev` requires both variables before reading any prompt file.

### Running the CLI against a gateway

Two processes: the gateway holds vendor credentials; the client points at it.

```text
export ANTHROPIC_API_KEY=sk-ant-...
export PROMPTFORGE_GATEWAY_KEY=dev-secret
cargo run -p promptforge-gateway -- serve gateway.toml &

export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
cargo run -p promptforge-cli -- run prompts/hello.md
```

Only a prompt whose frontmatter declares a supported `promptforge:` major (currently `1`) runs; otherwise the CLI exits non-zero.

### Interactive prompt development (`promptforge-dev`)

Start the gateway yourself, export `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY`, then:

```text
cargo run -p promptforge-dev -- <prompt.md> [input] [--watch]
```

Result on stdout; observer lines and Lua `log()` checkpoints on stderr. Each run clears `<prompt-stem>.store` beside the prompt, then write-through mirrors store files and raw model turns under `.trace/`. Context, thinking, and `max_tokens` belong on the prompt under `models.need` / `models.always` - the binary accepts only the prompt path, optional input, and `--watch`. Full author notes: [`crates/promptforge-dev/README.md`](crates/promptforge-dev/README.md).

### Real-model scenario suite (`promptforge-core-tests`)

Ordinary `cargo test -p promptforge-core-tests` stays offline. The opt-in scenario suite targets Windows and Linux on x86-64 or arm64, plus macOS on x86-64 or Apple Silicon. Build the gateway first, then from the repository root:

```text
cargo build -p promptforge-gateway
cargo run -p promptforge-core-tests
cargo run -p promptforge-core-tests -- scenarios
```

Both spellings run the same fixed suite. The harness writes a temporary gateway profile pinning official `Qwen/Qwen3-0.6B-GGUF` file `Qwen3-0.6B-Q8_0.gguf` (SHA-256 `9465e63a22add5354d9bb4b99e90117043c7124007664907259bd16d043bb031`), starts `promptforge-gateway serve` on a free loopback port with a random bearer, waits until `GET /health` and authenticated `GET /v1/models` advertise the local model, then runs text and tool-call fixtures through `GatewayClient`. The gateway owns GGUF download/caching under `~/.promptforge/` and spawns `llama-server`. Dropping the guard kills the gateway process tree. Pin provenance and fixture contracts: [`crates/promptforge-core-tests/README.md`](crates/promptforge-core-tests/README.md).

## Gateway configuration

The gateway reads one TOML profile. It defines the server bearer, optional queue, endpoints (backends), models (caller-facing names), optional local generative models, and optional tool credentials.

Shipped development file at the repository root (`gateway.toml`):

```toml
[server]
bind = "127.0.0.1:8081"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[queue]
max_depth = 100
fair_scheduling = true

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

[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
```

Serve forms:

```text
promptforge-gateway serve gateway.toml
promptforge-gateway serve --profile analytical
promptforge-gateway serve --profiles-dir ./profiles --profile base
```

Default profiles directory: `~/.promptforge/profiles/` (Windows: `%USERPROFILE%\.promptforge\profiles\`). Example profiles live under repo `profiles/` (`base.toml`, `analytical.toml`). Pass either `--profile NAME` or a config path, not both.

### Namespaces

Three distinct namespaces, on purpose:

- `endpoint.id` - operator-chosen handle (`anthropic`, `pod-a`, `east-1`); unique within the file; referenced by each model's `endpoints` list. v0 uses the first id; further ids are parsed and ignored.
- `model.name` - caller-facing catalog id. Prompts reach it through `models.need` / `models.use` or `models.always`. Changing it is a breaking change for callers.
- `model.upstream` - vendor model string substituted into the request before it leaves the gateway.

Each `[[model]]` also carries catalog metadata for bearer-authed `GET /v1/models`:

- `description` - required prose for the catalog and semantic bind
- `context` - required context window size in tokens
- `thinking` - `never`, `always`, or `switchable` (default `never`)

Chat still passes `temperature`, `max_tokens`, and `chat_template_kwargs` through the request body's catch-all; the catalog only advertises what a binding may ask for. Several models can share one endpoint (same `base_url` + `api_key`).

### Env interpolation

Any string value may contain `${VAR}`, expanded from the process environment at load time; `$$` is a literal `$`. An unset variable fails the load. Unknown TOML keys fail the load (`deny_unknown_fields`).

### Profiles and include

A profile may inherit:

```toml
include = ["base.toml"]
```

- Paths are relative to the including file
- Depth-first resolution; max depth 16; cycles are `ConfigError`
- Arrays (`endpoint` / `model` / `local_model` / `device`) merge by append; same `id` or `name` is replaced by the later definition
- Scalars (`server.*`, `queue.*`, `local.cache_dir`) - later wins

Admin routes (same bearer as `/v1`):

| Route | Behaviour |
|---|---|
| `GET /admin/profiles` | List `*.toml` stems in the profiles directory |
| `GET /admin/status` | Current profile name, model names, local child count, queue note |
| `POST /admin/switch-profile` `{"name":"..."}` | Immediate switch of routing and local children; bind address unchanged |

`GET /health` is unauthenticated liveness. Every other route checks `Authorization: Bearer`.

### Local generative models

When `[[local_model]]` is present, the gateway downloads a pinned `llama-server`, downloads each GGUF into `~/.promptforge` (or `[local].cache_dir`), spawns one child per local model, and registers each as a normal catalog model. Optional devices/lanes control concurrency; for a local model the resolved lane concurrency is both the gateway admit limit and `llama-server --parallel`.

```toml
[local]
# cache_dir = "~/.promptforge"

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning"
source = "https://huggingface.co/.../model.gguf"
sha256 = "..."
context = 65536
thinking = "never"
gpu_layers = 99
flash_attention = true
cache_type_k = "q8_0"
cache_type_v = "q4_0"
n_predict = 8192
# chat_template_file = "..."   # optional tools-capable Jinja override
```

After a local child is healthy, the gateway resolves a `tool_dialect` / `tools_mode` and advertises them in the catalog. Remote OpenAI-compat models default to `openai` / `native`. See `gateway.local.example.toml` and [`crates/promptforge-gateway/design-gateway.md`](crates/promptforge-gateway/design-gateway.md) for the full local path.

### Tool configuration (`web_search`)

Optional. Without `[tools.web_search]` the gateway still serves chat; the tool route returns 404.

```toml
[tools.web_search]
provider = "brave"                   # v0 supports only "brave"
api_key = "${BRAVE_API_KEY}"
base_url = "https://api.search.brave.com/res/v1"  # optional override
# default_count = 10
# max_count = 20
# max_per_host = 2
# default_freshness = ""
# default_safesearch = ""
# strip_tracking = true
```

Exposes bearer-authed `POST /v1/tools/web_search`. The route echoes `query`, returns trimmed hits (`title`, `url`, `description`, optional `age` / `site_name` / `extra_snippets`), rejects empty query with 400, and applies host diversity plus optional domain filters after Brave.

## Model catalog and binding flow

1. Host fetches `GET /v1/models` (CLI hard-fails when `PROMPTFORGE_GATEWAY_KEY` is set; MCP soft-fails to empty with a warning) and builds `ModelCatalog`.
2. Host builds a complete live `ToolRegistry` and a picker `Catalog` from the same concrete tool instances (`web_fetch` always; `web_search` when gateway credentials exist).
3. `bind::bind_prompt` runs shared H1 Lua once:
   - Tools: `tools.need(alias, description)` then optional `tools.always(alias)` for prompt-wide scope.
   - Models: `models.need(alias, description, opts?)` filters the catalog by hard constraints (`context`, `thinking`) then resolves the description semantically with the same picker stack rebuilt over the filtered catalog. Optional `opts` fields: `context`, `thinking`, `temperature`, `max_tokens`. Invocation params freeze on the binding; same weights under different params may be distinct aliases.
   - `models.always(alias)` or combined `models.always(alias, description, opts?)` sets the prompt-wide default. At most one `models.always` per prompt.
4. Outcomes `Absent`, `Duplicate`, and `Ambiguous` map to distinct core errors for tools and models. Tool identity collisions and near-duplicate pairs are rejected or precomputed; model aliases are not rejected for sharing weights. Prompts with no `models.need` keep working with an empty catalog.
5. Result is an immutable `BoundPrompt`. `SectionVm` replays frozen declarations without resolving again. H2 `tools.add(alias)` and at most one `models.use(alias)` close section scope. Non-empty model-facing prose without `models.use` or `models.always` fails with `Error::ModelRequired`.

Identity: v0 `ModelId` uses namespace `"gateway"` plus the caller-facing `[[model]].name`. Completions pass through `CompletionNormalizer` (default `OpenAiChatNormalizer`) before the tool loop.

## Store API

The run-scoped store provides three read operations:

| Op | Returns | Use |
|---|---|---|
| `store.read_lines(path)` | Numbered lines (`1\| ...`) | Editing, navigation, `str_replace` |
| `store.read(path)` | Verbatim contents | Trusted handoff, run output, clean dumps |
| `store.inject(path)` | Verbatim + untrusted envelope | Model-facing re-injection |

`store.write(path, contents)` creates or overwrites. `store.append`, `store.str_replace`, `store.delete`, and `store.glob` round out the API. `str_replace` requires the old string to occur exactly once. `glob` supports `*` (one path segment) and `**` (across `/`). Arms of a fanout share the run's store with the invoker.

## MCP server configuration

```text
cargo run -p promptforge-mcp-server -- serve prompts.toml            # streamable HTTP on [server].bind
cargo run -p promptforge-mcp-server -- serve --stdio prompts.toml    # stdio, for a local harness
```

Over HTTP the MCP endpoint is `/mcp` and every request must present the shared bearer from `[server].token` (`401` + `WWW-Authenticate: Bearer` otherwise). Empty or whitespace-only token is refused at load; absent token refuses the HTTP bind. Auth is per HTTP request, not per MCP session. `/healthz` is the one unauthenticated route. SSE streams are pinged every 15 seconds.

Over stdio nothing is bound and no token is read. `[server].bind` is logged as ignored; `[server].token` may be omitted. `[gateway].key` is required on both transports. Logs go to stdout on HTTP and to stderr on stdio.

Boot resolves the whole prompt catalog first and refuses an incomplete one. It builds the complete live tool registry and matching picker catalog, and fetches `GET /v1/models` (soft-fail to empty). Every `run_prompt` reuses its run id as the observer execution id, binds H1 needs on Tokio's blocking pool, then executes the `BoundPrompt` with the same observer.

Shipped `prompts.toml` at the repository root (run from the repository root so relative paths resolve):

```toml
[server]
bind = "127.0.0.1:9310"
token = "${PROMPTFORGE_MCP_TOKEN}"

[paths]
prompts = "prompts"

[gateway]
url = "http://127.0.0.1:8081/v1"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[catalog]
include = ["**/*.md"]
exclude = ["**/_*.md", "drafts/**"]
```

Only `[server]` and `[gateway]` are required; other tables have defaults; unknown keys fail the load. Durations are plain strings (`500ms`, `30s`, `1h`). `${VAR}` / `$$` interpolation matches the gateway.

Fuller shape with defaults:

```toml
[server]
bind = "127.0.0.1:9310"
token = "${PROMPTFORGE_MCP_TOKEN}"
max_concurrent_runs = 4
admission_timeout = "30s"
reply_deadline = "240s"
retain_completed = "1h"
watch = true
watch_debounce = "500ms"

[paths]
prompts = "prompts"

[gateway]
url = "http://127.0.0.1:8081/v1"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["**/_*.md", "drafts/**"]

[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
```

`reply_deadline` should stay under the calling client's ceiling (Cursor remote calls fail around 300 seconds; progress notifications do not reset that clock).

### Catalog resolution

Expand `include`, subtract `exclude`, then apply `[prompts.NAME]` blocks (`enabled = false` drops a glob hit; `file = "..."` reaches a file no glob matches). Patterns are relative to `[paths].prompts`.

Stored identity is frontmatter `name` matching `^[a-z][a-z0-9_]{0,47}$`. `run_prompt` case-folds and treats `-` as `_` for lookup. Reserved names: `list_prompts`, `run_prompt`, `need_prompt`, `check_run`. Boot refuses collisions, unreadable/unparsable files, duplicate names, stale overrides, and empty catalogs. Markdown without `promptforge:` is skipped silently by globs.

### Reloading on save

With `watch = true`, save re-resolves the catalog; validation failures become broken entries rather than stopping the process. Events debounce for `watch_debounce`. In-flight runs keep the catalog they started with. `[server]`, `[gateway]`, and `[paths].prompts` do not hot-reload - restart required. A candidate catalog that cannot resolve leaves the previous catalog serving. The published tool list never changes (no `listChanged`).

### Harness surface

| Tool | Arguments | Published when |
|---|---|---|
| `list_prompts` | none | always |
| `run_prompt` | `prompt`, optional `args` | always |
| `need_prompt` | `capability` | `picker` feature compiled in |
| `check_run` | `run_id` | always |

`run_prompt` returns structured `RunResult` (`run_id`, `prompt`, `status` of `running` / `completed` / `failed`, `value`, `turns`, `elapsed_ms`, `error`). Past `reply_deadline` the call returns `status: running` with a collectable `run_id`; use `check_run`. Admission waits up to `admission_timeout` for a `max_concurrent_runs` slot. Finished runs stay collectable for `retain_completed`. The registry is in-memory; restart forgets every run.

`need_prompt` embeds name + description and returns up to three closest prompts. Broken prompts are never candidates. `--no-default-features` drops the retrieval `picker` feature and `need_prompt`, but keeps embedding weights required for execution-time capability binding.

Progress (`notifications/progress` when the call carries a `progressToken`):

| When | `progress` | `message` |
|---|---|---|
| run starts | 0 | prompt H1 title |
| each section starts | sections entered so far, from 1 | section heading |

`total` is always absent. Reported `turns` counts exact `Model turn completed` observations.

### Attaching clients

Cursor (streamable HTTP) in `~/.cursor/mcp.json` or project `.cursor/mcp.json`:

```json
{
  "mcpServers": {
    "promptforge": {
      "url": "http://127.0.0.1:9310/mcp",
      "headers": {
        "Authorization": "Bearer dev-secret"
      }
    }
  }
}
```

Claude Code (stdio) after `cargo install --path crates/promptforge-mcp-server`, in project `.mcp.json`:

```json
{
  "mcpServers": {
    "promptforge": {
      "command": "promptforge-mcp-server",
      "args": ["serve", "--stdio", "/abs/path/to/prompts.toml"],
      "env": {
        "PROMPTFORGE_GATEWAY_KEY": "dev-secret"
      }
    }
  }
}
```

Use absolute paths for the config argument and `[paths].prompts` under stdio: the harness chooses the working directory.

## Watching a run

`execute::run` takes `RunOptions`:

```rust
use promptforge_core::execute::{self, RunOptions};
use promptforge_core::observe::NullObserver;

let opts = RunOptions {
    execution: "example-run", // one caller-owned id for parse, bind, and run
    observer: &NullObserver,   // or your own Observer
    client: None,              // None builds the gateway client from the environment
    debug: None,               // None skips raw request/response capture
};
let result = execute::run(&prompt, input, &tools, &store, opts).await?;
```

- `execution` - caller-owned stable id. Pass the same value to `Prompt::parse`, `bind_prompt`, and `RunOptions`. CLI generates one per invocation; MCP reuses its run id.
- `observer` - receives borrowed `(execution, section, detail)` strings for parse/bind, run start/end, section boundaries, model turns, tool calls, harness store ops, and accepted Lua `log(message)` checkpoints. Synchronous and on the caller's path: forward by copying into a queue, do not block. Observations are reports, never decisions (`NullObserver` is what the CLI passes).
- Fixed details are stable exact strings from `promptforge_core::observe::detail`. They contain no prompt prose, model I/O, tool payloads, store paths/contents, credentials, or fetched content. The sole payload-bearing exception is a validated `Lua: <message>` author checkpoint (max 256 UTF-8 chars, no newline or control character) - keep it a short static label.
- Empty model product fails as `Error::EmptyModelReply` (observed as `Model turn failed`). `observe::detail::MODEL_REPLY_EMPTY` remains for host compatibility but is not emitted on that path. A successful turn whose `finish_reason` is `length` also reports `Model turn truncated`.
- The MCP adapter recognizes `Run started` and `Section started` for cosmetic numeric progress and tolerates unknown details.
- `client: None` builds from `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY` on first use. File-configured hosts pass an owned client (edition 2024 forbids setting process env from safe Rust in this workspace).
- `debug` is opt-in `DebugCapture` for raw request/response bodies. Production hosts leave it `None`. The dev runner writes each turn under `<prompt-stem>.store/.trace/`.

Design depth for core and gateway: [`crates/promptforge-core/design-core.md`](crates/promptforge-core/design-core.md), [`crates/promptforge-gateway/design-gateway.md`](crates/promptforge-gateway/design-gateway.md), [`crates/promptforge-mcp-server/design-mcp-server.md`](crates/promptforge-mcp-server/design-mcp-server.md).
