# PromptForge Gateway User Guide

PromptForge Gateway is a local model-serving gateway. It serves your prompts and models through an OpenAI-compatible HTTP API, so your existing OpenAI clients and tools work against it without modification. You describe every model once in a single `gateway.toml` file. You group models into named profiles. You then switch the served model set mid-run without downtime, and you keep every vendor key in one file on one host. This guide teaches you to start the gateway, configure models and profiles, run local models on your own GPU, and operate the admin surface with confidence.

## What the Gateway Is

The gateway is one binary, `gateway`. It routes OpenAI-shaped chat completion requests to a backend. You run it in the foreground and it serves until you stop it.

Every client addresses a model by the public `name` you gave it in `gateway.toml`. The gateway resolves that name to a configured endpoint. It rewrites the name to the endpoint's upstream model alias before it calls the provider. Your clients never learn the upstream alias.

The gateway is the only process that holds a vendor key. Clients authenticate to the gateway with a single shared bearer key. The gateway authenticates to the provider with the vendor key. The caller's bearer key never leaks upstream.

The gateway can also run local models with no external server. It downloads model files, verifies their checksums, and manages the `llama-server` child processes for you. Local and remote models merge into one catalog. Clients address both by name in the same way.

Named profiles select which models the gateway serves. You switch the active profile mid-run. In-flight requests drain before the switch completes.

## Running the Gateway

Start the gateway with one command:

````bash
gateway serve gateway.toml --profile main
````

`serve` is the only subcommand. Boot requires two things: a config path and a profile name.

The config path comes from the positional argument or the `PROMPTFORGE_GATEWAY_CONFIG` environment variable. The argument wins. With neither, boot aborts with a usage error.

The profile comes from three sources, in fixed precedence: the `--profile` flag beats `PROMPTFORGE_PROFILE`, which beats the persisted state file. The gateway records the active profile in a `<config-stem>.state.toml` sibling as `active_profile = "name"`, so the selection survives restarts.

A minimal config file looks like this:

````toml
config-version = 2

[server]
bind = "127.0.0.1:8080"
api_key = "my-secret-key"
````

The file must be a single `gateway.toml` with `config-version = 2` at the top. A missing or wrong version fails to load. The old include/profiles-dir layout is a hard break: load fails with an error that names the file, the key, the line, and the replacement to use.

The `[server]` section sets the listener address and the shared bearer key. Both fields accept `${VAR}` interpolation from the process environment. A `.env` file next to the config loads automatically at startup. It never overrides variables already set in the process environment.

Once serving, a log line announces the bound address. Press Ctrl-C to shut down gracefully. The process exits 0 on success and non-zero otherwise. Failures print a chain of `caused by:` lines so you see the root cause. Startup failures are classified into six named categories: Config, Provisioning, Bind, Thread, Serve, and Workshop. You can tell a bad config file from a port conflict without reading internals.

Run `gateway -h` to print usage and exit. Unknown flags, unknown subcommands, and a missing `serve` subcommand are usage errors printed to stderr with the full usage text.

## Inference Endpoints

Call the chat endpoint with any OpenAI client. Send your bearer key on every request:

````bash
curl http://127.0.0.1:8080/v1/chat/completions \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"model": "my-model", "messages": [{"role": "user", "content": "Hello"}]}'
````

The `model` field carries your public model name. The gateway resolves it to a configured endpoint and returns the model's reply. A wrong token gets a 401. An unknown model name gets a 404 with code `model_not_found`.

Add `"stream": true` to switch the response to a server-sent event stream of typed chunks. The stream ends in a `data: [DONE]` sentinel. If the upstream fails before streaming starts, you get a JSON 502, never a dying event stream.

The gateway serves three inference routes. Each enforces the model's declared `kind`:

- `POST /v1/chat/completions` serves chat models.
- `POST /v1/embeddings` serves models configured with `kind = "embedding"`. Batches, encoding formats, and provider token-usage statistics pass through.
- `POST /v1/rerank` serves models configured with `kind = "classifier"`. You send a query, a document set, and an optional `top_n`. You receive ranked results with relevance scores.

A rerank call looks like this:

````bash
curl http://127.0.0.1:8080/v1/rerank \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"model": "rerank-model", "query": "what is rust", "documents": ["doc one", "doc two"], "top_n": 2}'
````

A model called on the wrong route is refused with a 400 `kind_mismatch` before any queue slot is consumed or any upstream call is made.

`GET /v1/models` lists the served catalog. Each entry shows the model's id, kind, description, context size, thinking flag, and capabilities. Optional fields are omitted rather than null when unset. `GET /health` needs no authentication and always returns 200 while the gateway is serving.

Every error arrives in the standard OpenAI error envelope:

````json
{"error": {"message": "unknown model ghost", "type": "invalid_request_error", "code": "model_not_found"}}
````

This shape holds for JSON error responses and for mid-stream SSE error events.

## Model Routing and Concurrency

Each `[[model]]` entry in `gateway.toml` binds a public name to one or more endpoints. An endpoint supplies the provider's `base_url` and `api_key`. The catalog listing follows `gateway.toml` order.

You control concurrency with named pools. Declare a `[[dominion]]` and bind endpoints to it:

````toml
[[dominion]]
name = "pool"
max_concurrency = 4
max_queue = 16
````

An endpoint binds to the pool with `dominion = "pool"`. All endpoints bound to one dominion compete for a single pool of concurrency slots. The `max_concurrency` limit caps how many requests reach the backend at once. Excess requests wait in a bounded queue rather than reaching the backend.

Requests identify themselves to the fair scheduler with the `X-PromptForge-Client` header. The header defaults to `"default"`.

When all concurrency and waiting slots are exhausted, the request is rejected. You get a 503 `queue_full`, or a 429 `queue_rejected` under the fail-fast reject policy (`policy = "reject"`), so OpenAI clients see a retryable rate-limit error. A streaming request holds its slot for the stream's entire lifetime.

Two limits apply by omission. An endpoint with no `dominion` has unlimited concurrency. A dominion without `max_concurrency` is unlimited and its queue settings never engage.

Startup validation catches routing mistakes. Two models sharing one name fail startup. So does a model with no endpoints, a model naming an undefined endpoint, or an endpoint naming an undefined dominion.

## Profiles and Profile Switching

A profile is a named checklist over the global catalog. Declare profiles as `[[profile]]` tables:

````toml
[[profile]]
name = "alpha"
models = ["test-model"]

[[profile]]
name = "beta"
models = ["beta-model"]
````

Only the active profile's models appear in the served catalog. Activating a profile applies its allowlist to routing.

Switch the active profile mid-run with one call:

````bash
curl -X POST http://127.0.0.1:8080/admin/switch-profile \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"name": "beta"}'
````

The response is a server-sent event stream. It reports the stages `loading-profile`, `stopping-models`, and `starting-models`, then ends with exactly one terminal event: `ready` with the new profile name, or `error` with a message. Disconnecting from the stream never interrupts the switch.

A switch waits for in-flight inference requests to finish, up to a bounded 30-second drain window. An active conversation is never cut off mid-stream. Stragglers are then cancelled with a further 1-second grace, so a stuck request can never block a switch longer than about 31 seconds. New requests that arrive during a switch wait behind it and are then served against the new profile's routing, never the old one.

Switches are safe by construction:

- The switch uses the already-loaded catalog. It needs no disk reload, so it stays correct even if the config file on disk is corrupted after startup.
- Switching performs no inference requests and generates no backend traffic or cost.
- Two concurrent switches cannot interleave.
- Old local models shut down and free their VRAM before replacements start.
- A successful switch persists the new active profile to the state file immediately.

Profiles carry no sections, so a switch never changes `[server]` or `[workshop]`. The bearer key and listener are process-owned `[server]` state: they stay stable across profile switches. A staged edit to `[server]` or `[workshop]` promotes to disk on Apply but takes effect only on restart, which the apply response reports as `restart_required`.

Two read endpoints report the current state. `GET /admin/status` reports the active profile name, loaded model names, the model allowlist, local child count, and a queue note. `GET /admin/profiles` lists every profile name in the loaded catalog.

## Local Models and GPU Inference

Declare a `[[local_model]]` entry to run a model on your own hardware. The gateway downloads the file, verifies it, and starts a managed `llama-server` child process:

````toml
[local]
cache_dir = "/data/promptforge-cache"

[[local_model]]
name = "qwen-tiny"
description = "A tiny chat model"
source = "https://example.com/qwen-tiny.gguf"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
context = 4096
thinking = "never"
gpu_layers = 0
flash_attention = false
n_predict = 64
````

Each entry takes a name, a description, a download `source` URL, and a `sha256` checksum. Sources are pinned: an `https` URL requires a `sha256`, a local path may go unpinned, and plaintext `http` is rejected outright. Configured local models are provisioned and started automatically as part of gateway startup, with live progress while downloads run. Restarting against a warm cache relaunches without re-downloading multi-gigabyte artifacts.

Per-model settings control context size, thinking mode, GPU layer offload, flash attention, parallelism, and max predicted tokens. Setting `gpu_layers = 0` runs entirely on CPU. The `[local] cache_dir` setting chooses where downloaded model files are stored. The default is `~/.promptforge`.

Local models can be chat, embedding (`kind = "embedding"`), or reranking (`kind = "classifier"`) models. They merge into the same catalog as remote models. Clients address both by name the same way, and local requests authenticate with the same bearer key.

Two optional blocks extend a local chat model. A speculative drafter speeds up generation:

````toml
[local_model.speculative]
type = "draft-mtp"
draft_max = 2
````

The `draft_max` value is bounded to 1 through 16. The response's `timings` extension reports how many drafted tokens were accepted. A multimodal projector lets the model accept image inputs:

````toml
[local_model.multimodal_projector]
source = "https://example.com/mmproj-F16.gguf"
sha256 = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
````

The catalog then advertises `images = true` for that model.

Speech-to-text models are first-class catalog entries. Declare `[[stt_model]]` blocks alongside `[[model]]` and `[[local_model]]`. Profile-checked STT models load into the gateway-owned transcription engine and serve `POST /v1/audio/transcriptions`. STT requires the `workshop` build feature and a `[workshop]` section; the refusal names the missing section.

On Windows x86-64, the gateway picks the `llama-server` build from the host's GPUs: a Blackwell GPU (compute capability 12.x) gets the PromptForge CUDA build, any other NVIDIA GPU gets the upstream CUDA 13 build, and anything else gets the Vulkan build. Both CUDA builds ship their runtime DLLs, so the host needs only the NVIDIA driver. `[local] llama_backend` overrides the pick (`auto`, `cuda-blackwell`, `cuda`, `vulkan`), and `[local] llama_server_path` - or the `PROMPTFORGE_LLAMA_SERVER` environment variable - pins an exact executable instead of the managed download. Every download is sha256-pinned: a tampered payload is a named startup error, never a silent fallback. A `--no-default-features` build is headless: it refuses any configuration that declares `[[local_model]]`, at startup and on profile switch.

You can inspect bounded, credential-redacted stdout/stderr tails from each running local model, keyed by your configured model name. Use the tails to verify what a child actually reported, such as the CUDA device seen or layers offloaded, without reaching its private port.

## Artifact Cache and Files

Downloaded model files live in the artifact cache. The cache root comes from the `[local] cache_dir` setting, defaulting to `~/.promptforge`. Blobs land under `<cache_dir>/models/<key>/`.

Download a remote blob into the cache with one call:

````bash
curl -X POST http://127.0.0.1:8080/v1/cache \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"source": "https://example.com/model.gguf", "sha256": "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"}'
````

The `sha256` pin is optional. When present, the downloaded bytes are verified against it. The response streams live progress as server-sent events of the form `{"status": "downloading", "bytes", "total"}`. A digest mismatch arrives as a terminal error event, not a failed HTTP status. An already-cached source answers immediately with `status: "ready"` and no re-download. A failed download leaves no partial files behind. Validation failures, such as a non-http(s) URL or a pin that is not a 64-character hex digest, are rejected before any network access.

Three more endpoints manage the cache:

- `GET /v1/cache` lists every cached blob with its source URL, on-disk path, SHA-256 digest, and size. It never re-hashes any file. The listing is sorted by source URL for a stable order.
- `DELETE /v1/cache/{sha256}` removes a cached blob and its metadata. The path parameter must be a 64-character hex digest.
- `GET /admin/orphans` lists cache files no loaded `[[local_model]]` entry references, so you can find leftovers to adopt or delete. Sizes come from the filesystem and digests from cache sidecars; multi-gigabyte blobs are never re-hashed. The digest is null for files the cache API never downloaded.

On local builds, `GET /admin/model-info?path=` reads a cached GGUF file's architecture, layer count, and parameter count from its header only. It never loads tensor data. The path must be relative and confined to the artifact cache; traversal attempts are refused.

`POST /admin/reveal` opens the host OS file manager at a path confined to the artifact cache. It replies immediately with 204 without waiting for the file manager. On Windows the target file is highlighted in its folder. On macOS and Linux the parent folder opens. This endpoint backs the config UI's "reveal in folder" button and is loopback-only.

## Web Search and Speech

The gateway includes a built-in, Brave-powered web search endpoint. It is compiled in by default and active when `[tools.web_search]` is configured. POST a query and receive results:

````bash
curl -X POST http://127.0.0.1:8080/v1/tools/web_search \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"query": "promptforge gateway"}'
````

Each result carries a title, URL, site name, and extra snippets. Results are capped at 2 per host by default so no single site dominates. An empty or whitespace-only query is a 400. Calling the endpoint when web search is not configured is a 404.

With the `workshop` feature, the gateway accepts OpenAI-compatible multipart audio transcription at `POST /v1/audio/transcriptions`. Audio uploads are capped at 25 MiB; larger bodies get a 413 `file_too_large`.

## Tool-Call Dialects

Models that declare no dialect default to standard OpenAI tool calling. For models without native tool support, such as Gemma 3, set one key on the model entry:

````toml
tool_dialect = "gemma3_tool_code"
````

You then send standard OpenAI `tools` and `tool_choice` fields. The gateway converts your tool definitions into a plain-language system guide, strips the unsupported fields before the upstream call, and converts the model's `tool_code` fence replies back into standard OpenAI `tool_calls` objects with `finish_reason: "tool_calls"`. Each call gets a unique synthetic id.

The model writes Python-style calls, `name(key=<json>)`, one per line, inside a `tool_code` fence. Full JSON values round-trip as arguments. A recognized-but-malformed fence never corrupts the turn: the reply content is emptied, a `gateway_warning` field explains why, and the recovery is logged. Ordinary prose passes through untouched.

## Administration and Configuration

The gateway serves an embedded config UI at `/config/` on the gateway's own port. There is no second listener. `GET /config` redirects to `/config/`. The entire admin config surface sits behind a loopback wall: a non-loopback peer gets a bare 403 before bearer auth even runs, in every build.

Read the running configuration as JSON:

````bash
curl http://127.0.0.1:8080/admin/config \
  -H "Authorization: Bearer my-secret-key"
````

Secrets are redacted in the response. Stage a new configuration with `PUT /admin/config` using the same JSON shape. Secrets sent as `"***"` preserve the existing value.

Edits are staged as shadow files beside the real files: `gateway.toml.next`, and likewise for the state and env files. The live config is never touched and the gateway never reloads until you apply. Saves are validated before anything is written, so a bad save leaves no shadow file behind.

Three endpoints complete the staging workflow:

- `POST /admin/config-apply` promotes every staged shadow to its real file and reloads the active profile. Its response tells you which files were promoted, whether a reload happened, and whether a restart is required. Changes to `[server]`, `[workshop]`, or the env file only take effect after a restart; the `restart_required` flag tells you. After an API-key change is applied, the old key keeps working until restart. Applying with nothing staged is a clean no-op. An invalid staged config is rejected without touching live files.
- `POST /admin/config-revert` discards every staged change and nothing else.
- `GET /admin/config-pending` previews the full configuration exactly as it will look after applying, including which profile a pending change would activate. `GET /admin/config-dirty` is a cheap poll reporting whether unapplied changes exist, which files are pending, and which top-level sections would change, so you can judge blast radius.

The env file has its own pair of endpoints. `GET /admin/env` reads the gateway's `.env` file with values included, and shows which config fields reference each variable. `PUT /admin/env` stages edits to it. Env edits take effect only on restart; the process environment of a running gateway is never mutated. Variable names must start with a letter or underscore and contain only letters, digits, and underscores.

Four more read endpoints serve the config UI's views:

- `GET /admin/hf/search` and `GET /admin/hf/model/{repo}` proxy Hugging Face hub search and model details through the gateway, so the browser never holds a token. The gateway authenticates with a process `HF_TOKEN` read once at boot; with no token, public repos still work anonymously. `limit` must be 1 through 100, `sort` is only `downloads`, `trendingScore`, or `lastModified`, and `filter` is only `gguf`.
- `GET /admin/system` reports host CPU, RAM, cache-drive disk usage, and NVIDIA GPU name and VRAM. It succeeds on machines without a GPU or driver.
- `GET /admin/chat-templates` catalogs the bundled chat-template families, the built-in model-to-family mappings, and each configured chat model's effective template resolution with a plain-language reason. You can override a model's template with `chat_template_file`, either a bundled builtin such as `builtin:phi-4` or a path to a custom Jinja template.

Because the gateway is the only component holding vendor keys, rotating a credential means editing one file on one host.

## Workshop UI

The gateway can host the PromptForge Workshop UI on a second, loopback-only listener inside the same process. Enable it by adding a `[workshop]` section to the boot config:

````toml
[workshop]
open_browser = true

[workshop.stt]
window_seconds = 8
interval_ms = 250
vocabulary = ["MCP", "GGUF"]
````

Omitting the `[workshop]` section runs the gateway headless with no workshop hosted. The default bind is `127.0.0.1:7910`, and the listener must be a loopback address; anything else is a startup error.

The workshop's client URL and bearer key derive from the boot `[server]` section. No credential is duplicated and none can drift. Set `open_browser` to open the system browser at the workshop URL once it is serving; a browser that fails to open is logged, never fatal.

The `[workshop.stt]` section tunes push-to-talk transcription: `window_seconds` defaults to 15, `interval_ms` defaults to 500, and `vocabulary` lists domain terms whisper is biased toward. An empty list disables biasing. `[workshop.tape]` is accepted for compatibility and ignored: agent sessions persist their event logs under the workshop's state directory instead. Both listeners answer `/health` and `/v1/models` independently.

## Progress and Observability

During long-running operations, such as startup provisioning, model downloads, and applies, the gateway draws live progress bars in the terminal. When stderr is not a terminal, progress appears as plain log lines: `started` on first sight, a percentage line at each 5-percent advance, and `done` at completion.

Subscribe to the same events over HTTP:

````bash
curl http://127.0.0.1:8080/admin/progress \
  -H "Authorization: Bearer my-secret-key"
````

`GET /admin/progress` streams every progress event as a bearer-authenticated server-sent event stream. A fresh subscriber first receives the currently live operations replayed. Heartbeat lines every 15 seconds keep the connection alive through NAT and firewall timeouts. The apply route reports its reload stages through this stream while its own response stays plain JSON.

For routine state checks, poll `GET /admin/status`. It reports the active profile, loaded models, the model allowlist, local child count, and a queue note, along with a process-lifetime `config_generation` identifier the config UI uses to detect a restart.
