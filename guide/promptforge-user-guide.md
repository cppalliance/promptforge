# PromptForge User Guide

PromptForge turns Markdown files into executable AI prompt pipelines. This guide covers every component: the CLI that runs prompts, the gateway that talks to model backends, the core library that parses and executes prompt files, the MCP server, the tool picker, the web fetch tool, and the development runner.

---

## promptforge-cli User Guide

`promptforge` is a command-line tool that runs PromptForge prompt files in a single process. Point it at a prompt file, and it parses the sections, executes them top to bottom, and prints the returned value. No server to start, no connection to manage, no configuration to write. You edit a prompt, run it, and see what it produces. This guide covers every capability the CLI provides, from the first invocation to gateway configuration and cancellation.

### Running Your First Prompt

The binary is named `promptforge`. It has one command:

```bash
promptforge run <file.md> [input]
```

The file must be a PromptForge prompt. That means its YAML frontmatter must declare a `promptforge:` version. If it does not, the CLI refuses the file before attempting to parse it:

```
error: prompt.md is not a promptforge prompt: its frontmatter declares no `promptforge:` version
```

A valid prompt file is read from disk, parsed by the core parser, and executed in-process. The binary links the PromptForge executor directly rather than connecting to an MCP server or any other service. This is a development tool for the edit-run loop: you edit a prompt file, run it with `promptforge run`, and see the result immediately.

The simplest invocation takes just a file path:

```bash
promptforge run prompts/hello.md
```

Prompts are addressed by file path, not by name from a catalog. There is no configuration file, no resolution rule, and no catalog lookup. Shell completion, relative paths, and `..` work as they do with any file argument.

### Input and Output

The optional second argument is a raw input string that becomes the prompt's `args` value in its entirety:

```bash
promptforge run prompts/staker.md "Bloomberg"
```

The prompt body decides what that text means. The binary does not inspect, split, or coerce it. An input containing spaces must be quoted as a single shell argument.

When the prompt completes, its returned value goes to stdout. Errors go to stderr. Nothing is mixed. On success, stdout contains exactly the returned value and nothing else. On failure, nothing appears on stdout. This clean separation means shell substitution works:

```bash
report=$(promptforge run prompts/digest.md "2026-08")
```

The variable `report` captures exactly what the prompt returned.

### Gateway Configuration

Gateway credentials come from two environment variables:

- `PROMPTFORGE_GATEWAY_URL` - the gateway base URL
- `PROMPTFORGE_GATEWAY_API_KEY` - the bearer token

There are no CLI flags for credentials. This is deliberate: secrets never appear in `argv`, where `ps` and shell history can expose them.

**Local-only mode** is the default. With neither variable set (or with empty/whitespace-only values), the CLI runs without a gateway. The `web_fetch` tool is available, but there is no `web_search` and no remote model catalog. A prompt that makes no model calls works entirely self-contained in this mode.

**Remote mode** activates when both variables are set:

```bash
export PROMPTFORGE_GATEWAY_URL="https://gateway.example.com/v1"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
promptforge run prompts/search-demo.md "latest Rust news"
```

This enables the `web_search` tool and fetches the remote model catalog, so prompts can perform inference through the gateway.

Setting a key without a URL is rejected explicitly:

```
error: PROMPTFORGE_GATEWAY_API_KEY is set but PROMPTFORGE_GATEWAY_URL is missing or empty; both are required to reach the gateway
```

### Tools

Two tools are available to prompts, depending on the gateway configuration:

**`web_fetch`** runs locally and is always available regardless of gateway mode. It needs no credentials.

**`web_search`** proxies through the gateway and is available only when both `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` are set. When the gateway is not configured, `web_search` is omitted entirely rather than advertised as a tool that would fail on its first call.

The tool picker resolves `tools.bind` calls from prompts against the live tool set. Picker descriptors are derived from the same live tool instances, so the tool catalog and picker catalog have identical entries by construction. If a prompt needs a tool that is not available (for example, `web_search` without gateway credentials), the resolution produces the standard absent-capability error before any section executes.

### File Validation

Before parsing, the CLI checks whether the file's YAML frontmatter declares a `promptforge:` version key. If the key is absent, the file is refused with a clear message naming the reason.

This matters because pointing the tool at an ordinary markdown file without this check would produce a confusing parse error about syntax, sending the user to fix the wrong thing. The version check answers a different question: is this file one of ours at all?

### Cancellation and Exit Codes

Press Ctrl-C to cancel a running prompt. The signal trips a cooperative cancellation handle, and the process exits with code 130.

The four exit codes:

| Code | Meaning |
|------|---------|
| 0 | Success - the prompt completed and its value was printed |
| 1 | Operational failure - unreadable file, not a prompt, parse error, setup failure, or execution failure |
| 2 | Usage error - owned by the argument parser (missing file, unknown command) |
| 130 | Cancelled - the run was interrupted with Ctrl-C |

In a script, check `$?` to branch on success or failure. If you need to distinguish failure causes, read the error message on stderr.

### Runtime Behavior

Each run creates an in-memory store. A prompt's filed state lives exactly as long as the process. Nothing is written to disk unless the prompt itself writes something. State does not accumulate across runs. A prompt that needs durable artifacts requires a caller that provides a durable store.

Each run generates a unique execution ID, a 36-character string prefixed with `cli-`, for correlating observations within a single invocation.

Progress is discarded by default. The binary installs a null observer, so long runs produce no progress output. The result appears when the run finishes, and silence in between is expected. A rendering client or progress display would be a separate concern.

---

## User Guide - promptforge-gateway

promptforge-gateway is the one process in PromptForge that talks to LLM backends. Point it at a TOML file, and it serves an OpenAI-compatible HTTP API that routes chat completions to configured backends, holds every credential, manages a model catalog, runs a built-in web search tool, and optionally spawns local `llama-server` processes for GGUF models. Nothing above it holds a vendor key. Nothing above it knows which machine answers. A key rotation touches one file on one host. After reading this guide, you will be able to configure, start, and operate the gateway for remote endpoints, local models, multiple profiles, and built-in tools.

### What the Gateway Does

The gateway accepts `POST /v1/chat/completions` requests in the OpenAI chat completions format. It resolves the model name the caller asked for, substitutes the backend's own model string into the outgoing request, forwards it, and restores the caller's model name on the response. Everything else in the request body - sampling parameters, tool definitions, template arguments - passes through untouched in a flattened map, so a parameter the gateway has never heard of reaches the backend without a gateway release.

Credentials live here and nowhere else. Each `[[endpoint]]` carries an `api_key`, each `[tools.web_search]` carries a search provider key, and each `[[local_model]]` is reached over a loopback connection with a generated bearer. The `Secret` type ensures no credential can be serialized, logged, or printed: it redacts in both `Debug` and `Display`, and `expose()` is the single plaintext accessor.

Model resolution is one exact string lookup. A miss is a 404. There is no prefix matching, no regex, no alias chain, and no default model. A typo is a clear error rather than a silent charge against the wrong backend.

### Configuration

The gateway boots from two TOML files: the boot file named on the command line, which is the catalog and infrastructure, and a named profile from the boot file's sibling `profiles/` directory, which is the initial loaded set. Every configuration struct uses `deny_unknown_fields`, so a misspelled key is a boot failure rather than a setting silently ignored.

A minimal configuration defines a server (bind address and bearer key), one endpoint, and one model:

```toml
[server]
bind = "127.0.0.1:8080"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"

[[model]]
name = "reasoning-large"
description = "Anthropic's best reasoning model"
context = 200000
upstream = "claude-sonnet-4-6"
endpoints = ["anthropic"]
```

The `name` is what callers request. The `upstream` is what the backend knows the model by. Name your models by capability (`reasoning-large`, `fast-draft`) when you want the same prompt to work across environments where the backend changes.

Any string value can use `${VAR}` to reference an environment variable. Interpolation runs after the TOML is parsed, so it applies only to string values. An unresolved variable fails the load, so a deployment that forgot to export a credential never starts serving with a blank one. Use `$$` for a literal dollar sign.

There is no implicit pickup of `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` from the ambient environment. Every credential must appear in the configuration as an explicit `${VAR}` reference.

At boot the process environment is populated from at most two env files: the profile's own file first (`profiles/main.env` for `--profile main`), then the boot file's sibling env file (`gateway.env` beside `gateway.toml`). Neither file overrides a variable that is already set, so precedence is the process environment, then the profile's file, then the boot file's. Included files' env files are never loaded. On a profile switch only the new profile's env file is loaded; the boot file's is already in the process.

#### Model fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `name` | yes | - | Caller-facing model name |
| `description` | yes | - | Prose for catalog consumers |
| `context` | yes | - | Context window size in tokens |
| `upstream` | yes | - | The string the backend knows this model by |
| `endpoints` | yes | - | Endpoint ids (first is used) |
| `thinking` | no | `never` | `never`, `always`, or `switchable` |
| `default_max_tokens` | no | - | Parsed, not yet consumed |

#### Endpoint fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `id` | yes | - | Operator handle referenced by models |
| `protocol` | yes | - | Wire protocol; only `openai` |
| `base_url` | yes | - | Backend URL (trailing slash trimmed) |
| `api_key` | yes | - | Backend credential; empty string skips the `Authorization` header |
| `concurrency` | no | unlimited | Max in-flight requests |
| `device` | no | - | Device id for concurrency |
| `dominion` | no | - | Remote dominion id for a shared limit |

### Starting the Gateway

```
promptforge-gateway serve gateway.toml --profile main
```

Boot requires two things: a config path and a profile name. The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable; the CLI argument wins, and with neither set the boot fails with a usage error naming both sources. The profile name comes from `--profile` only (no env var). It is required - there is no anonymous boot. Every gateway has at least one profile; the initial loaded set always has a name.

The profiles directory is always the `profiles/` directory beside the config file - never independently configurable, and there is no `~/.promptforge/profiles` default. Booting with an unknown profile name fails with a startup error listing the available profiles; a missing `profiles/` directory or a missing profile file is likewise a startup error.

The boot file is the catalog and infrastructure; it is not loaded as the runtime config directly. The named profile is loaded with include resolution and becomes the initial config. The single-file setup needs one minimal profile, `profiles/main.toml` beside `gateway.toml`:

```toml
include = ["../gateway.toml"]
```

Startup order: load the two env files, resolve the profile's include chain, start local model runtime (when `[[local_model]]` is present), build the routing table, bind, serve. A broken config never reaches a listening socket.

### Model Catalog and Routing

Clients discover available models by calling `GET /v1/models` with a bearer token:

```json
{
  "object": "list",
  "data": [
    {
      "id": "reasoning-large",
      "object": "model",
      "description": "Anthropic's best reasoning model",
      "context": 200000,
      "thinking": "never",
      "tool_dialect": "openai",
      "tools_mode": "native"
    }
  ]
}
```

Each entry includes the model's context window, thinking mode, and tool-calling dialect so clients can make binding decisions before sending a request.

A chat completion request names a model and provides messages:

```json
{
  "model": "reasoning-large",
  "messages": [{"role": "user", "content": "Explain monads"}],
  "temperature": 0.7
}
```

Send this as `POST /v1/chat/completions` with `Authorization: Bearer <token>`.

The gateway validates: model must be non-empty, messages must be non-empty, each message must be a JSON object with a supported role (`system`, `user`, `assistant`, `tool`, `function`, `developer`) and either `content` or a tool/function call. Everything else passes through verbatim.

The response carries the caller's model name, not the backend's.

### Authentication and Errors

Every route except `GET /health` checks `Authorization: Bearer <token>` against `server.api_key`. The comparison is constant-time: both values are SHA-256 hashed to fixed-length digests, then compared with the `subtle` crate's `ConstantTimeEq`. A missing or wrong token returns 401 with no detail.

`GET /health` is unauthenticated and always returns `{"status": "serving"}` while the process is up.

All errors use the OpenAI error envelope:

```json
{
  "error": {
    "message": "unknown model reasoning-large",
    "type": "invalid_request_error",
    "code": "model_not_found"
  }
}
```

| Condition | Status | `type` | `code` |
|---|---|---|---|
| Wrong or missing bearer | 401 | `authentication_error` | `unauthorized` |
| Unknown model | 404 | `invalid_request_error` | `model_not_found` |
| Tool not configured | 404 | `invalid_request_error` | `not_found` |
| Bad request body | 400 | `invalid_request_error` | `malformed_request` |
| Backend unreachable | 502 | `server_error` | `upstream_transport` |
| Backend decode failure | 502 | `server_error` | `upstream_protocol` |
| Backend 4xx | upstream's | `invalid_request_error` | `upstream_client_error` |
| Backend 5xx | 502 | `server_error` | `upstream_error` |
| Queue full | 503 | `server_error` | `queue_full` |

An unmodified OpenAI SDK surfaces these as its own error types rather than unparseable blobs.

### Concurrency and Queuing

Set `concurrency` on an endpoint to limit how many requests are in flight at once:

```toml
[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"
concurrency = 10
```

Requests beyond the limit wait in a queue. Configure the queue globally:

```toml
[queue]
max_depth = 100         # waiting requests (not counting in-flight); default 100
fair_scheduling = true  # round-robin by client key; default true
```

When `fair_scheduling` is true, callers identify themselves via the `X-PromptForge-Client` header. Each client gets turns in round-robin order, so one fast client cannot monopolize slots. Missing or invalid headers map to the `"default"` bucket. The scheduler tracks up to 32 distinct client labels; additional labels fold into `"default"`.

A full queue returns 503 with `code: "queue_full"`. An endpoint without `concurrency` is unlimited.

### Dominions

A dominion is a named pool of compute - a remote provider pool or a local GPU - carrying one concurrency limit and one bounded waiting queue shared by everything bound to it. Where `concurrency` on an endpoint caps that endpoint alone, a dominion's limit is shared: two endpoints bound to the same dominion compete for the same slots.

```toml
[[dominion]]
id = "runpod-pool"
kind = "remote"
max_concurrency = 4
max_queue = 50            # bounded wait, then rejection; default 100
policy = "queue"          # "queue" | "reject" (fail-fast); default "queue"
fair_scheduling = true    # per-client round-robin; default true

[[dominion]]
id = "gpu0"
kind = "local"
vram_gb = 24              # local kind only; co-residency budget

[[endpoint]]
id = "runpod-a"
protocol = "openai"
base_url = "https://..."
api_key = "${RUNPOD_KEY}"
dominion = "runpod-pool"  # optional; absent = unlimited pass-through

[[local_model]]
name = "qwen-local"
description = "..."
source = "..."
context = 65536
dominion = "gpu0"         # optional; must name a local dominion
parallel = 4              # child --parallel and gateway queue limit
vram_gb = 14              # footprint estimate for the co-residency check
```

| Field | Required | Default | Purpose |
|---|---|---|---|
| `id` | yes | - | Operator handle referenced by endpoints and local models |
| `kind` | yes | - | `remote` (bindable by endpoints) or `local` (bindable by local models) |
| `max_concurrency` | no | unlimited | Max in-flight requests admitted across every binder |
| `max_queue` | no | 100 | Max waiting requests before new admits are rejected |
| `policy` | no | `queue` | `queue` waits for a slot; `reject` fails fast at capacity |
| `fair_scheduling` | no | `true` | Round-robin waiting callers by client key |
| `vram_gb` | no | - | VRAM budget in GiB; local kind only |

Binding is by explicit id and is kind-checked: an endpoint's `dominion` must name a remote dominion, a local model's `dominion` must name a local one, and an unknown id is a boot failure. `vram_gb` on a remote dominion is rejected. Dominion ids must be unique and non-empty, and `max_concurrency` and `max_queue` must be at least 1 when set.

Dominions are being introduced alongside the legacy knobs: `concurrency`, `device`, `lane`, and `[queue]` still parse during the transition, and an endpoint with only those fields keeps its own per-endpoint queue. An endpoint bound to a remote dominion shares that dominion's queue with every other bound endpoint - the shared limit is enforced now. Local-model dominion binding and the VRAM co-residency check land as the queue rework completes.

### Web Search Tool

Enable the built-in web search tool by adding a `[tools.web_search]` section:

```toml
[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
```

The gateway proxies search requests to the Brave Search API with its own credential. The executor never sees the search key.

Send a search request:

```json
{
  "query": "Rust async runtime comparison",
  "count": 5
}
```

Send this as `POST /v1/tools/web_search` with `Authorization: Bearer <token>`.

The response contains trimmed results:

```json
{
  "query": "Rust async runtime comparison",
  "results": [
    {
      "title": "Comparing Tokio, async-std, and smol",
      "url": "https://example.com/article",
      "description": "A detailed comparison of...",
      "age": "2 days ago",
      "site_name": "example.com"
    }
  ]
}
```

Provider extras like thumbnails and ranking metadata are dropped - every byte would land in a model's context window.

#### Request fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `query` | yes | - | Search query (Unicode trimmed, max 512 chars) |
| `count` | no | `default_count` (10) | Results requested; clamped to `1..=max_count` |
| `freshness` | no | `default_freshness` | `pd` (day), `pw` (week), `pm` (month), `py` (year), or `YYYY-MM-DDtoYYYY-MM-DD` |
| `country` | no | - | 2-char country code |
| `search_lang` | no | - | 2-3 char language code |
| `safesearch` | no | `default_safesearch` | `off`, `moderate`, or `strict` |
| `include_domains` | no | - | Bare hostnames to include |
| `exclude_domains` | no | - | Bare hostnames to exclude |

Domain filters must be bare hostnames (no scheme, path, or port). A hostname matches when it equals the domain or ends with `.<domain>`.

#### Configuration defaults

| Key | Default | Purpose |
|---|---|---|
| `provider` | (required) | Only `brave` |
| `api_key` | (required) | Provider credential |
| `base_url` | `https://api.search.brave.com/res/v1` | Provider URL |
| `default_count` | 10 | Used when request omits `count` |
| `max_count` | 20 | Clamp ceiling |
| `max_per_host` | 2 | Diversity cap per hostname |
| `default_freshness` | `""` (omit) | Applied when request omits freshness |
| `default_safesearch` | `""` (omit) | Applied when request omits safesearch |
| `strip_tracking` | `true` | Remove `utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid` from URLs |

Results are post-processed in fixed order: sanitize text, strip tracking parameters, set `site_name`, apply include/exclude domain filters, diversify by hostname (max 2 per host by default), then cap at `count`. Over-length URLs are dropped whole rather than truncated into broken links.

When `[tools.web_search]` is absent, the route returns 404 - an absent resource, not a broken capability.

### Named Profiles

Organize configurations for different environments as TOML files in the `profiles/` directory beside the boot file:

```
<config-parent>/
  gateway.toml
  profiles/
    main.toml
    analytical.toml
    dev.toml
```

Start with a named profile:

```
promptforge-gateway serve gateway.toml --profile analytical
```

Every gateway boots into a profile, so the initial loaded set always has a name. A profile typically contains `include = ["../gateway.toml"]` plus its own overrides, keeping the boot file as the shared catalog.

#### Profile inheritance

A profile can include parent files:

```toml
# analytical.toml
include = ["base.toml"]

[[model]]
name = "analysis"
description = "Deep analysis model"
context = 200000
upstream = "claude-sonnet-4-6"
endpoints = ["anthropic"]
```

Includes resolve depth-first relative to the including file. Max nesting depth is 16. Cycles are detected and rejected.

Merge rules:
- Arrays (`[[endpoint]]`, `[[model]]`, `[[local_model]]`, `[[device]]`, `[[dominion]]`): merged by append. An entry with the same `id` or `name` replaces the earlier definition.
- Scalars (`server.*`, `queue.*`, `[local].cache_dir`): later wins.

#### The boot file owns `[server]`

After include resolution, the profile's merged `[server]` section must equal the boot file's `[server]` exactly - bind address and api_key, compared as values after `${VAR}` interpolation. A mismatch fails the boot (or the profile switch): a bind mismatch names both addresses, while an api_key mismatch names only the profile and the field, with both keys redacted. The conventional setup passes by construction because profiles include the boot file. The consequence: the socket and the gateway bearer key are fixed for the process lifetime, and a profile switch never rotates the admin credential.

Includes remain free-form: a profile may include a different file than the boot path, or be self-contained - a self-contained profile must replicate the boot file's `[server]` verbatim to boot. At startup the gateway logs the resolved include chain, plus a warning when the boot file is not in it: the likely-mistake case, where edits to the boot file have no effect.

#### Admin routes

All admin routes use the same bearer token as `/v1`:

| Route | Method | Purpose |
|---|---|---|
| `/admin/profiles` | GET | List `*.toml` stems in the profiles directory |
| `/admin/status` | GET | Current profile name, loaded model names, local child count |
| `/admin/switch-profile` | POST | Switch to a named profile immediately |

Switch with:

```json
{"name": "analytical"}
```

Send this as `POST /admin/switch-profile` with `Authorization: Bearer <token>`.

Profile switches are serialized by a mutex. The old local children are stopped (freeing VRAM) before new ones start. The new configuration is built and validated before touching live state. On success, the routing, web-search settings, and local runtime are atomically swapped. On failure, the previous state stays intact with a stable admin credential.

The `[server]` section does not change on switch: the boot file owns it, and a profile whose merged `[server]` differs from the boot file's is rejected. Moving the socket or rotating the gateway key requires a restart.

Profile names must be a single path component - no separators, no `.` or `..`, no empty string. This confinement prevents directory traversal through the admin API.

### Local Inference

Run local generative models by declaring `[[local_model]]` entries. The gateway provisions a pinned `llama-server` binary (GPU builds: Vulkan on Windows/Linux, Metal on macOS), downloads each GGUF, and spawns one child process per model.

```toml
[local]
# cache_dir = "~/.promptforge"  # default

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning"
source = "https://huggingface.co/Qwen/Qwen3.5-9B-GGUF/resolve/main/qwen3.5-9b-q4_k_m.gguf"
sha256 = "abcdef..."
context = 65536
gpu_layers = 99
flash_attention = true
```

Each local model becomes a normal catalog entry. Clients reach it through the same `POST /v1/chat/completions` as remote models - the fact that it runs locally is invisible to callers.

#### Configuration fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `name` | yes | - | Caller-facing model name |
| `description` | yes | - | Prose for catalog and semantic bind |
| `source` | yes | - | Hugging Face URL or local path to GGUF |
| `sha256` | no | - | SHA-256 hex pin; verified after download |
| `context` | yes | - | Context window (`--ctx-size`) |
| `thinking` | no | `never` | `never`, `always`, or `switchable` |
| `gpu_layers` | no | 99 | GPU layers offloaded (`-ngl`) |
| `flash_attention` | no | `true` | Enable flash attention |
| `cache_type_k` | no | `q8_0` | KV cache type for K |
| `cache_type_v` | no | `q4_0` | KV cache type for V |
| `n_predict` | no | 8192 | Generation ceiling (`--n-predict`) |
| `chat_template_file` | no | - | Jinja template override (`--chat-template-file`) |
| `device` | no | - | Device id for concurrency |
| `lane` | no | - | Lane id under the device |
| `dominion` | no | - | Local dominion id for a shared limit |
| `parallel` | no | - | Max concurrent inferences (`--parallel` and queue limit) |
| `vram_gb` | no | - | VRAM footprint estimate in GiB |

#### Cache and provisioning

The cache directory defaults to `~/.promptforge` (set `[local].cache_dir` to override). Models land in `<cache>/models/`, the llama.cpp binary in `<cache>/llama.cpp/`.

First-time downloads show an indicatif progress bar on interactive TTY stderr - percent, bytes, rate, and ETA. On non-TTY stderr, periodic tracing progress lines are emitted instead.

When `sha256` is set, the downloaded file is verified against the digest.

#### Tool-calling dialect detection

After a local child reports ready, the gateway queries its `/props` endpoint and resolves a tool-calling dialect through promptforge-core's `ToolDialectRegistry`. The catalog advertises the resolved `tool_dialect` (e.g. `openai`, `gemma3_tool_code`) and `tools_mode` (`native` or `emulated`). A sidecar `.md` file beside the GGUF (with frontmatter and a Jinja chat template) provides fallback evidence when `/props` omits `chat_template`; live props always win.

#### Child supervision

If a transport failure occurs against a dead `llama-server` child, the gateway respawns it once on the same port and alias, then retries the request. There is no background watchdog. `GET /health` remains process-level liveness only.

#### Device and lane concurrency

For structured concurrency control over local GPU resources:

```toml
[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1

[[local_model]]
name = "qwen-local"
description = "..."
source = "..."
context = 65536
device = "local-gpu"
lane = "generative"
```

The lane's `concurrency` is both the gateway's admit limit and `llama-server --parallel`. A local model without a device/lane defaults to concurrency 1.

Dropping the `LocalRuntime` (on process exit or profile switch) kills all `llama-server` children.

---

## promptforge-core User Guide

promptforge-core is a Rust library that turns Markdown files into executable AI prompt pipelines. You write a prompt as a document - YAML frontmatter for metadata, embedded Lua for logic, prose blocks for model instructions - and the library parses it into a validated representation, then executes it against any OpenAI-compatible endpoint. No process-global state, no framework lock-in, no runtime compilation surprises. The caller owns every resource. What you get: structured multi-section prompts with tool dispatch, model orchestration, concurrent fanout, and a virtual filesystem, all driven from a single `run` call that returns a string.

---

### Prompt Files

A prompt file is a Markdown document with YAML frontmatter. The frontmatter must declare `name` and `description`. A `promptforge:` key identifies the file as a promptforge prompt - the runtime refuses files that lack a supported version number.

```yaml
---
name: summarizer
description: Summarize a document into bullet points
promptforge: 1
---
```

Below the frontmatter, the document has one H1 title and zero or more H2 sections. A prompt with H2 sections walks them top to bottom in fall-through order. A prompt with no H2 sections executes the H1 blocks and returns the model reply. The H1 region always runs first, resolving tools and models before any section begins.

#### Minimal Prompt File

```markdown
---
name: hello
description: A greeting prompt
promptforge: 1
---

# Hello

## Greet

Say hello to the user in a friendly tone.
```

The parser compiles Lua code at parse time. A successfully parsed prompt is syntactically executable without any runtime compilation step - Lua syntax errors surface before any network call is made.

#### Structural Rules

The parser enforces strict structure:

- When H2 sections are present, the first and every root heading must be exactly H2.
- Sibling section names must be unique; duplicates produce a diagnostic naming both heading locations.
- Orphan deep headings (H4 under H2 with no H3) are rejected rather than silently reparented.
- Unknown frontmatter fields are rejected so misspelled keys fail loudly.
- Sections nest recursively using heading levels H2 through H6.
- Executable Lua fences must use exact unindented triple-backtick `lua` openers. Longer markers, indentation, or extra info-string words remain inert prose.

Parse errors report stable kind discriminants and optional byte spans for editor diagnostics. Lua compilation errors include absolute source-line numbers that map back to the original prompt file.

#### Optional Frontmatter Fields

- `max_tool_iterations` - integer between 1 and 1000 (default: 24)

---

### Execution Model

Execution is a free function call over caller-owned resources. There is no process-global state. The caller owns the prompt, the execution id, the tool picker, the tool catalog, the model catalog, the store, and the observer.

```rust
use promptforge_core::{run, Prompt, RunConfig, StoreRef, ResolutionContext};
use promptforge_core::tools::ToolCatalog;

let prompt = Prompt::parse(source, "my-execution", &observer)?;
let tool_catalog = ToolCatalog::new(&tools)?;

let result = run(
    &prompt,
    "user input here",
    ResolutionContext::new(&picker, &models, &tool_catalog),
    &StoreRef::memory(),
    RunConfig::new("my-execution"),
).await?;
```

The run resolves the H1 block once, then walks H2 sections top to bottom. A section falls through to the next when its Lua does not return a value. An explicit return stops fall-through. When execution falls off the last section, the result is the last model reply, then the generic string "done".

#### Run Configuration

`RunConfig` uses a builder pattern:

```rust
RunConfig::new("execution-id")
    .observer(my_observer)
    .debug(my_debug_capture)
    .client(gateway_client)
    .cancel(cancel_handle)
    .limits(run_limits)
```

All builder methods are optional. Without `.client()`, the runtime lazily constructs one from environment variables.

#### Run Limits

Configurable limits cap resource consumption:

```rust
RunLimits::new()
    .max_tool_iterations(NonZeroU32::new(24).unwrap())        // model round-trips per section
    .max_fanout_concurrency(NonZeroUsize::new(8).unwrap())    // parallel arms
    .max_response_bytes(NonZeroU64::new(16 * 1024 * 1024).unwrap())
    .lua_memory_bytes(NonZeroUsize::new(64 * 1024 * 1024).unwrap())
    .lua_log_events(NonZeroU32::new(1024).unwrap())
    .request_timeout(Duration::from_secs(120))
```

---

### Lua Scripting

A prompt is built from alternating Lua and prose blocks. Each section can contain any number of Lua blocks interleaved with prose segments. The last prose block in a section runs a full tool-call loop; earlier prose blocks run single-shot (one model round, then control continues to the next Lua block).

Preamble, prologue, and epilog are positions, not phases: the preamble is the H1 region, the prologue is a section's Lua before its first prose, and the epilog is Lua after the last prose.

#### The H1 Preamble

Lua blocks in the H1 region execute once in source order before any H2 section. The preamble declares tools and models, sets variables, and can short-circuit the entire run:

````markdown
# My Prompt

```lua
models.default("writer", "a capable writing model")
tools.bind("search", "web search capability")
tools.always("search")
var.topic = "Rust async patterns"
```

## Write

Write an article about {{ var.topic }}.
````

Returning a scalar value (string, integer, number, or boolean) from H1 skips all H2 sections and becomes the run result.

#### Shared Libraries

A `lua shared` fence in the H1 defines a reusable library compiled once and replayed into every section VM as its first chunk:

````markdown
```lua shared
function summarize(text)
    return "Summary: " .. text
end
```
````

The replay runs with the full section environment installed (`args`, `sys`, `var`, `reply`, `store`, `log`, the `tools`/`models` tables, and the control globals), so top-level shared code may use them at load. Two exclusions apply: the captured tool/model alias globals install only after the replay (a declared alias wins over a same-named shared global), and `jump` during the load is a hard error. A scalar top-level return is discarded - the replay loads a library, it does not produce the section's result.

#### Section Environment

Each section VM provides these globals:

| Global | Purpose |
|--------|---------|
| `args` | Input string passed to the run |
| `sys` | Sealed read-only runtime metadata |
| `var` | Writable data bridge, persists across sections |
| `store` | Virtual filesystem |
| `tools` | Tool scope and call counts |
| `log` | Diagnostic checkpoint function |
| `reply` | Previous section's model answer |

The `sys` table includes `when`, `now`, `id`, `section_name`, `execution`, `section_count`, `model` (after first model interaction), and `reply_finish_reason` (after inference). It is sealed - writes raise errors and the metatable cannot be replaced. `sys.id` is the run-global execution-unit counter: H1 keeps id 0, and every section entry and every fanout arm takes the next value. A fanout arm also carries `sys.index`, its 1-based position within the current fanout; `sys.index` is absent outside fanout.

`var` is the walk-local clipboard: writes persist across sections on the same walk (H1 included), and `execute`/`fanout` clone it in and discard it out - child writes never reach the caller. `var` holds JSON data only; a non-JSON value fails at the assigning line. Bare globals (`x = 42` without `local`) are section-local scratch, visible to prose as `{{ x }}`.

#### Template Substitution

Prose blocks support `{{ path }}` template substitutions. The sources are `args`, `reply`, `var`, `sys`, `item` (fanout arms only), and bare globals - a section-local Lua global resolves as `{{ x }}`, with dotted paths indexing into its JSON form:

````markdown
## Research

```lua
var.query = "latest Rust async runtimes"
```

Search for {{ var.query }} and summarize the results for {{ args }}.
The previous section said: {{ reply }}
Current item: {{ item }}
Run id: {{ sys.id }}
````

Escape literal delimiters with backslash: `\{{` emits `{{`.

#### Control Flow

`jump(target)` transfers control to another section by heading name, clearing conversation context. The current `reply` value is preserved across the jump. Clear it explicitly with `reply = nil` before jumping when the target should not inherit the previous reply. `execute(target, input)` runs a section as a subroutine with a fresh VM and conversation, returning that section's reply:

````markdown
## Router

```lua
local result = execute("## Research", "find Rust crates for HTTP")
var.research = result
jump("## Synthesize")
```

## Research

Research the topic: {{ args }}

## Synthesize

Using this research: {{ var.research }}

Write a summary.
````

`execute()` nests up to 8 levels deep, and the count accumulates across `fanout` boundaries. A chain starts with `reply` set to nil - pass context through the `input` parameter instead - and with a clone of the caller's `var`. A `jump()` inside a chain moves within the chain, and a `return` inside a chain ends the chain, not the run. Sections are referenced by heading string.

#### Sandbox Constraints

The Lua sandbox provides only `string`, `table`, and `math` standard libraries. Dangerous globals (`load`, `dofile`, `require`, `print`, `rawget`, `rawset`, `collectgarbage`) are removed. A runaway Lua block is automatically aborted after exceeding the instruction budget (approximately 10 million instructions). Per-VM memory ceiling defaults to 64 MiB. The `log()` function accepts messages limited to 256 Unicode scalars with no newlines or control characters.

Tool and model aliases must match `[A-Za-z][A-Za-z0-9_-]{0,63}`.

---

### Models

Models are declared by capability description and resolved semantically against a model catalog at runtime.

#### Declaring and Binding

```lua
-- Declare a model by what you need it to do
models.bind("writer", "a creative writing model", {
    thinking = true,
    temperature = 0.7,
    context = 128000,
    max_tokens = 4096
})

-- Set it as the prompt-wide baseline
models.default("writer")
```

The `models.default(alias, description, opts)` form declares and designates in one atomic call; the single-alias form designates a model already declared with `models.bind`. Within sections, `models.use(alias)` selects a specific model and returns its handle:

```lua
local analyst = models.use("analyst")
```

Sections without `models.use` inherit the `models.default` baseline. A prompt can carry both - the baseline applies everywhere a section does not override it. Sections with non-empty prose but no model binding receive a clear error.

#### Hard Constraints

The opts table filters the catalog before semantic resolution:

- `thinking` - boolean, required or forbidden
- `context` - minimum context window (positive integer)
- `temperature` - float in range 0.0 to 2.0
- `max_tokens` - positive integer

Duplicate model aliases or duplicate `models.default` calls are rejected atomically. `models.use` may be called at most once per section.

#### Model Inference from Lua

`infer` has one shape: a single tool-free inference round on a fresh conversation. It never sets `reply` and never touches `sys`. Two forms exist:

```lua
-- The section's current model (the models.use selection, else the models.default baseline)
local tag = models.infer("One-word sentiment of: " .. args)

-- Any declared model, via its handle
local critic = models.get("critic")
local review = critic:infer("Critique this draft: " .. reply)
```

`models.get(alias)` returns the handle for a declared model without changing the section's model selection, so `handle:infer` is the way to consult a different model inside a section. A Lua block that needs tools uses `execute` on a section instead.

#### Inspecting Model Properties

After binding, a model handle's frozen properties are accessible from Lua: `name`, `model_id`, `description`, `context`, `thinking`, `temperature`, `max_tokens`, and `dialect`.

#### Catalog and Dialects

The library fetches a live model catalog from a gateway's `GET /v1/models` endpoint with bearer authentication. The caller provides a model catalog built from descriptors with identity, description, context window, and thinking mode (Always, Switchable, or Never).

Two tool-calling dialects ship: OpenAI (native tool calls) and Gemma-3 tool_code (emulated via content fences). Dialect resolution is automatic from model catalog evidence - endpoint capabilities, chat template markers, model id, and source provenance.

---

### Tools

#### Declaring Tools

Tools are declared by capability description and resolved semantically at runtime via a picker:

```lua
-- Declare a tool binding
local search = tools.bind("search", "web search capability")

-- Promote to prompt-wide scope (available in all sections)
tools.always("search")
```

A tool declared with `tools.bind` is not exposed to the model unless `tools.always` or `tools.add` is called. This is explicit - you control exactly what the model sees.

```lua
-- Section-local scoping
tools.add("search")            -- by alias string
tools.add(search)              -- by handle object
tools.add({"a", "b", tool_c}) -- arrays of strings or handles
```

`tools.add` calls are atomic: a failure rolls back all entries. An empty add is a no-op.

#### Tool Properties

After `tools.bind`, the returned handle exposes: `name`, `description`, `parameters` (JSON schema), `wire_name`, and `untrusted` flag. Tool objects are frozen - assigning a field errors. The model-facing description is overridden positionally at declaration or scoping time:

```lua
tools.bind("search", "web search capability", "Search the web for current information")
tools.always("search", "Search the web for current information")
tools.add("search", "Search the web for current information")
```

Precedence is `add` over `bind`/`always` over the catalog description.

#### Tool Dispatch Loop

The tool loop runs the model in a cycle: dispatch tool calls, feed results back, re-prompt until the model produces a final text reply or the iteration cap is reached (default 24 rounds, configurable via `max_tool_iterations` in frontmatter).

#### Tool Safety

Untrusted tool output is wrapped with a CSPRNG nonce envelope before reaching the model, preventing prompt injection. One nonce per run; envelopes are deterministic within a run. Trusted tool output passes verbatim. Trust marking is mandatory at construction time.

Near-duplicate tools in the same section scope are detected and rejected before any model call, with similarity diagnostics. Out-of-scope tool calls produce a clear error distinguishing globally-declared-but-unscoped tools from truly unknown ones.

#### Tool Call Counts

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

Counts increment even when a tool call fails. Mistyped aliases produce a hard error with the available scope listed.

#### Local Tools

`tools.add_local(alias, description, params, handler)` declares a tool backed by a Lua function, available from any H2 Lua block. When the model calls the tool, the handler runs synchronously in the declaring section's VM rather than reaching an external service:

```lua
tools.add_local("grab", "Grab a value from the store", {
    key = {"string", "Store path to read"},
}, function(args)
    return store.read(args.key)
end)
```

The params table maps each parameter name to a bare type string or a `{type, description}` array. Supported types are `"string"`, `"integer"`, `"number"`, and `"boolean"`; all declared parameters are required. The handler receives the arguments as a Lua table and returns a string. It shares the section's VM (store, `var`, globals), may call `execute()`, `fanout`, and the `infer` forms (`models.infer(prompt)`, `handle:infer(prompt)`), and cannot call `jump()`. Local tool output is trusted - no nonce envelope. A local tool becomes visible to the model starting from the next prose block.

#### Implementing Custom Tools

A custom tool requires:

- A stable `ToolId` (server + name pair)
- A wire name matching `[A-Za-z0-9_.-]`
- A description string
- A JSON-Schema parameters definition
- An async `call` method returning `ToolOutput` (marked trusted or untrusted)

Tools can run locally in-process or proxy through a remote gateway, both dispatched uniformly through the `Tool` trait.

#### Built-in Web Search

The web search tool sends queries through a gateway proxy so the search provider credential never leaves the server. Results are automatically marked as untrusted output. Parameters include count (1-20), freshness filter (pd/pw/pm/py), SafeSearch level (off/moderate/strict), domain inclusion/exclusion lists (up to 20 each), country code, and language code.

---

### Fanout

`fanout(worker, collection)` maps a worker section over a collection in parallel. Each member is processed by its own isolated execution arm with a fresh Lua VM.

````markdown
## Process

```lua
local results = fanout("### Worker", list_from_section("### URLs"))
var.output = table.concat(results, "\n\n")
```

### Worker

Fetch and summarize: {{ item }}

### URLs

- https://example.com/page1
- https://example.com/page2
- https://example.com/page3
````

The worker is referenced by markdown heading address (level + name). The second parameter is always a collection, never a section name: any Lua table works, and `list_from_section("### List")` feeds a list section's pre-parsed items straight in. An empty collection is an error - no work is likely a bug.

#### Arm Execution

Each arm receives the current member as the `item` variable, a `sys.index` giving its 1-based position within the current fanout, and a unique run-global `sys.id`. Each arm starts with a fresh clone of the caller's `var` - arm writes to `var` never reach the caller. The arm can:

- Run Lua blocks that short-circuit before any prose (enabling pure-Lua map operations)
- Substitute `{{ item }}` in prose
- Run the full model tool loop
- Run Lua blocks after the prose for post-processing

Results are returned in collection order (not finish order). Each result has `.text`, `.ok`, `.item`, and `.exhausted` fields; an arm that produces no reply yields `.text == ""` with `.ok == true`. The result array supports `table.concat` since objects coerce via `__tostring`. All arms share the run's store: two arms of one fanout writing the same path is a hard error (a write-write race), while `store.append` from concurrent arms stays legal.

#### Resilience

An exhausted arm (tool loop budget exceeded) soft-degrades into an incomplete stub rather than failing the entire fanout. A fatal error in any arm aborts all sibling arms, preventing wasted work. Cancellation propagates from the parent into each spawned arm cooperatively.

Default concurrency is 8 parallel arms, configurable via `RunLimits`.

---

### Store

The store is a run-scoped virtual filesystem shared across all sections. Data persists within a single run and the handle is thread-safe across concurrent tasks.

```lua
store.write("notes/summary.md", "# Summary\n" .. reply)
store.append("log.txt", "processed: " .. args .. "\n")

local content = store.read("notes/summary.md")
local numbered = store.read_numbered("notes/summary.md")

store.str_replace("notes/summary.md", "old text", "new text")

local files = store.glob("notes/*.md")
local exists = store.exists("notes/summary.md")

store.delete("notes/summary.md")
```

`store.delete` on a missing path is silent - delete is idempotent. Within a single `fanout`, two arms calling `store.write` on the same path is a hard error (a write-write race); `store.append` from concurrent arms stays legal.

#### Safe Injection

Wrap stored content in the untrusted-input guard envelope with the `untrusted(s)` global before re-injecting it into model prompts: `untrusted(store.read(path))`. Forged close-tags in stored content are escaped, so injected data cannot break out of the envelope:

```lua
store.write("user-data.txt", user_provided_content)
-- Later, safely inject into a prompt context:
local safe = untrusted(store.read("user-data.txt"))
```

#### Path Validation

All store paths are validated:

- Forward-slash only (backslash rejected)
- No path traversal (`.` and `..` segments rejected)
- No Windows reserved device names (CON, NUL, COM1-9, LPT1-9)
- No trailing dots or spaces
- Maximum 1024 bytes

#### Glob Matching

- `*` matches within a single path segment
- `**` matches across path separators
- Unsupported syntax (backslash escapes, triple-star, misplaced `**`) is rejected
- Matching uses a bounded, non-backtracking algorithm

The `str_replace` operation requires the old text to be unique in the file; ambiguous matches are refused with a count of occurrences.

The default in-memory backend (`StoreRef::memory()`) requires no filesystem or network and drops cleanly with the run. Custom backends implement the `Store` trait.

---

### Gateway Client

The gateway client sends requests to an OpenAI-compatible chat completions endpoint with bearer authentication.

#### Configuration

Set two environment variables:

```bash
export PROMPTFORGE_GATEWAY_URL="https://your-gateway.example.com"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
```

Or construct programmatically:

```rust
let client = GatewayClient::new(endpoint, key);
```

Point `PROMPTFORGE_GATEWAY_URL` at a local server or another gateway to retarget. The credential is automatically redacted in Debug output, Display, and logs. Empty credentials are rejected at construction time.

Gateway URLs are validated: non-HTTP schemes, embedded credentials, query strings, and fragments are rejected. Trailing slashes are normalized.

For testing, `GatewayClient::disabled()` creates a client that always returns a Disabled error.

---

### Observation and Debugging

#### Observer

The observer is a pluggable, report-only seam for watching execution in flight. Implement the `Observer` trait:

```rust
fn observe(&self, execution: &str, section: &str, event: Observation<'_>);
```

Events include parse started/completed, run started/succeeded/failed, section started/finished, model turn completed/truncated, tool call succeeded/failed, store operations, fanout arm lifecycle, and Lua log checkpoints. All observations are correlated by execution id and section name.

`NullObserver` discards all events when no tracing is needed. Attaching or detaching an observer does not change execution results.

#### Debug Capture

A separate debug sink records raw request and response JSON for each model turn:

```rust
fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);
```

Debug events capture the full request body as JSON and the response finish reason with reasoning content. Events from nested `model:infer` calls and fanout arms are forwarded to the same sink.

#### Cancellation

Cancellation is cooperative via a caller-supplied `CancelHandle`. It propagates into tools, models, Lua instruction hooks, and fanout arms. A cancelled run returns a `RunError` with `is_cancelled() == true`, distinguishable from faults.

---

### Error Handling

Every public boundary returns its own typed error rather than one crate-wide error type. Each error exposes a stable `kind()` classifier for programmatic handling without matching on private representations. Public structs are `#[non_exhaustive]` so they evolve without breaking downstream code.

| Error | Kinds | Queries |
|-------|-------|---------|
| `RunError` | Parse, Version, Binding, Completion, Tool, Store, Lua, Quota, Substitution, Cancelled, Internal | `is_retryable()`, `is_cancelled()` |
| `CompletionError` | Transport, Backend, MalformedResponse, EmptyReply, Disabled, Config | `is_retryable()`, `is_timeout()`, `status()` |
| `StoreError` | NotFound, Anchor, InvalidAnchor, InvalidPath, InvalidPattern, Backend | `is_not_found()`, `path()` |
| `ToolError` | InvalidArguments, Backend, Transport, Cancelled, Other | `is_retryable()`, `is_cancelled()` |
| `ParseError` | (by kind) | `kind()`, `span()` |
| `DialectError` | NoMatch, Tie, Unknown | `kind()` |

Backend error bodies are accessible through opt-in accessors but never leak into Display output.

`promptforge_version(source)` detects whether a file is a promptforge prompt without requiring a full parse - it needs only the `promptforge:` key.

---

## promptforge-mcp-server

promptforge-mcp-server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. It puts a prompt catalog behind four fixed MCP tools rather than publishing each prompt as its own tool, which means `tools/list` never changes and a prompt saved ten seconds ago is callable with no reconnect. You point it at a `prompts.toml`, it resolves your prompts, connects to a gateway, and serves - over HTTP with bearer auth, or over stdio for a local spawn.

### Starting the Server

````sh
promptforge-mcp-server serve prompts.toml
````

This binds the streamable-HTTP transport at `http://127.0.0.1:9310/mcp`. Every request to `/mcp` must carry an `Authorization: Bearer <token>` header matching `[server].api_key`.

For a harness that spawns the server as a child process:

````sh
promptforge-mcp-server serve --stdio prompts.toml
````

Stdio speaks JSON-RPC over standard input and output, binds no port, and ignores `[server].api_key` entirely. Logs go to stderr so they do not corrupt the wire.

### Configuration

A single `prompts.toml` carries everything the server needs. The minimal configuration:

````toml
[server]
api_key = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"
````

Every string value supports `${VAR}` interpolation from the process environment. Use `$$` for a literal dollar. An unset variable fails the load everywhere except `[server].api_key`, where it drops the key silently so a stdio install can boot without a credential its transport never reads.

#### Full configuration

````toml
[server]
bind = "127.0.0.1:9310"
api_key = "${PROMPTFORGE_MCP_SERVER_API_KEY}"
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
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["_*.md", "drafts/**"]

[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
````

#### Defaults

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

#### Sections

**`[server]`** - Bind address, shared bearer key, concurrency limits, timing, and reload settings. `allowed_hosts` controls DNS-rebinding protection: on a loopback bind an empty list defaults to `localhost`, `127.0.0.1`, `::1`; a non-loopback bind with no hosts is refused.

**`[paths]`** - The prompts directory. Catalog patterns and `[prompts.NAME].file` paths are both relative to it.

**`[gateway]`** - The model gateway every run goes through. `url` must be a valid http/https URL with a host. `api_key` is the bearer credential sent on every model call.

**`[catalog]`** - Glob patterns that assemble the catalog. `include` names what to resolve; `exclude` subtracts from it. `*` does not cross a separator, `**` does.

**`[prompts.NAME]`** - Per-prompt overrides keyed by the prompt's frontmatter name. Set `enabled = false` to drop one the globs caught. Set `file = "path.md"` to add a file no glob matches. The key must match the prompt-name shape: `^[a-z][a-z0-9_]{0,47}$`.

### The Tool Surface

The server publishes a fixed set of built-in tools. No prompt appears in `tools/list` - a prompt is reached only by naming it to `run_prompt`.

| Tool | Purpose |
|------|---------|
| `list_prompts` | Report every enabled prompt: name, description, and any problem stopping it |
| `run_prompt` | Execute a named prompt and return its artifact |
| `check_run` | Collect a run that outlived its call |
| `need_prompt` | Discover prompts by semantic similarity (requires `picker` feature) |

The `picker` feature is on by default. Without it the server publishes three tools and `need_prompt` is absent. A build without `picker` is smaller and removes the embedding model weights.

### Running a Prompt

Call `run_prompt` with `prompt` (required) and `args` (optional):

````json
{
  "prompt": "research_person",
  "args": "Herb Sutter, ABI stability positions"
}
````

#### What happens

1. **Name resolution** - The name is matched case-normalized against the catalog. An unresolvable name returns all enabled names nearest-first so the model can correct itself.

2. **Admission** - The call waits for one of `max_concurrent_runs` slots. If none comes free within `admission_timeout`, the call gets a retryable refusal: "every run slot is busy and none came free within 30s. Retry in a moment."

3. **Execution** - The prompt runs against the gateway. Progress notifications stream to the client if it supplied a `progressToken`.

4. **Reply deadline** - If the run finishes in time, the result comes back inline. If it exceeds `reply_deadline`, the call returns immediately with status `running` and a `run_id`.

#### Background runs

A run that outlives its call continues in background. Collect it with `check_run`:

````json
{
  "run_id": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6"
}
````

A finished run stays collectable for `retain_completed` (default 1 hour), then is evicted.

If the client disconnects while a run is in progress, the run is cancelled cooperatively.

#### Result format

Every result carries structured content:

````json
{
  "run_id": "a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6",
  "prompt": "research_person",
  "status": "completed",
  "value": "The full artifact text...",
  "turns": 3,
  "elapsed_ms": 42000,
  "error": null
}
````

Status is one of `running`, `completed`, or `failed`. A completed run carries `value`; a failed run carries `error`; a running run carries neither.

### Discovering Prompts

#### list_prompts

Browse the catalog with optional pagination:

````json
{ "cursor": "100" }
````

Returns up to 100 entries per page:

````json
{
  "prompts": [
    { "name": "research_person", "description": "Build a stakeholder profile...", "problem": null },
    { "name": "broken_one", "description": "", "problem": "parse error at line 3" }
  ],
  "next_cursor": "200"
}
````

A broken prompt appears in the listing with its problem visible, so the operator knows what to fix.

#### need_prompt

When you have a capability description rather than a name:

````json
{ "capability": "Build a stakeholder position report for one entity." }
````

Returns up to three candidates ranked best-first:

````json
{
  "prompts": [
    { "name": "research_person", "description": "Build a stakeholder profile..." },
    { "name": "staker", "description": "Assess positions on a proposal..." }
  ]
}
````

State the capability the way a tool author would document it: an imperative phrase naming the operation and what it acts on. Conversational phrasing resolves less reliably.

If retrieval is unavailable (model failed to load), `need_prompt` reports it and points you at `list_prompts` instead.

### Live Reload

With `watch = true` (the default), saving a prompt file or `prompts.toml` triggers a re-resolution after the debounce window settles. The catalog and its retrieval index are published together as one atomic generation - no reader ever sees a torn pair.

What reload does:

- A healthy edit updates the catalog immediately. The tool list stays the same because tools are fixed; only the catalog behind `run_prompt` changes.
- A broken edit (parse error, bad name) retains the prompt as a listed entry carrying its problem rather than freezing the whole catalog.
- An edit to a prompt's body alone (no name or description change) carries the previous retrieval index forward without rebuilding it.
- A broken platform watch is re-registered on the next settled window rather than permanently losing live reload.

Set `watch = false` to serve a static catalog for the life of the process.

### Transport and Security

#### HTTP

The streamable-HTTP transport puts MCP at `/mcp` and a liveness probe at `/healthz`. The bearer check wraps `/mcp` only - `/healthz` is unauthenticated by design.

Authentication is per-request, not per-session. The token is fixed for the life of the server, but the check happens on every HTTP request rather than once at initialization - so a session that already completed the MCP handshake is still refused if its credential does not match. The comparison is constant-time.

SSE keep-alive is 15 seconds, so a run that thinks between sections does not look dead to a proxy.

`allowed_hosts` is the DNS-rebinding defence. On a loopback bind it defaults to `localhost`, `127.0.0.1`, `::1`. On a non-loopback bind you must enumerate the authorities explicitly or the server refuses to start.

#### Stdio

Stdio binds no port, checks no token, and has a bounded line reader so a peer without newlines costs a fixed buffer rather than the process. The harness that spawned it is the only thing that can talk to it.

#### Shutdown

Ctrl-C triggers graceful shutdown on both transports. The SSE streams are closed, in-flight calls drain, and the watcher stops before the process exits. No late reload can publish after the shutdown signal.

### Boot Sequence and Gateway

At startup the server:

1. Loads and validates `prompts.toml`
2. Resolves the catalog (refuses to start on any fault)
3. Builds the retrieval index over the catalog (if `picker` feature is present; a failure is logged and the server continues)
4. Fetches the gateway model catalog via `GET /v1/models`
5. Builds the live tool catalog (`web_fetch`, `web_search`) and the semantic tool picker
6. Starts the filesystem watcher
7. Serves the chosen transport

The gateway fetch distinguishes transient failures from fatal ones. A connection timeout or a 5xx is transient: the server warns and serves with an empty model catalog, so prompts without `models.bind` keep working. A 401, a bad URL, or a malformed response is fatal: the server refuses to boot rather than hiding a misconfiguration behind runtime failures.

---

## promptforge-tool-picker

A sentence-embedding resolver that turns "read a file from disk" into the tool that does it - no LLM call, no network, no guessing. You describe your tools in prose, build a picker over the catalog, and ask it which tool a need refers to. It answers with a decision: one bound tool, a duplicate report, an ambiguous shortlist, or an abstention. The model is compiled into the library, so there is no path to configure and no weights to ship. Querying is a dot product, not an API call. Determinism is structural: the same inputs always produce the same answer.

### Identity, Descriptors, and Catalogs

Every tool is identified by a `(server, name)` pair. The pair is structural - never concatenated - so a server or name containing any delimiter stays unambiguous.

````rust
use promptforge_tool_picker::{ToolId, ToolDescriptor, ToolAnnotations, Catalog};
use serde_json::json;

let id = ToolId::new("files", "read_file");
assert_eq!(id.server(), "files");
assert_eq!(id.name(), "read_file");
````

A `ToolDescriptor` carries the identity, a prose description, a JSON Schema for the tool's arguments, and optional behavioral hints:

````rust
let tool = ToolDescriptor::new(
    ToolId::new("files", "read_file"),
    "Read a file from disk",
    json!({"properties": {"path": {"type": "string"}}}),
);

let tool = tool.with_annotations(
    ToolAnnotations::new()
        .with_read_only(true)
        .with_destructive(false),
);

assert_eq!(tool.name(), "read_file");
assert_eq!(tool.description(), "Read a file from disk");
assert_eq!(tool.annotations().read_only(), Some(true));
````

Annotations are optional and advisory. They affect ranking only as a tie-break between candidates that score identically. A positive read-only claim is preferred first, then non-destructive, then idempotent.

A `Catalog` is an ordered collection of descriptors:

````rust
let catalog = Catalog::new(vec![
    ToolDescriptor::new(
        ToolId::new("files", "read_file"),
        "Read a file from disk",
        json!({"properties": {"path": {"type": "string"}}}),
    ),
    ToolDescriptor::new(
        ToolId::new("net", "fetch_url"),
        "Fetch a web page over HTTP",
        json!({"properties": {"url": {"type": "string"}}}),
    ),
]);

assert_eq!(catalog.len(), 2);

let found = catalog.get(&ToolId::new("net", "fetch_url"));
assert_eq!(found.map(|t| t.name()), Some("fetch_url"));

for tool in &catalog {
    println!("{}: {}", tool.name(), tool.description());
}
````

You can also build a catalog from an iterator or a `Vec`:

````rust
let catalog: Catalog = vec![/* descriptors */].into();
let catalog: Catalog = some_iterator.collect();
````

With the `serde` feature (enabled by default), catalogs deserialize from JSON. The identity fields are flat on each descriptor. The schema field accepts both `input_schema` and its MCP spelling `inputSchema`:

````json
[
  {
    "server": "files",
    "name": "read_file",
    "description": "Read a file from disk",
    "inputSchema": {
      "properties": { "path": { "type": "string" } }
    },
    "annotations": { "readOnlyHint": true }
  }
]
````

Duplicate identities in a catalog are accepted. Two tools claiming the same identity is a result the engine reports, not an input it refuses.

### Building a Picker

The simplest path loads the model and indexes a catalog in one call:

````rust
use promptforge_tool_picker::{ToolPicker, Catalog, Config};

let picker = ToolPicker::build(catalog, Config::default())?;
assert_eq!(picker.len(), 2);
````

Loading the model is the expensive step - it materializes the compiled-in weights into memory. If you serve several catalogs, load the model once and build each picker against it:

````rust
use promptforge_tool_picker::Model;

let model = Model::load()?;

let files_picker = ToolPicker::build_with_model(&model, files_catalog, Config::default())?;
let weather_picker = ToolPicker::build_with_model(&model, weather_catalog, Config::default())?;
````

`Model` is cheap to clone (it shares the loaded weights through an `Arc`), and it is `Send + Sync + 'static`, so you can pass it across threads.

When your catalog changes - a reconnected server, a watched directory - rebuild from the existing picker to preserve its model and configuration:

````rust
let updated = picker.rebuild(new_catalog)?;
````

The original picker is immutable and still answers from its own catalog. The rebuilt picker answers from the new catalog with the same model and config.

You can iterate a picker's tools with `picker.iter()` or `for tool in &picker`, and look up a specific tool with `picker.get(&id)`.

### Resolving a Need

`resolve` takes a plain-English need and returns one of four outcomes:

````rust
use promptforge_tool_picker::Outcome;

match picker.resolve("read a file from disk")? {
    Outcome::Bind(tool) => {
        println!("call {}", tool.name());
    }
    Outcome::Duplicate(group) => {
        // One server publishes tools that are copies of each other.
        println!("{} publishes {} twins", group.first().server(), group.len());
    }
    Outcome::Ambiguous(group) => {
        // Several tools match well enough that the margin could not separate them.
        for tool in &group {
            println!("candidate: {}/{}", tool.server(), tool.name());
        }
    }
    Outcome::Absent => {
        println!("no tool covers this need");
    }
    _ => {}
}
````

`Absent` is a successful answer, not an error. An `Err` from `resolve` means the need could not be embedded (tokenization or inference failed), so no answer was produced at all.

Results borrow the picker's descriptors. No schema or descriptor is deep-cloned. If you need to keep a tool identity beyond the picker's lifetime, clone the specific `ToolId`:

````rust
let kept_id: ToolId = match picker.resolve("read a file")? {
    Outcome::Bind(tool) => tool.id().clone(),
    _ => return Ok(()),
};
````

A `CandidateGroup` (from `Duplicate` or `Ambiguous`) always contains at least two entries. You can inspect them with `group.first()`, `group.second()`, `group.get(index)`, `group.len()`, and `group.iter()`.

### Shortlisting

`shortlist` returns candidates above the similarity floor without making a final decision, so the caller can choose:

````rust
let candidates = picker.shortlist("read a file from disk", 3)?;

for tool in &candidates {
    println!("{}: {}", tool.name(), tool.description());
}

if candidates.is_empty() {
    println!("nothing relevant");
}
````

`resolve` and `shortlist` never contradict each other on relevance. If `resolve` abstains, `shortlist` returns nothing. If `resolve` binds a tool, `shortlist` offers exactly that tool.

The solo-candidate exception applies to both: when one candidate sits between the solo floor and the strict similarity floor, and no runner-up reaches the solo floor, that candidate is offered.

A `limit` of zero returns an empty shortlist without embedding the need. The `Shortlist` type offers `.len()`, `.is_empty()`, `.first()`, `.get(index)`, and `.iter()`.

### Configuration

`Config::default()` provides justified defaults. A caller who has not measured their own catalog should change none of them:

| Threshold | Default | Meaning |
|-----------|---------|---------|
| `similarity_floor` | 0.825 | Cosine similarity a candidate must reach to be considered |
| `margin` | 0.05 | Gap the leader must clear the runner-up by to bind |
| `duplicate_threshold` | 0.98 | Tool-to-tool similarity at which two tools are treated as twins |
| `solo_floor` | 0.5 | Minimum score for a lone candidate to bind below the strict floor |
| `top_k` | 3 | How many candidates a duplicate or ambiguous outcome reports |

Adjust one threshold at a time with checked consuming setters:

````rust
use promptforge_tool_picker::Config;

let config = Config::default()
    .with_similarity_floor(0.85)?
    .with_top_k(5)?;

assert_eq!(config.top_k().get(), 5);
````

Every `Config` is always valid. Thresholds must be finite and in `0.0..=1.0`. `top_k` must be nonzero. There is no `validate` method because no public operation can produce an invalid value.

A setter that receives an out-of-domain value returns `ConfigError`, which names the rejected field:

````rust
use promptforge_tool_picker::{ConfigError, ConfigField};

let error: ConfigError = Config::default()
    .with_similarity_floor(2.0)
    .expect_err("out of domain");
assert_eq!(error.field(), ConfigField::SimilarityFloor);
````

With the `serde` feature, configuration serializes and deserializes as JSON. Absent fields take their defaults, and checked deserialization rejects invalid wire values:

````json
{"similarity_floor": 0.85, "top_k": 5}
````

**Decision precedence** is fixed: absent, then duplicate, then bind, then ambiguous. The similarity floor is checked first. Then same-server twins are detected against the duplicate threshold (measured between the tools' own embeddings, not against the query). Then the margin test separates a clear leader from a near-tie. Every threshold boundary is inclusive - a score exactly at the floor is considered.

**The solo-candidate rule:** when the top candidate scores at or above `solo_floor` but below `similarity_floor`, and no runner-up reaches the solo floor, the leader binds. Two candidates between the floors abstain.

### Near-Duplicate Detection

`near_duplicates` compares selected tools against the configured duplicate threshold using the picker's stored embeddings. The comparison is tool-to-tool, not need-to-tool - it measures how alike two tools' own descriptions are, independent of any query.

````rust
let pairs = picker.near_duplicates(&[
    ToolId::new("calendar", "create_event"),
    ToolId::new("calendar", "add_event"),
])?;

for pair in &pairs {
    println!(
        "{}/{} and {}/{} are {:.3} similar",
        pair.first().server(), pair.first().name(),
        pair.second().server(), pair.second().name(),
        pair.similarity(),
    );
}
````

Every requested identity must be present in the picker. An absent identity returns `SelectionError` before any comparison happens, naming the first missing `ToolId` via `error.missing_id()`. Repeated identities are idempotent set membership.

Pairs are output in catalog pair order. Each `NearDuplicate` provides `.first()`, `.second()`, and `.similarity()`. The `NearDuplicates` collection provides `.len()`, `.is_empty()`, `.get(index)`, and `.iter()`.

### Error Handling

Each fallible operation returns its own error type. There is no crate-wide error enum.

| Operation | Error Type | Key Accessor |
|-----------|-----------|--------------|
| `Model::load` | `ModelLoadError` | - |
| `ToolPicker::build` | `BuildError` | - |
| `ToolPicker::build_with_model` | `IndexError` | - |
| `ToolPicker::resolve` / `shortlist` | `QueryError` | `.kind()` |
| `ToolPicker::near_duplicates` | `SelectionError` | `.missing_id()` |
| `Config::with_*` | `ConfigError` | `.field()` |

`QueryError::kind()` returns a `QueryErrorKind` that classifies the failure without exposing dependency types:

````rust
use promptforge_tool_picker::{QueryError, QueryErrorKind};

match error.kind() {
    QueryErrorKind::Tokenization => { /* the need text could not be tokenized */ }
    QueryErrorKind::Inference => { /* the model's forward pass failed */ }
    QueryErrorKind::InvalidEmbedding => { /* the produced vector could not be normalized */ }
    _ => {}
}
````

`BuildError` wraps either a `ModelLoadError` or an `IndexError`, and implements `From` for both. All error types are `Send + Sync + 'static`.

### Determinism and the Embedded Model

The crate promises deterministic results: the same model bytes, dependency versions, target, execution environment, catalog, configuration, and need always produce the same outcome. Cross-platform byte-identical vectors at floating-point boundaries are not promised.

The embedding model (BAAI/bge-small-en-v1.5, 384 dimensions) is compiled into the library. There is no model path in the configuration, no weights file to deploy, and no network call at runtime. The build script fetches the model from the Hugging Face Hub at a pinned immutable commit, verifies every file against a hardcoded SHA-256 digest, and downcasts the fp32 weights to fp16 to halve binary size. Subsequent builds reuse the Hugging Face cache.

At load time, the crate verifies that the embedded weights' provenance metadata matches the pinned repository and revision. A mismatched or substituted checkpoint fails loudly rather than silently altering rankings.

The first build requires network access to the Hugging Face Hub (about 130 MB download). Set `HF_HUB_CACHE` or `HF_HOME` to point at an existing cache, or `HF_ENDPOINT` to a reachable mirror.

---

## promptforge-webfetch

Hand a language model one tool and let it read the web. `promptforge-webfetch` fetches a URL, extracts the useful content, and returns it as markdown the model can cite - while enforcing an SSRF boundary that prevents the model from reaching your internal network no matter what URL it supplies. The common call is one argument (`url`). The security is layered and runs at DNS-resolution time on every hop, so it catches names that resolve inward, rebinding attacks, and redirect chains that point somewhere they should not. By the end of this guide you will know how to wire the tool into a promptforge pipeline, tune its policy for your deployment, and trust it with model-supplied URLs.

### Fetching a Page

Construct the tool and call it with a URL:

````rust
use promptforge_webfetch::WebFetch;
use promptforge_core::tools::Tool;

let tool = WebFetch::new();
let output = tool.call(serde_json::json!({ "url": "https://example.com/article" })).await?;
println!("{}", output.text());
````

The tool accepts one required argument (`url`) and two optional ones (`raw` and `max_chars`). It performs a GET, classifies the response by content type, and returns the text behind a provenance header:

````text
url: https://example.com/article
truncated: false
extraction: readability

# Article Title

The main content rendered as markdown...
````

The three header fields are a contract:

- **url** - the final URL after any redirects, so the model knows where its text came from
- **truncated** - whether the text was cut short by a size cap
- **extraction** - which of three processing paths produced the output: `readability` (article isolation), `raw-html` (whole-page render), or `plain` (non-HTML text returned verbatim)

### How Content Is Processed

The response's `Content-Type` header decides the route before the body is downloaded:

**HTML** (`text/html`, `application/xhtml+xml`): The main article is isolated with a readability algorithm and rendered to markdown. If the extracted article is shorter than 100 characters, the whole page is rendered instead, automatically. The `extraction:` header tells you which path fired.

**Structured text** (`application/json`, `application/xml`, `text/xml`, and any `+json`/`+xml` suffix): Returned verbatim as decoded text. No extraction, no transformation.

**Flat text** (all other `text/*`): Returned decoded. If it exceeds the byte cap, the prefix is kept and `truncated: true` is set.

**Everything else** (PDF, images, audio, video, `application/octet-stream`): Refused with a message naming the content type so the model can try a different URL.

**No Content-Type**: Refused. The tool does not sniff.

Use `raw` when article extraction would discard the content you want - for example a page that is mostly a data table:

````rust
let output = tool.call(serde_json::json!({
    "url": "https://example.com/pricing",
    "raw": true
})).await?;
````

This forces whole-page rendering and reports `extraction: raw-html`. Ignored for non-HTML responses.

Responses compressed with gzip or brotli are decompressed transparently.

### Size Limits and Truncation

Two caps govern how much data the tool accepts:

- **Byte cap** (`max_bytes`, default 8 MiB): the largest decompressed response body. A declared `Content-Length` over this cap is refused before any bytes are read. A streaming body that crosses it mid-read is aborted.
- **Character cap** (`max_chars`, default 40,000): the longest text returned to the model. Text is cut on a character boundary so multibyte characters are never split.

The two caps interact differently depending on the content type:

| Route | Body over byte cap | Text over char cap |
|---|---|---|
| HTML | Refused (incomplete HTML is invalid) | Truncated, flagged |
| Structured (JSON, XML) | Refused (truncated prefix is invalid) | Truncated, flagged |
| Flat text | Truncated at byte cap, flagged | Truncated at char cap, flagged |

A per-call `max_chars` argument lets the model request less text for one call:

````rust
let output = tool.call(serde_json::json!({
    "url": "https://example.com/long-page",
    "max_chars": 5000
})).await?;
````

The per-call value is clamped to the configured ceiling - a model cannot request more than the policy allows, only less.

### Customizing the Security Policy

The default policy (`WebFetch::new()`) is safe for fetching the public internet: HTTPS only, ports 80 and 443, no bare IP-literal URLs, every non-globally-reachable address blocked. Customize it through the builder:

````rust
use std::time::Duration;
use promptforge_webfetch::{FetchConfig, WebFetch};

let policy = FetchConfig::builder()
    .allow_http(true)
    .allow_ports([80, 443, 8080])
    .max_bytes(16 * 1024 * 1024)
    .max_chars(100_000)
    .timeout(Duration::from_secs(60))
    .user_agent("my-service/1.0")
    .build()?;

let tool = WebFetch::try_with_config(policy)?;
````

Every setter returns `self` for chaining. Validation happens once at `.build()`, which returns `ConfigError` for any invalid field. The available knobs:

| Knob | Default | Ceiling | Notes |
|---|---|---|---|
| `allow_http` | `false` | - | Whether `http://` URLs are permitted |
| `allow_ports` | `[80, 443]` | - | Replaces the port allowlist |
| `allow_ip_literals` | `false` | - | Grants literal syntax only; address still classified |
| `deny_cidr` | (none) | - | Adds a blocked CIDR range (can call multiple times) |
| `allow_host_address` | (none) | - | Exact escape hatch (see next section) |
| `max_redirects` | `5` | `20` | Zero refuses all redirects |
| `max_bytes` | 8 MiB | 64 MiB | Must be >= 1 |
| `max_chars` | `40,000` | `10,000,000` | Must be >= 1 |
| `connect_timeout` | 5s | 60s | Must be > 0 |
| `timeout` | 20s | 300s | Must be > 0 |
| `pool_idle_timeout` | 10s | 600s | Must be > 0 |
| `user_agent` | `"promptforge-webfetch/0.0"` | - | Must be a valid HTTP header value |

### Reaching an Internal Host

By default, every non-globally-reachable address is blocked. The only supported way to reach one is an exact host-plus-address pair:

````rust
use std::net::IpAddr;
use promptforge_webfetch::FetchConfig;

let addr: IpAddr = "10.0.5.42".parse()?;
let policy = FetchConfig::builder()
    .allow_http(true)
    .allow_ports([80, 443, 8080])
    .allow_host_address("wiki.internal.corp", addr)
    .build()?;
````

The escape hatch is deliberately narrow:

- Keyed on **both** host and address, so `evil.com` resolving to `10.0.5.42` does not inherit the exception
- Grants access to exactly one address, not a range
- The host is canonicalized (lowercased, trailing dot stripped) so case variants match

You can also block additional ranges for your deployment:

````rust
let policy = FetchConfig::builder()
    .deny_cidr("10.99.0.0/16")
    .deny_cidr("172.20.0.0/14")
    .build()?;
````

### The SSRF Boundary

The tool enforces four layers of defense, in order:

1. **URL admission** (before any network access): Rejects bad schemes, embedded userinfo, non-allowed ports, and bare IP literals that map to blocked addresses. Catches obfuscated IPv4 encodings (`0177.0.0.1`, `2130706433`, `127.1`, `[::ffff:127.0.0.1]`).

2. **Guarded DNS resolver** (at connect time, every hop): Resolves the host, filters the answers through the address policy, hands only the allowed addresses to the HTTP client. A host that resolves entirely to blocked addresses fails. A host with mixed public/private answers connects to the public one. No verdict is cached, so a DNS-rebinding answer is caught on the hop that returns it.

3. **Redirect re-validation** (on every redirect hop): Re-runs the full URL policy on the redirect target. Refuses HTTPS-to-HTTP downgrades. Enforces the hop cap. The resolver re-classifies the redirect target's addresses at connect time.

4. **No ambient identity**: The client carries no cookies, no `Authorization` header, no `Referer`, and disables ambient proxy (`HTTP_PROXY`/`HTTPS_PROXY`). A redirect cannot smuggle credentials to a cross-origin target.

The built-in blocked-address table covers all IPv4 and IPv6 special-use space: loopback, RFC1918, CGNAT, link-local (including `169.254.169.254`), documentation, benchmarking, multicast, reserved, and IPv6 equivalents including IPv4-mapped, NAT64, unique-local, and deprecated site-local. IPv4-embedded IPv6 addresses (`::ffff:127.0.0.1`, `::10.0.0.1`) are normalized to their embedded IPv4 value and reclassified.

### Error Behavior

Errors split into two categories based on whether a retry makes sense:

**Soft outcomes** (returned as tool text the model can act on):
- HTTP error status (404, 500, etc.)
- Timeouts
- DNS failures
- Unsupported or absent content type
- Body too large
- Body read failure mid-stream
- Redirect refused
- Blocked scheme (`http` when only `https` is allowed)

**Hard errors** (the URL itself is invalid, no retry will help):
- Unparseable URL
- URL contains userinfo
- Port not on the allowlist
- IP literal not allowed
- Address is blocked / no allowed address for the host

When a blocked address is reported to the model, only the host name appears in the message - never the resolved address or the blocking range. Query strings and fragments are redacted from all diagnostic URLs so a `?token=secret` never reaches logs or model output.

---

## promptforge-dev

promptforge-dev is the edit-run-inspect loop for PromptForge prompts. Point it at a prompt file, and it runs the prompt against your already-running gateway, dumps the store for inspection, and optionally watches for saves so every edit triggers a fresh run. No gateway management, no model downloads, no weight files - just the prompt and its output, tight enough that your iteration cycle is limited by how fast you can think, not how long you wait.

### Prerequisites

promptforge-dev requires a running `promptforge-gateway`. Start it yourself, then export two environment variables:

````sh
export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
export PROMPTFORGE_GATEWAY_API_KEY=<bearer from your gateway profile>
````

Both must be set and non-empty. If either is missing, the binary fails immediately with a message naming the missing variable and reminding you to start the gateway. No prompt file is read until both are validated.

### Your First Run

From the PromptForge repository root:

````sh
cargo run -p promptforge-dev -- my-prompt.md
````

This runs `my-prompt.md` with an empty input. The second positional argument supplies an input string:

````sh
cargo run -p promptforge-dev -- my-prompt.md "summarize this paragraph"
````

The input becomes the prompt's `args`. If you omit it, it defaults to empty.

Model runtime parameters - context window, thinking mode, max tokens - are not CLI flags. Declare them on the prompt file under `models.bind` or `models.default`. The binary's argument surface is deliberately minimal: `promptforge-dev [--watch] [--capture-raw] <prompt.md> [input]`.

### What Happens During a Run

Each invocation follows a fixed pipeline:

1. **Validate environment.** Confirm `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY` are set.
2. **Fetch the model catalog.** One HTTP call to the gateway. The catalog is fetched once and reused across watch-mode reruns.
3. **Build the tool set.** Two tools are always constructed: `web_fetch` (runs locally) and `web_search` (proxies through the gateway). A semantic tool picker is derived from the same live set, so no picker descriptor can advertise a tool without a matching callable.
4. **Parse the prompt.** The file must declare `promptforge:` in its YAML frontmatter. A file without it is refused: "is not a promptforge prompt."
5. **Execute.** The prompt runs against the gateway. The store stays in memory during execution - no filesystem writes happen on the async path.
6. **Dump the store.** After the run (success or failure), the in-memory store is reconciled to disk beside the prompt file.

A unique execution id is minted for each run: `dev-` followed by 128 random hex bits. It prints to stderr before any observer output, so you can always tell which run produced which output:

````text
run id: dev-3a7f1b2c9e4d5a8f0011223344556677
````

Observer records stream to stderr as single trace lines:

````text
[dev-3a7f1b2c9e4d5a8f0011223344556677] Research: Run started
[dev-3a7f1b2c9e4d5a8f0011223344556677] Research: Lua: checkpoint
````

The final result prints to stdout. This separation lets you pipe or redirect output without observer noise.

### Inspecting Output

Every run dumps its store to `<prompt-stem>.store/` beside the prompt file. For a prompt named `briefer.md`, the dump lands in `briefer.store/`:

````text
briefer.md
briefer.store/
  evidence.md
  notes/
    deep.txt
````

The dump reconciles on every run:

- Changed files are overwritten with current contents.
- Files from a previous run that are no longer in the store are deleted.
- The `.trace/` subdirectory (used by raw trace capture) is preserved across reconciles.
- When the store is empty and no trace files remain, the dump directory is removed entirely.

A failed run still dumps its partial store. That partial output is exactly what you need when debugging a prompt that errored partway through.

### Watch Mode

Add `--watch` to enter a rerun loop:

````sh
cargo run -p promptforge-dev -- --watch my-prompt.md "test input"
````

The prompt runs once, then the file is watched for changes:

````text
watching my-prompt.md for changes; press Ctrl-C to stop
````

Every save triggers a rerun after a 300 ms debounce quiet period. The debounce absorbs editor write-then-rename save bursts so a single save produces a single rerun, not two or three.

The gateway catalog, tools, and picker built at startup are reused across every rerun - no repeated network calls. Each rerun gets a fresh execution id.

If a rerun fails, the error prints to stderr and watching continues. A broken edit does not kill your session.

The watcher monitors the prompt's parent directory, filtered to the prompt's file name. Store dump writes (to the `.store/` directory) do not retrigger reruns. The watcher uses a capacity-one bounded channel, so a slow rerun or a noisy filesystem cannot grow an unbounded event backlog. Watcher backend errors surface through a separate loss-proof slot - they are never silently dropped, even when the channel is full.

### Raw Trace Capture

Add `--capture-raw` to persist the verbatim request and response bodies for each model turn:

````sh
cargo run -p promptforge-dev -- --capture-raw my-prompt.md
````

A warning prints to stderr:

````text
warning: --capture-raw persists verbatim prompts, tool arguments and results, and model output to my-prompt.store/.trace
````

Each model turn writes two files under `.trace/`:

````text
my-prompt.store/
  .trace/
    turn-1-request.json
    turn-1-response.json
    turn-2-request.json
    turn-2-response.json
````

These contain the full, unredacted request and response JSON. The material is sensitive - raw prompts, tool arguments and results, model output - which is why capture is off by default and requires an explicit flag.

Trace capture uses a bounded queue (128 events) with a dedicated worker thread. The worker serializes and writes each payload with owner-only permissions and atomic semantics. If the worker falls behind, events are counted as dropped and the count is reported when the run finishes. I/O never blocks the run's async task.

All queued writes are flushed before the store dump reconcile, so trace files are always complete when you inspect the dump directory.

### Filesystem Security

All dump writes - store files and trace captures - go through a security layer:

- **Owner-only permissions.** Directories are created `0o700` and files `0o600` on Unix. On Windows, inherited access is stripped and full control is granted to the current user alone via `icacls`.
- **No symlink traversal.** Every write checks the target and all existing ancestors for symlinks and Windows reparse points. A planted link at any path component is refused, preventing writes from escaping the dump tree.
- **Atomic writes.** Each file is written to a sibling temporary (`.{name}.tmp{random}`), flushed, permission-restricted, then renamed over the destination. An interrupted write cannot truncate a prior file. The temporary is cleaned up on failure.
- **Path safety.** Store paths that are absolute, traverse with `..`, contain backslashes, control characters, or Windows reserved characters (`*`, `?`, `"`, `<`, `>`, `|`) are skipped with a status report. Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9 - including Unicode superscript digit variants) are also rejected.

You do not configure any of this. It is always active.

### Diagnostics

When a Lua error maps to a prompt line, the failure message leads with the file and line number:

````text
dev run failed: briefer.md:51: run briefer.md: lua error: section `Web Search` epilog:51: assertion failed!
````

This format enables click-to-navigate in editors that recognize `file:line:` patterns.

Errors without a mapped prompt line omit the line prefix:

````text
dev run failed: some transport error
````

**Exit codes:**

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Runtime error (gateway, parse, execution, dump) |
| 2 | Usage error (bad arguments) |
| 130 | Interrupted by Ctrl-C |

Ctrl-C is handled cooperatively: the run is cancelled, its completion is awaited (so blocking fanout joins are not abandoned), and the process exits with code 130.

### Edge Cases and Validation

**Unknown flags.** Any flag starting with `--` that is not `--watch`, `--capture-raw`, or `--` is rejected with usage text. This includes former server knobs like `--context`, `--max-tokens`, and `--no-think` that were removed when model parameters moved to the prompt file.

**Non-PromptForge files.** A markdown file whose YAML frontmatter does not declare `promptforge:` is refused with a clear message rather than producing a confusing parse error.

**The `--` delimiter.** Use `--` to pass an input that begins with dashes:

````sh
cargo run -p promptforge-dev -- my-prompt.md -- --this-is-input-not-a-flag
````

Everything after `--` is treated as a positional argument.

**Credential protection.** The bearer key is wrapped in a `GatewayKey` type that renders as `<redacted>` in Debug output. An accidental `{:?}` on a `GatewayEnv` cannot leak the credential.

---

*This guide was assembled from per-crate documentation. For source-level details, see each crate's individual user guide.*
