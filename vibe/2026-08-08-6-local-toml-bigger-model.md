---
name: local.toml bigger model
overview: Create inherited local profiles under `C:\Users\Vinnie\cursor\` (common/gemma/qwen), wire HF_TOKEN for official Gemma downloads, and purge stale model caches in `~/.promptforge` and the promptforge repo.
todos:
  - id: hf-token-download
    content: "Gateway: send HF_TOKEN/HUGGING_FACE_HUB_TOKEN on Hugging Face downloads"
    status: completed
  - id: write-profile-hierarchy
    content: Write C:\Users\Vinnie\cursor\{common,gemma,qwen}.toml with include inheritance
    status: completed
  - id: purge-stale-caches
    content: Purge stale ~/.promptforge and repo .model-cache leftovers against the keep-set
    status: completed
isProject: false
---

# Local gateway profiles: common + gemma + qwen

## Layout

Directory: `C:\Users\Vinnie\cursor\` (outside the repo; not committed).

| File | Role |
|---|---|
| [`common.toml`](C:\Users\Vinnie\cursor\common.toml) | Shared server, queue, Brave search |
| [`gemma.toml`](C:\Users\Vinnie\cursor\gemma.toml) | `include = ["common.toml"]` + official Gemma 3 27B IT QAT Q4_0 |
| [`qwen.toml`](C:\Users\Vinnie\cursor\qwen.toml) | `include = ["common.toml"]` + Qwen3.5-9B Q4_K_M (existing pin) |

Serve a leaf file, not common:

```text
promptforge-gateway serve C:\Users\Vinnie\cursor\gemma.toml
promptforge-gateway serve C:\Users\Vinnie\cursor\qwen.toml
```

Includes resolve relative to the file (gateway design). Array merge: child `[[local_model]]` appends; same `name` replaces.

No Anthropic endpoint in this hierarchy (local-only profiles).

## `common.toml`

```toml
[server]
bind = "127.0.0.1:8081"
key = "${PROMPTFORGE_GATEWAY_KEY}"

[queue]
max_depth = 100
fair_scheduling = true

[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
```

## `gemma.toml`

```toml
include = ["common.toml"]

[[local_model]]
name = "gemma-27b"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "https://huggingface.co/google/gemma-3-27b-it-qat-q4_0-gguf/resolve/main/gemma-3-27b-it-q4_0.gguf"
sha256 = "<fill at write time from authenticated HF LFS oid, or omit if unavailable>"
context = 65536
thinking = "never"
gpu_layers = 99
flash_attention = true
cache_type_k = "q8_0"
cache_type_v = "q4_0"
n_predict = 8192
```

## `qwen.toml`

```toml
include = ["common.toml"]

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf"
sha256 = "03b74727a860a56338e042c4420bb3f04b2fec5734175f4cb9fa853daf52b7e8"
context = 65536
thinking = "never"
gpu_layers = 99
flash_attention = true
cache_type_k = "q8_0"
cache_type_v = "q4_0"
n_predict = 8192
```

Description matches briefer’s `models.always` so either profile binds as the writer.

## Gateway change (needed for official Gemma)

In [`promptforge/crates/promptforge-gateway/src/local/artifacts.rs`](promptforge/crates/promptforge-gateway/src/local/artifacts.rs) `download`: if `HF_TOKEN` or `HUGGING_FACE_HUB_TOKEN` is set, add `Authorization: Bearer <token>`. Prefer `HF_TOKEN`. Never log the token. Cover with a unit test (header present when set, absent when unset).

Operator still needs the Gemma license accepted on the model page and `HF_TOKEN` in the environment when serving `gemma.toml`. Qwen does not need a Hugging Face token.

## Purge stale caches

**Keep-set** (do not delete):

| Artifact | Why |
|---|---|
| `Qwen3.5-9B-Q4_K_M.gguf` (+ matching digest) | `qwen.toml` |
| `gemma-3-27b-it-q4_0.gguf` (+ matching digest once known) | `gemma.toml` |
| `Qwen3-0.6B-Q8_0.gguf` (+ scenario digest) | core-tests scenarios still pin it |
| Current `llama.cpp/b10082-*` install tree | current gateway pin |

**Operator cache** (`%USERPROFILE%\.promptforge` / `~/.promptforge`):

- Under `models/` and `downloads/`: remove GGUFs, `.part` partials, and other blobs whose basename or recorded digest is not in the keep-set.
- Under `llama.cpp/`: remove install trees that are not the current `b10082-*` pin.
- Leave lock files alone if a gateway is running; do this with gateway stopped.
- Report what was removed (names + sizes); do not delete the whole `~/.promptforge` root.

**Repo** ([`promptforge/`](promptforge/)):

- Delete legacy root [`.model-cache/`](promptforge/.model-cache/) if present (gitignored; superseded by `~/.promptforge`).
- Delete stray `*.part` / orphan download dirs under the repo if any.
- Do not delete `target/`, `*.store/`, or git-tracked sources.
- Leave [`gateway.toml`](promptforge/gateway.toml) and [`profiles/`](promptforge/profiles/) alone unless a file is only a cache dump (none expected).

Stale means “not referenced by the keep-set above,” not “older than N days.”
