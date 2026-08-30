# Gateway

promptforge-gateway is the one process in PromptForge that talks to LLM backends. Point it at a TOML file, and it serves an OpenAI-compatible HTTP API that routes chat completions to configured backends, holds every credential, manages a model catalog, runs a built-in web search tool, and optionally spawns local `llama-server` processes for GGUF models. Nothing above it holds a vendor key. Nothing above it knows which machine answers. A key rotation touches one file on one host.

After reading this chapter you will be able to configure, start, and operate the gateway for remote endpoints, local models, multiple profiles, and built-in tools.

## What the Gateway Does

The gateway accepts `POST /v1/chat/completions` requests in the OpenAI chat completions format. It resolves the model name the caller asked for, substitutes the backend's own model string into the outgoing request, forwards it, and restores the caller's model name on the response. Everything else in the request body - sampling parameters, tool definitions, template arguments - passes through untouched in a flattened map, so a parameter the gateway has never heard of reaches the backend without a gateway release. Models configured with `kind = "embedding"` are served instead at `POST /v1/embeddings` in the OpenAI embeddings format, with the same routing and passthrough discipline. Models configured with `kind = "classifier"` serve rerank requests at `POST /v1/rerank` - a query and a document set in, ranked relevance scores out - under the same discipline.

Credentials live here and nowhere else. Each `[[endpoint]]` carries an `api_key`, each `[tools.web_search]` carries a search provider key, and each `[[local_model]]` is reached over a loopback connection with a generated bearer. The `Secret` type ensures no credential can be serialized, logged, or printed: it redacts in both `Debug` and `Display`, and `expose()` is the single plaintext accessor.

Model resolution is one exact string lookup. A miss is a 404. There is no prefix matching, no regex, no alias chain, and no default model. A typo is a clear error rather than a silent charge against the wrong backend.

## Configuration

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

### Environment Variable Interpolation

Any string value can use `${VAR}` to reference an environment variable. Interpolation runs after the TOML is parsed, so it applies only to string values. An unresolved variable fails the load, so a deployment that forgot to export a credential never starts serving with a blank one. Use `$$` for a literal dollar sign.

There is no implicit pickup of `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` from the ambient environment. Every credential must appear in the configuration as an explicit `${VAR}` reference.

At boot the process environment is populated from at most two env files: the profile's own file first (`profiles/main.env` for `--profile main`), then the boot file's sibling env file (`gateway.env` beside `gateway.toml`). Neither file overrides a variable that is already set, so precedence is the process environment, then the profile's file, then the boot file's. Included files' env files are never loaded. On a profile switch only the new profile's env file is loaded; the boot file's is already in the process.

### Model Fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `name` | yes | - | Caller-facing model name |
| `kind` | no | `chat` | Model kind: `chat`, `embedding`, or `classifier` |
| `description` | yes | - | Prose for catalog consumers |
| `context` | yes | - | Context window size in tokens |
| `upstream` | yes | - | The string the backend knows this model by |
| `endpoints` | yes | - | Endpoint ids (first is used) |
| `thinking` | no | `never` | `never`, `always`, or `switchable`; chat kind only |
| `default_max_tokens` | no | - | Parsed, not yet consumed; chat kind only |
| `max_output` | no | - | Max output tokens per completion; must not exceed `context` |
| `default_temperature` | no | - | Sampling temperature applied when the caller omits one |
| `images` | no | `false` | Whether the model accepts image inputs |
| `parallel_tool_calls` | no | `false` | Whether the model can emit parallel tool calls |
| `effort_levels` | no | - | Reasoning-effort levels the model accepts; chat kind only, requires `thinking` other than `never` |
| `default_effort` | no | - | Effort level applied when the caller omits one; must name a listed `effort_levels` entry; chat kind only |
| `adaptive_thinking` | no | `false` | Whether the model adaptively chooses how much to think per request; chat kind only |
| `tool_dialect` | no | `openai` | Tool-calling dialect: `openai` (native) or `gemma3_tool_code` (emulated fences); chat kind only |

`embedding` and `classifier` models reject chat-only fields at load: `thinking`, `default_max_tokens`, `tool_dialect`, `effort_levels`, `default_effort`, and `adaptive_thinking` are chat-only, while `context` applies to every kind. The catalog entry carries the kind so clients can filter before building a request. The effort knobs (`effort_levels`, `default_effort`) are also rejected when `thinking = "never"`, and `max_output` must not exceed `context`.

### Endpoint Fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `id` | yes | - | Operator handle referenced by models |
| `protocol` | yes | - | Wire protocol; only `openai` |
| `base_url` | yes | - | Backend URL (trailing slash trimmed) |
| `api_key` | yes | - | Backend credential; empty string skips the `Authorization` header |
| `dominion` | no | - | Remote dominion id for a shared limit and queue |

## Starting the Gateway

```bash
promptforge-gateway serve gateway.toml --profile main
```

Boot requires two things: a config path and a profile name. The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable; the CLI argument wins, and with neither set the boot fails with a usage error naming both sources. The profile name comes from `--profile` only (no env var). It is required - there is no anonymous boot. Every gateway has at least one profile; the initial loaded set always has a name.

The profiles directory is always the `profiles/` directory beside the config file - never independently configurable, and there is no `~/.promptforge/profiles` default. Booting with an unknown profile name fails with a startup error listing the available profiles; a missing `profiles/` directory or a missing profile file is likewise a startup error.

The boot file is the catalog and infrastructure; it is not loaded as the runtime config directly. The named profile is loaded with include resolution and becomes the initial config. A profile may declare a top-level `models = ["name", ...]` allowlist selecting a subset of the catalog's `[[model]]` and `[[local_model]]` entries; the loaded set is exactly the selection, so `GET /v1/models` shows nothing else. An allowlist entry naming a model the catalog does not define is a validation error at load. With no `models` key, the profile loads the full catalog. The single-file setup needs one minimal profile, `profiles/main.toml` beside `gateway.toml`:

```toml
include = ["../gateway.toml"]
```

Startup order: load the two env files, resolve the profile's include chain, start local model runtime (when `[[local_model]]` is present), build the routing table, bind, serve. A broken config never reaches a listening socket.

## Model Catalog and Routing

Clients discover available models by calling `GET /v1/models` with a bearer token:

```json
{
  "object": "list",
  "data": [
    {
      "id": "reasoning-large",
      "object": "model",
      "kind": "chat",
      "description": "Anthropic's best reasoning model",
      "context": 200000,
      "thinking": "switchable",
      "max_output": 64000,
      "images": true,
      "parallel_tool_calls": true,
      "effort_levels": ["low", "high"],
      "default_effort": "low",
      "adaptive_thinking": false
    }
  ]
}
```

Each entry includes the model's kind, context window, thinking mode, and capability metadata so clients can make binding decisions before sending a request. The flags (`images`, `parallel_tool_calls`, `adaptive_thinking`) and `effort_levels` are always present; the optional knobs (`max_output`, `default_temperature`, `default_effort`) appear only when configured.

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

## Emulated Tool Calling

A chat model whose backend has no native tool array can still serve tool calls by setting `tool_dialect = "gemma3_tool_code"` on its `[[model]]` entry:

```toml
[[model]]
name = "gemma-remote"
description = "A Gemma 3 instruct model behind an OpenAI-shaped server"
context = 8192
tool_dialect = "gemma3_tool_code"
upstream = "gemma3-it"
endpoints = ["local-llama"]
```

For a non-streaming chat completion to such a model the gateway emulates the protocol. Outbound, the OpenAI `tools` array is translated into a plain-language system guide teaching the `tool_code` fence protocol (one `name(key=value)` call per line) and prepended as a system message; `tools` and `tool_choice` are stripped from the forwarded request. Inbound, the reply's content is scanned for a leading `tool_code` fence (or an interim `json` fence carrying a `tool_calls` blob); a well-formed fence becomes OpenAI `tool_calls` objects with a null `content` and a `tool_calls` finish reason.

Recovery is warn-and-continue, never silent and never fatal: a recognized-but-malformed fence (unterminated, empty, an invalid call line, or trailing prose after the fence) yields an empty `content` plus a `gateway_warning` field on the message carrying the reason, and the recovery is logged at warn. `gateway_warning` is a gateway-specific extension on the OpenAI response shape; downstream clients should ignore unknown fields. The dialect uses content-fence parsing only - the gateway never sets `response_format: json_object`.

Emulation applies to non-streaming completions only; a `stream: true` request is forwarded unchanged.

## Streaming

Pass `"stream": true` on a chat completion to get a server-sent events stream instead of a single JSON response:

```json
{
  "model": "reasoning-large",
  "messages": [{"role": "user", "content": "Explain monads"}],
  "stream": true
}
```

The response is `Content-Type: text/event-stream` with one `data:` line per chunk, terminated by `data: [DONE]`. The relay is typed: the gateway parses and validates each upstream chunk, rewrites the model name back to the caller's, and re-serializes it - it never splices raw upstream bytes through. A failure before the stream starts (an upstream 4xx/5xx, an unreachable backend) comes back as the usual JSON error envelope, never as an SSE stream that dies mid-flight; a failure mid-stream is emitted as an error-envelope `data:` event before the stream ends. The dominion queue slot is held for the stream's whole lifetime, so a long stream counts against concurrency until it ends. Streaming requests use a connect-timeout-only HTTP client: the usual whole-request deadline is not applied, since it would kill any stream that outlived it.

Two caveats. A client that disconnects mid-stream cancels the upstream stream: dropping the response drops the upstream connection and frees the queue slot in the same unwind, so a hung client never holds a backend stream open. And a malformed upstream chunk - one that is not JSON, or that lacks the minimal choice shape (at least one choice with an `index` and a `delta`) - is logged and skipped rather than relayed or fatal, so one bad chunk never ends an otherwise healthy stream. The terminal `data: [DONE]` is a sentinel, not JSON; it is recognized before parsing and never logged as malformed.

## Embeddings

Models configured with `kind = "embedding"` serve OpenAI-shaped embedding requests at `POST /v1/embeddings`:

```json
{
  "model": "embed-large",
  "input": ["first document", "second document"],
  "encoding_format": "float"
}
```

The `input` is a single string or an array of strings; `encoding_format` (`float` or `base64`) is optional, and every other OpenAI embeddings field passes through verbatim. The route applies the same bearer auth, model resolution and rewrite, and dominion queue admission as chat completions, and the response restores the caller's model name with `data` and `usage` passed through from the backend. Naming a chat or classifier model here is a 400 `kind_mismatch`. A model whose upstream cannot serve embeddings at all - a local chat server, for example - fails with 400 and `code: "model_unavailable"`.

## Rerank

Models configured with `kind = "classifier"` serve rerank requests at `POST /v1/rerank`:

```json
{
  "model": "rerank-large",
  "query": "what is rust",
  "documents": ["a card game", "a systems language"],
  "top_n": 2
}
```

The request names a query and the documents to rank against it; `top_n` is optional, and every other field passes through verbatim. The route applies the same bearer auth, model resolution and rewrite, and dominion queue admission as chat completions, and the response restores the caller's model name with `results` and `usage` passed through from the backend. Naming a chat or embedding model here is a 400 `kind_mismatch`. A model whose upstream cannot serve rerank at all - a local chat server, for example - fails with 400 and `code: "model_unavailable"`.

## Authentication and Errors

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
| Model kind does not match the route | 400 | `invalid_request_error` | `kind_mismatch` |
| Model's upstream cannot serve the route's workload | 400 | `invalid_request_error` | `model_unavailable` |
| Tool not configured | 404 | `invalid_request_error` | `not_found` |
| Bad request body | 400 | `invalid_request_error` | `malformed_request` |
| Backend connection failed before the request was sent | 502 | `server_error` | `upstream_connect` |
| Backend unreachable mid-flight (read/timeout) | 502 | `server_error` | `upstream_transport` |
| Backend decode failure | 502 | `server_error` | `upstream_protocol` |
| Backend 4xx | upstream's | `invalid_request_error` | `upstream_client_error` |
| Backend 5xx | 502 | `server_error` | `upstream_error` |
| Queue full | 503 | `server_error` | `queue_full` |
| Rejected at capacity (`policy = "reject"`) | 429 | `rate_limit_error` | `queue_rejected` |

An unmodified OpenAI SDK surfaces these as its own error types rather than unparseable blobs.

## Concurrency and Queuing

Bind an endpoint to a dominion to cap how many requests are in flight at once. The limit lives on the dominion, so everything bound to it shares one pool of slots:

```toml
[[dominion]]
id = "anthropic-pool"
kind = "remote"
max_concurrency = 10
max_queue = 100         # waiting requests (not counting in-flight); default 100
policy = "queue"        # "queue" | "reject" (fail-fast); default "queue"
fair_scheduling = true  # round-robin by client key; default true

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"
dominion = "anthropic-pool"
```

Requests beyond the limit wait in the dominion's bounded queue. What a full queue does comes from `policy`: with `queue` (the default), up to `max_queue` requests wait and further arrivals are rejected with 503 and `code: "queue_full"`; with `reject`, a request that finds no free concurrency slot is turned away immediately - fail-fast, mapped to 429 with `code: "queue_rejected"`.

When `fair_scheduling` is true, callers identify themselves via the `X-PromptForge-Client` header. Each client gets turns in round-robin order, so one fast client cannot monopolize slots. Missing or invalid headers map to the `"default"` bucket. The scheduler tracks up to 32 distinct client labels; additional labels fold into `"default"`. The header is self-asserted: a scheduling hint for trusted-host callers, not an authenticated identity.

An endpoint without a `dominion` is unlimited.

## Dominions

A dominion is a named pool of compute - a remote provider pool or a local GPU - carrying one concurrency limit and one bounded waiting queue shared by everything bound to it: two endpoints bound to the same dominion compete for the same slots. A dominion binding is the only way to cap concurrency.

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
parallel = 4              # child --parallel; the queue limit when no dominion is bound
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

An endpoint bound to a remote dominion shares that dominion's queue with every other bound endpoint, and a local model bound to a local dominion shares that dominion's queue with every other bound local model.

When a local dominion sets `vram_gb`, every local model bound to it must set its own `vram_gb` footprint estimate, and the estimates must sum to no more than the budget: an over-booked or incomplete budget fails validation at boot and at profile switch, before any child process starts and surfaces as an OOM. A local dominion without `vram_gb` imposes no co-residency obligation.

## Web Search Tool

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

### Request Fields

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

### Configuration Defaults

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

## Named Profiles

Organize configurations for different environments as TOML files in the `profiles/` directory beside the boot file:

```text
<config-parent>/
  gateway.toml
  profiles/
    main.toml
    analytical.toml
    dev.toml
```

Start with a named profile:

```bash
promptforge-gateway serve gateway.toml --profile analytical
```

Every gateway boots into a profile, so the initial loaded set always has a name. A profile typically contains `include = ["../gateway.toml"]` plus its own overrides, keeping the boot file as the shared catalog.

### Selecting a Subset of the Catalog

A profile selects its loaded set with a top-level `models` allowlist:

```toml
# analytical.toml
include = ["../gateway.toml"]
models = ["reasoning-large", "qwen3.8-local"]
```

After the include chain merges, the catalog's `[[model]]` and `[[local_model]]` arrays filter to the listed names: the loaded set is the selection, and `GET /v1/models` shows exactly it. The filter runs before validation, so reference checks and the VRAM co-residency check apply to the loaded set only - a catalog whose local models over-book a GPU in total still boots a profile whose selection fits the dominion's `vram_gb` budget, and endpoints or dominions referenced only by filtered-out models may stay defined. An allowlist entry naming a model the catalog does not define fails the load. With no `models` key the profile loads the full catalog, and when several files in one include chain declare `models`, the later file's list replaces the earlier one. `GET /admin/status` reports the active selection as `model_allowlist` (`null` when the full catalog is loaded).

### Profile Inheritance

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

- Arrays (`[[endpoint]]`, `[[model]]`, `[[local_model]]`, `[[dominion]]`): merged by append. An entry with the same `id` or `name` replaces the earlier definition.
- Scalars (`server.*`, `[local].cache_dir`): later wins.
- The `models` allowlist: later wins - one list replaces the other, never unioned.

### The Boot File Owns `[server]`

After include resolution, the profile's merged `[server]` section must equal the boot file's `[server]` exactly - bind address and api_key, compared as values after `${VAR}` interpolation. A mismatch fails the boot (or the profile switch): a bind mismatch names both addresses, while an api_key mismatch names only the profile and the field, with both keys redacted. The conventional setup passes by construction because profiles include the boot file. The consequence: the socket and the gateway bearer key are fixed for the process lifetime, and a profile switch never rotates the admin credential.

Includes remain free-form: a profile may include a different file than the boot path, or be self-contained - a self-contained profile must replicate the boot file's `[server]` verbatim to boot. At startup the gateway logs the resolved include chain, plus a warning when the boot file is not in it: the likely-mistake case, where edits to the boot file have no effect.

### Admin Routes

All admin routes use the same bearer token as `/v1`:

| Route | Method | Purpose |
|---|---|---|
| `/admin/profiles` | GET | List `*.toml` stems in the profiles directory |
| `/admin/status` | GET | Current profile name, loaded model names, local child count, and the profile's `models` allowlist |
| `/admin/switch-profile` | POST | Switch to a named profile immediately |

Switch with:

```json
{"name": "analytical"}
```

Send this as `POST /admin/switch-profile` with `Authorization: Bearer <token>`.

Profile switches are serialized by a mutex. The old local children are stopped (freeing VRAM) before new ones start. The new configuration is built and validated before touching live state. On success, the routing, web-search settings, and local runtime are atomically swapped. On failure, the previous state stays intact with a stable admin credential.

The `[server]` section does not change on switch: the boot file owns it, and a profile whose merged `[server]` differs from the boot file's is rejected. Moving the socket or rotating the gateway key requires a restart.

Profile names must be a single path component - no separators, no `.` or `..`, no empty string. This confinement prevents directory traversal through the admin API.

## Local Inference

Run local generative models by declaring `[[local_model]]` entries. The gateway provisions a pinned `llama-server` binary, downloads each GGUF, and spawns one child process per model. On a Windows x86-64 build with the `llama-cuda` feature the binary is a host-native CUDA build staged from a bundle embedded in the gateway binary; every other build downloads the pinned GPU archive (Vulkan on Windows/Linux, Metal on macOS).

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

Each local model becomes a normal catalog entry. Clients reach it through the same `POST /v1/chat/completions` as remote models (`POST /v1/embeddings` for `kind = "embedding"`, `POST /v1/rerank` for `kind = "classifier"`) - the fact that it runs locally is invisible to callers.

### Configuration Fields

| Field | Required | Default | Purpose |
|---|---|---|---|
| `name` | yes | - | Caller-facing model name |
| `kind` | no | `chat` | Model kind: `chat`, `embedding`, or `classifier` |
| `description` | yes | - | Prose for catalog and semantic bind |
| `source` | yes | - | Hugging Face URL or local path to GGUF |
| `sha256` | no | - | SHA-256 hex pin; verified after download |
| `context` | yes | - | Context window (`--ctx-size`) |
| `thinking` | no | `never` | `never`, `always`, or `switchable`; chat kind only |
| `gpu_layers` | no | 99 | GPU layers offloaded (`-ngl`) |
| `flash_attention` | no | `true` | Enable flash attention |
| `cache_type_k` | no | `q8_0` | KV cache type for K |
| `cache_type_v` | no | `q4_0` | KV cache type for V |
| `n_predict` | no | 8192 | Generation ceiling (`--n-predict`) |
| `chat_template_file` | no | - | Jinja template override (`--chat-template-file`); chat kind only |
| `dominion` | no | - | Local dominion id for a shared limit |
| `parallel` | no | 1 | Max concurrent inferences (`--parallel`; the queue limit when no dominion is bound) |
| `vram_gb` | no | - | VRAM footprint estimate in GiB |
| `max_output` | no | - | Max output tokens per completion; must not exceed `context` |
| `default_temperature` | no | - | Sampling temperature applied when the caller omits one |
| `images` | no | `false` | Whether the model accepts image inputs |
| `parallel_tool_calls` | no | `false` | Whether the model can emit parallel tool calls |
| `effort_levels` | no | - | Reasoning-effort levels the model accepts; chat kind only, requires `thinking` other than `never` |
| `default_effort` | no | - | Effort level applied when the caller omits one; must name a listed `effort_levels` entry; chat kind only |
| `adaptive_thinking` | no | `false` | Whether the model adaptively chooses how much to think per request; chat kind only |

A local model with `kind = "embedding"` or `kind = "classifier"` rejects the chat-only fields `thinking`, `chat_template_file`, `effort_levels`, `default_effort`, and `adaptive_thinking` at load; `context` and the launch knobs (`gpu_layers`, `flash_attention`, cache types, `parallel`, `vram_gb`) apply to every kind. The effort knobs are also rejected when `thinking = "never"`, and `max_output` must not exceed `context`.

### Companion Artifacts

A chat local model can declare two companions - a speculative-decoding drafter and a multimodal projector - each downloaded and digest-verified through the same cache machinery as the main model:

```toml
[[local_model]]
name = "gemma-4"
description = "Gemma 4 E2B instruct with MTP drafting and vision"
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/gemma-4-E2B-it-UD-Q4_K_XL.gguf"
sha256 = "b52f438017efaec5debf1c0d8be690571e212a07c312f1102bbce927258cfc32"
context = 131072

[local_model.speculative]
type = "draft-mtp"          # multi-token-prediction drafter
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mtp-gemma-4-E2B-it.gguf"
sha256 = "9eba819938efccfd6044f8af84e3bbfddc639a2bcf32ebc36420e6a649191919"
draft_max = 2               # tokens drafted per step; 1..=16

[local_model.multimodal_projector]
source = "https://huggingface.co/unsloth/gemma-4-E2B-it-GGUF/resolve/main/mmproj-F16.gguf"
sha256 = "140be8d7849741f88c50757d529b84373ee8e27052cc2236855b537f4a8215fa"
```

`[local_model.speculative]` makes the child launch with `--spec-draft-model`, `--spec-type draft-mtp`, and `--spec-draft-n-max`, so generation runs with multi-token-prediction speculative decoding. `[local_model.multimodal_projector]` passes `--mmproj`, so the model accepts image content in chat completions. Companion sources follow the main source's rules: an `https` URL requires a `sha256` pin, a local path may go unpinned, and plaintext `http` is rejected. Both companions are chat-only. Each companion lands in its own cache slot keyed by its own source, the resolved paths live in the child's launch state so a respawn re-emits the exact verified artifacts, and a companion resolution failure stops startup before any child process is spawned.

### CUDA Builds

A gateway built with `--features llama-cuda` on Windows x86-64 compiles the pinned `third_party/llama.cpp` submodule into a Release CUDA `llama-server` during the Cargo build. The build machine needs the submodule checked out (`git submodule update --init`), CUDA Toolkit >= 12.8, and the NVIDIA GPUs the server should run on - the build detects every visible GPU's compute capability and compiles only those architectures, and cross-compilation is rejected. The `workshop-cuda` feature implies `llama-cuda` and adds CUDA acceleration for the whisper voice engine; the desktop app's default `cuda` feature forwards to `workshop-cuda`.

The split is strict: Cargo build compiles, runtime only verifies and stages. The build embeds a versioned manifest (source commit, tool identities, architectures, per-file SHA-256) plus the runtime files into the gateway binary. At startup the gateway validates the embedded payload against the manifest, checks the host provides the declared CUDA Toolkit runtime DLLs, and atomically stages the files into the operator cache; a valid matching installation is reused as-is, and a CUDA build never silently falls back to the Vulkan archive. Runtime and serve paths never invoke a compiler or build tool.

Diagnostics: a build failure is a Cargo build error from the gateway's build script; a staging failure is a startup provisioning error naming the validation that failed (tampered payload, target mismatch, missing toolkit DLL). Embedding hosts can read a bounded, credential-redacted tail of each child's captured output through `Gateway::local_diagnostics` to confirm the child reported a CUDA device and offloaded its layers. On a suitable host the ignored live integration test proves the full path end to end:

```bash
cargo test -p promptforge-gateway --features llama-cuda -- --ignored live_cuda   # needs PROMPTFORGE_LIVE_CUDA=1
```

### Local Embeddings

A local model with `kind = "embedding"` launches its child as `llama-server --embeddings` and serves `POST /v1/embeddings` exactly like a remote embedding model. Artifact download, digest pinning, dominion binding, and child supervision (respawn of a dead child) are unchanged from a chat child.

```toml
[[local_model]]
name = "bge-local"
kind = "embedding"
description = "A compact English embedding model for retrieval and similarity"
source = "https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5-q8_0.gguf"
sha256 = "ec38e8da142596baa913124ae50550de284b6916bf59577ef2f0cb9660c2f514"
context = 512
```

### Local Classifiers

A local model with `kind = "classifier"` launches its child as `llama-server --reranking` and serves `POST /v1/rerank` exactly like a remote classifier model. Artifact download, digest pinning, dominion binding, and child supervision (respawn of a dead child) are unchanged from a chat child.

```toml
[[local_model]]
name = "jina-local"
kind = "classifier"
description = "A tiny English reranker for scoring query-document relevance"
source = "https://huggingface.co/gpustack/jina-reranker-v1-tiny-en-GGUF/resolve/main/jina-reranker-v1-tiny-en-Q8_0.gguf"
sha256 = "0defc1f8a1f4dd22183124a2a25a97765603e5a9e42258046c9b2c8a26d1f553"
context = 512
```

### Cache and Provisioning

The cache directory defaults to `~/.promptforge` (set `[local].cache_dir` to override). Models land in `<cache>/models/`, the llama.cpp binary in `<cache>/llama.cpp/`.

First-time downloads and other long-running operations show indicatif progress bars on interactive TTY stderr. On non-TTY stderr, periodic tracing progress lines are emitted instead.

When `sha256` is set, the downloaded file is verified against the digest.

### Tool-Calling Dialect Detection

After a local child reports ready, the gateway queries its `/props` endpoint and resolves a tool-calling dialect from that evidence, hard-failing on ambiguous or absent evidence so a local model never silently defaults to an incorrect dialect. A sidecar `.md` file beside the GGUF (with frontmatter and a Jinja chat template) provides fallback evidence when `/props` omits `chat_template`; live props always win. The probe runs for `kind = "chat"` children only: a non-chat child has no chat template to evidence a dialect, so it carries the default `openai` dialect, same as a remote model. The resolved dialect is gateway-internal and selects how tool calls are emulated; it is not advertised in the catalog.

### Child Supervision

If a transport failure occurs against a dead `llama-server` child, the gateway respawns it once on the same port and alias, then retries the request. There is no background watchdog. `GET /health` remains process-level liveness only.

### Local Concurrency

`parallel` on a `[[local_model]]` (default 1) is both the child's `--parallel` argument and, when the model has no `dominion`, its gateway queue limit:

```toml
[[local_model]]
name = "qwen-local"
description = "..."
source = "..."
context = 65536
parallel = 4
```

Bind the model to a local dominion to share one concurrency limit and one VRAM budget with every other bound model; admission is then governed by the dominion's shared queue instead of a per-model limit.

Dropping the `LocalRuntime` (on process exit or profile switch) kills all `llama-server` children.
