# Local Models

This chapter teaches you to run models on your own machine through the gateway. You will learn to declare a local model, how the gateway provisions and verifies it, and how the managed child processes behave. Local models share the gateway's OpenAI routing with remote models, so everything you learned about the catalog still applies.

## Declare a local model

A gateway-hosted model is a `[[local_model]]` entry. Start with the smallest useful declaration:

````
[[local_model]]
name = "qwen3-local"
kind = "chat"
description = "Qwen 3 8B, local"
source = "https://huggingface.co/qwen/qwen3-8b/resolve/main/model.gguf"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
vram_gb = 6.0
context = 8192
thinking = "switchable"
````

Each entry has a `name`, a `kind`, a `description`, a `source`, and sizing and serving knobs. The source is an https URL or a local GGUF path. The knobs and their defaults are: `parallel` default 1, `vram_gb`, `context`, `thinking`, `gpu_layers` default 99, `flash_attention` default true, `cache_type_k` default `q8_0`, `cache_type_v` default `q4_0`, and `n_predict` default 8192. A local model may also bind to a local dominion with the optional `dominion` key, and every model bound to the same dominion shares that dominion's concurrency limit. The gateway renders the child's launch flags directly from these knobs: the context size, the generation ceiling, the parallelism, the KV cache types, the GPU layers, the flash attention, and the chat template file.

A model downloaded from an https URL must be pinned by a `sha256` digest. A plaintext http source is rejected, even with a valid pin. A local filesystem path may be unpinned, and the path may use `~` expansion. The pin is verified after download and on every cache hit.

## The cache directory

Set `[local].cache_dir` for GGUF files and the pinned llama-server install:

````
[local]
cache_dir = "~/.promptforge"
````

The default is `~/.promptforge`, or `%USERPROFILE%\.promptforge` on Windows, where the location inherits the per-user ACL. Models land in `<cache_dir>/models`, keyed by a hash of the full source URL, so two distinct URLs that share a filename never collide. The llama.cpp runtime installs in `<cache_dir>/llama.cpp`.

On Windows x86-64 you can pick the llama-server build with `[local].llama_backend`: `auto`, `cuda-blackwell`, `cuda`, or `vulkan`. The `auto` setting picks from the host's GPUs. You can also force an explicit llama-server executable with `[local].llama_server_path`; it wins over the `PROMPTFORGE_LLAMA_SERVER` environment variable and the managed download.

## What runs underneath

Local inference runs on a pinned llama-server build, b10082. The gateway prefers GPU-enabled archives per platform: Vulkan on Windows and Linux, Metal on macOS. The gateway never compiles native dependencies at runtime; it downloads, verifies, stages, and launches pinned archives. A completed runtime install records its archive pins and a tree digest in a marker file, and a valid install skips re-extraction on later starts.

The gateway runs one managed llama-server child per configured `[[local_model]]`. Children get supervised respawn and deterministic teardown. Staged CUDA bundle directories are prepended to the child process's PATH only; the gateway's own environment is never mutated. Local models appear to clients as ordinary routed models under their configured names.

A local model's `kind` selects the child's serving mode: embedding models serve embeddings, and classifier models serve reranking. The `parallel` key sets both the child's concurrency and its admission limit. The thinking setting changes the child's sampling preset: thinking models sample at temperature 1.0 and top-p 0.95, while non-thinking models run with reasoning switched off and sample at 0.7 and 0.8.

## Chat templates

A local chat model needs a chat template. The gateway resolves one through a fixed precedence:

1. An explicit `chat_template_file` path.
2. A `chat_template_file = "builtin:<family>"` setting.
3. A known-override match.
4. The GGUF embedded template.

A model with no usable template refuses to launch, and the error names the model and the fix. The bundled catalog has twelve template families: ChatML, Llama 3, Llama 3.1, Qwen 2.5, Qwen 3, Gemma 3, Gemma 4, Mistral, Phi 3, Phi 4, GPT OSS, and Zephyr. Family names accept documented aliases, and case and surrounding whitespace are ignored. The gateway also recognizes 181 revision-pinned Hugging Face repository IDs and maps each to its family automatically. Models with a known-broken embedded template are silently repaired with a bundled corrected template. The configuration UI can show the effective template source and a plain-language reason before the model is downloaded.

## Reading the GGUF header

The gateway reads the architecture, the layer count, the parameter count, and the embedded chat template straight from each GGUF header, without loading tensor data. A malformed or hostile GGUF is rejected with a typed error instead of a crash or an unbounded read.

## Companion artifacts

Attach a speculative-decoding drafter to a chat model with a `[local_model.speculative]` sub-table:

````
[local_model.speculative]
type = "draft-mtp"
source = "https://huggingface.co/qwen/qwen3-8b/resolve/main/drafter.gguf"
sha256 = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789"
draft_max = 4
````

The only type is `draft-mtp`, and `draft_max` is bounded to 1 through 16. Attach a multimodal projector with a `[local_model.multimodal_projector]` sub-table carrying a `source` and a `sha256` pin; a model with a projector accepts image inputs.

Companion artifacts follow the main-model source rule: an https URL must be pinned, a local path may be unpinned, and plaintext http and empty sources are rejected. Companions on a non-chat model kind fail validation. Companions are provisioned and pin-verified before the child launches, and any failure aborts the launch.

## Downloads and verification

Artifact downloads are bounded. The connect timeout is 30 seconds, the whole-request ceiling is 2 hours, and a single artifact is capped at 256 GiB. Cache lookups refuse path traversal and absolute paths before any file is read, so a crafted model path cannot escape the cache root. An interrupted download resumes from the partial file's offset when the source URL still matches. A partial download from a different source restarts from zero. A pin mismatch on a cached blob is repaired by re-downloading. Once a blob passes its pin check, later runs and profile switches skip re-hashing. When a runtime download fails and an older verified install exists, the gateway uses the cached install with a warning. Bundled runtime assets, including the chat templates, are written into the cache only after a SHA-256 verification pass, and a cached copy whose bytes have drifted is repaired from the bundled copy.

Authenticate gated Hugging Face downloads with the `HF_TOKEN` or `HUGGING_FACE_HUB_TOKEN` environment variable. The token is attached only to HTTPS requests to huggingface.co and its subdomains.

Models downloaded from Hugging Face get a metadata sidecar file beside the cached GGUF. The sidecar records the source URL, the fetch time, the chat template, and an optional model card excerpt.

## Startup and supervision

Startup reports a structured progress tree. One subtree covers the llama-server runtime, and each local model gets download, verify, and ready stages. On a TTY you see progress bars; otherwise the progress appears as log lines.

Startup is best-effort. Every model that launched keeps serving, and each model that failed is reported by name with its error. One bad model never blocks the rest. Startup failures are classified as plausibly transient or permanent, and the classification annotates the respawn diagnostics you see in the logs.

Each child server listens only on loopback, and each launch uses a fresh random alias and bearer key, so other processes on the machine cannot ride the local endpoint. Responses still carry your configured model name. Startup waits up to 180 seconds for a child to become ready, and a port collision retries on a fresh port up to four times.

A child that dies is transparently respawned on the same port, alias, and key, with a 3 second cooldown between attempts so a crash loop cannot storm. Only transport-level deaths trigger a respawn, and an explicitly shut-down child is never respawned. A profile switch cancels and terminates even an in-flight respawn. Teardown is bounded to 5 seconds, so shutdown and profile switches never hang.

Child stdout and stderr are captured into bounded tails with the credential redacted. You can pull the tails per model as diagnostics; they include the CUDA device report and per-model GPU offload lines. At startup the gateway also probes each local chat model to detect native tool-call support and picks the correct tool-calling dialect from the evidence.

On Windows the child runs at below-normal priority with no console window, so weight loading and inference yield to interactive desktop use.

## What callers can do

Local chat completions accept deterministic sampling parameters such as `temperature`, `seed`, `presence_penalty`, and `max_tokens`. They also accept tool definitions and, with a projector, image inputs. Chat completions on a speculative-drafted model expose decoding statistics in the response's timings extension: `draft_n` and `draft_n_accepted`.

