---
name: Gateway Local Inference
overview: Design for embedding local model inference (llama.cpp + Candle) into the promptforge gateway, with profiles, device groups, lanes, queuing, and remote endpoint support.
todos:
  - id: queue
    content: Add per-device/lane semaphores and fair scheduling to gateway routing
    status: completed
  - id: profiles
    content: Implement profile config loading with recursive include resolution and switch-profile admin API
    status: completed
  - id: llama-link
    content: Link llama.cpp via Rust bindings, implement model load/unload lifecycle
    status: completed
  - id: local-completions
    content: Serve local llama.cpp inference through /v1/chat/completions endpoint
    status: pending
  - id: model-download
    content: HTTP download from source URL with cache management in ~/.promptforge/models/
    status: pending
  - id: candle-gpu
    content: Extend Candle from CPU-only to CUDA/Metal for utility models (classifiers, embedders)
    status: cancelled
isProject: false
---

# PromptForge Gateway - Local Inference Design

## Core Concept

The gateway becomes the single point of inference for all promptforge workloads. It holds API keys for remote providers, loads local models into VRAM, manages concurrency, and exposes a unified OpenAI-compatible API. The executor (promptforge-cli, MCP server) never knows whether a model is local or remote - it just talks to the gateway.

## Profiles

A profile is a named configuration that describes everything loaded on the local GPU at a given moment. The gateway runs exactly one profile at a time. Switching profiles is immediate - kill in-flight local inferences, unload, load new config.

### Profile location

```
~/.promptforge/profiles/
├── base.toml
├── analytical.toml
├── threat.toml
└── paperflow.toml
```

### Profile inheritance

A profile can include other profiles recursively. Resolution order: depth-first, last-wins for conflicts on the same key.

```toml
# analytical.toml
include = ["base.toml"]

# threat.toml - inherits base via analytical
include = ["analytical.toml"]
```

Concrete semantics:
- Arrays (models, endpoints, devices) merge by appending. A child can override a parent's entry by declaring the same `id`/`name`.
- Scalars (server.bind, server.token) - child wins.
- `include` is resolved relative to the same directory as the including file.

### Profile switching

```bash
# At startup
promptforge-gateway serve --profile analytical

# At runtime (admin API)
POST /admin/switch-profile
{"name": "threat"}
```

Switching behavior:
- Immediate. No drain. In-flight local requests get an error response.
- Remote endpoint connections stay alive (they're not GPU-bound).
- The model catalog is re-advertised on the next `GET /v1/models` call.

## Devices

A device represents a physical compute resource with a concurrency constraint. Models reference their device. The queue enforces limits per-device.

```toml
[[device]]
id = "local-4090"
type = "local"            # local GPU managed by the gateway

[[device]]
id = "anthropic"
type = "remote"
concurrency = 10          # max concurrent requests to this provider

[[device]]
id = "runpod-a100-1"
type = "remote"
concurrency = 4
```

Local devices don't declare a top-level `concurrency` because their limits come from lanes (see below).

## Lanes

A lane is a concurrency slot within a device. Models declare which lane they use. This prevents a 5ms classifier from being blocked behind a 60-second LLM call, while still preventing two LLM calls from running simultaneously.

```toml
[[device.lane]]
device = "local-4090"
id = "generative"
concurrency = 1           # one LLM inference at a time

[[device.lane]]
device = "local-4090"
id = "utility"
concurrency = 8           # classifiers/embeddings can batch
```

Remote devices don't need lanes - their concurrency is flat (the remote server handles its own internal scheduling).

## Models

### Local generative models (llama.cpp)

```toml
[[local_model]]
name = "qwen-27b"
description = "Dense 27B model for structured analysis and long-context review"
source = "https://huggingface.co/bartowski/Qwen3.6-27B-GGUF/resolve/main/Qwen3.6-27B-Q4_K_M.gguf"
device = "local-4090"
lane = "generative"
context = 65536
thinking = "never"
cache_type_k = "q8_0"
cache_type_v = "q4_0"
flash_attention = true
gpu_layers = 99
```

The `source` field is a URL. On first use (or on `POST /admin/pull`), the gateway downloads the GGUF to a local cache directory (`~/.promptforge/models/`). If the file already exists, it's used directly. A local file path also works:

```toml
source = "~/.promptforge/models/custom-finetune.gguf"
```

### Local utility models (Candle, CPU or GPU)

```toml
[[local_model]]
name = "paper-classifier"
description = "Multi-hypothesis classifier for WG21 paper routing"
framework = "candle"
source = "~/.promptforge/models/paper-classifier-v1.safetensors"
device = "local-4090"
lane = "utility"
compute = "cuda"          # or "cpu" or "metal"
```

```toml
[[local_model]]
name = "bge-small"
description = "General-purpose sentence embeddings"
framework = "candle"
source = "compiled"       # special value: weights compiled into the binary
device = "local-4090"
lane = "utility"
compute = "cpu"
```

### Remote models

```toml
[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"

[[model]]
name = "claude-sonnet-4-6"
description = "Frontier model for complex analysis and coding"
context = 200000
thinking = "never"
upstream = "claude-sonnet-4-6"
endpoints = ["anthropic"]
```

Remote models have no device/lane - their concurrency is governed by the endpoint's device entry.

## Queue and Concurrency

Every request entering the gateway is routed to a device+lane. The queue enforces:

- **Per-lane semaphore** - at most N concurrent inferences per lane
- **Fair scheduling** - round-robin across callers when multiple requests are queued. No single caller starves others.
- **Backpressure** - when the queue is full (configurable depth), return HTTP 503 immediately. The executor retries or reports failure.

```toml
[queue]
max_depth = 100           # total queued requests before rejecting
fair_scheduling = true    # round-robin across client tokens
```

For remote endpoints, the device's `concurrency` is the semaphore. For local models, the lane's `concurrency` is the semaphore.

## Model Catalog and Semantic Binding

The gateway serves `GET /v1/models` with all available models (local + remote) from the active profile. The promptforge executor's semantic picker matches prompt needs (e.g. "A careful analysis model suited to structured reasoning") against model descriptions via cosine similarity, same as today.

When a profile switch occurs, the catalog changes. Any cached bindings in an executor are stale - the executor must re-fetch the catalog on its next run.

## Local Inference Engine

### Generative (llama.cpp)

The gateway links llama.cpp via Rust bindings. At profile load:
1. Download GGUF if not cached
2. Call `llama_model_load()` with configured `gpu_layers`
3. Allocate context with configured `context`, `cache_type_k`, `cache_type_v`, `flash_attention`
4. Serve completions through the same OpenAI-compatible `/v1/chat/completions` endpoint

At profile switch:
1. `llama_model_free()` - unload from VRAM
2. Load new profile's models

### Utility (Candle)

For CPU models: loaded once, stay resident across profile switches (they don't use VRAM).
For CUDA/Metal models: loaded/unloaded with the profile like generative models.

Candle models expose a different interface than chat completions (embeddings return vectors, classifiers return label scores). The gateway serves these through:
- `POST /v1/embeddings` - standard OpenAI embeddings endpoint
- `POST /v1/classify` - custom endpoint for classifier models

## Build Configuration

The gateway binary links llama.cpp and optionally CUDA/Metal via Cargo features:

```toml
[features]
default = ["local-inference"]
local-inference = ["llama-cpp-2", "candle-core"]
cuda = ["llama-cpp-2/cuda", "candle-core/cuda"]
metal = ["llama-cpp-2/metal", "candle-core/metal"]
```

A server with no GPU compiles without `cuda`/`metal` features - it still runs the gateway for remote routing and CPU-based utility models, just no local generative inference.

## Admin API

```
POST /admin/switch-profile    {"name": "analytical"}
POST /admin/pull              {"model": "qwen-27b"}       # download source URL to cache
GET  /admin/status                                         # current profile, loaded models, queue depth
GET  /admin/profiles                                       # list available profiles
```

## Example: Full Profile for Analytical Reports

```toml
# ~/.promptforge/profiles/analytical.toml
include = ["base.toml"]

[[device]]
id = "local-4090"
type = "local"

[[device.lane]]
device = "local-4090"
id = "generative"
concurrency = 1

[[device.lane]]
device = "local-4090"
id = "utility"
concurrency = 8

[[local_model]]
name = "qwen-27b"
description = "Dense 27B model for structured analysis and long-context review"
source = "https://huggingface.co/bartowski/Qwen3.6-27B-GGUF/resolve/main/Qwen3.6-27B-Q4_K_M.gguf"
device = "local-4090"
lane = "generative"
context = 65536
thinking = "never"
cache_type_k = "q8_0"
cache_type_v = "q4_0"
flash_attention = true
gpu_layers = 99
```

```toml
# ~/.promptforge/profiles/base.toml

[server]
bind = "127.0.0.1:8081"
token = "${PROMPTFORGE_TOKEN}"

[queue]
max_depth = 100
fair_scheduling = true

[[device]]
id = "anthropic"
type = "remote"
concurrency = 10

[[endpoint]]
id = "anthropic"
protocol = "openai"
base_url = "https://api.anthropic.com/v1"
api_key = "${ANTHROPIC_API_KEY}"

[[model]]
name = "claude-sonnet-4-6"
description = "Frontier model for complex analysis and coding"
context = 200000
thinking = "never"
upstream = "claude-sonnet-4-6"
endpoints = ["anthropic"]

[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
```

## Operator Workflow

```bash
# Prepare for analytical work
promptforge-gateway serve --profile analytical

# In another terminal (or in a script)
promptforge run briefer.md "The C++ Alliance"

# Switch to threat analysis
curl -X POST http://localhost:8081/admin/switch-profile -d '{"name":"threat"}'

# Run threat workload
promptforge run threat-briefer.md "Target Organization"
```

Or in a batch script:

```bash
#!/bin/bash
curl -s -X POST http://localhost:8081/admin/switch-profile -d '{"name":"analytical"}'
promptforge run briefer.md "Subject A"
promptforge run briefer.md "Subject B"
promptforge run briefer.md "Subject C"

curl -s -X POST http://localhost:8081/admin/switch-profile -d '{"name":"paperflow"}'
promptforge run classify-paper.md "P3456R2.md"
```

## Implementation Layers (in build order)

1. **Queue + concurrency** - add semaphores and fair scheduling to existing gateway routing. Works immediately for remote endpoints. Easiest, immediately useful.
2. **Local generative inference + model download** - DONE via managed `llama-server` subprocess (not in-process FFI; revisit if IPC/lifecycle fails). Gateway owns download/caching under `~/.promptforge/`, spawns one child per `[[local_model]]`, serves through `/v1/chat/completions`.
3. **Profile system** - config loading, inheritance resolution, `switch-profile` admin API, profile directory scanning. Now meaningful because there are models to load/unload.
4. **Local utility models on GPU** - extend Candle integration from CPU-only (tool-picker) to CUDA/Metal for classifiers and embedders that need speed. Only when the paper classifier demands it.
5. **Core-tests migration** - DONE. core-tests writes a profile TOML and launches promptforge-gateway; download/cache/spawn live in the gateway.

## Execution Method

Follow [vibe-rulebook.md](c:\Users\Vinnie\src\cursor\tools-public\rulebooks\vibe-rulebook.md) and [rust-rulebook.md](c:\Users\Vinnie\src\cursor\tools-public\rulebooks\rust-rulebook.md) with these constraints:

### From the vibe-rulebook

- Work each step in subagents: write code in one, review in a second (fresh context), fix in a third.
- **One round of review only.** No looping. Review, fix, amend, move on.
- Keep the main session holding the plan clean - dispatch all reads, edits, and builds to subagents.
- Pass results between subagents via `vibe-review.md`, overwritten each cycle.
- Look outward (web search) when stuck on toolchain or FFI issues rather than guessing.
- Reversible calls are made without asking. Irreversible ones (architectural changes, public API shape) are in this plan.

### From the rust-rulebook

- **Feature gates are additive.** `local-inference`, `cuda`, `metal` are opt-in features. Building without them compiles and serves remote endpoints only. Gate entire modules with `#[cfg(feature = "...")]`.
- **Errors are concrete.** llama.cpp FFI failures get a `thiserror` error type in the gateway crate. No `anyhow` in library code.
- **Unsafe is minimal and wrapped.** All C FFI `unsafe` blocks live in one wrapper module with `// SAFETY:` comments. The rest of the gateway never sees `unsafe`.
- **Blocking inference runs off the runtime.** llama.cpp calls go through `spawn_blocking` or a dedicated thread. Never block the tokio executor.
- **Test in the same commit.** Queue gets unit tests with mock endpoints. Profile loader gets unit tests with temp TOML files. Integration tests use the tiny Qwen3-0.6B GGUF.
- **Module structure stays flat.** New modules: `queue.rs`, `profile.rs`, `inference.rs` (or `inference/` if it grows). Split at 500 lines.
- **Config types are future-proof.** `#[non_exhaustive]` on profile/model config structs. Derive `Debug`, `Clone`, `Deserialize`.
