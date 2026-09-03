# Running Local Models With The PromptForge Gateway

The PromptForge gateway can run LLM inference entirely on your own machine. You declare a model once. The gateway downloads the weights, verifies them against your SHA-256 pin, installs a prebuilt llama-server build matched to your platform, and picks the correct chat template automatically. You install nothing and compile nothing. This guide shows you how to add models, control their chat templates, run and supervise them, and manage the cache.

## Local models at a glance

A local model is an entry in the gateway configuration. You declare each one in its own `[[local_model]]` table. The entry names a source: an HTTP(S) URL to a GGUF file, or a path to a GGUF file already on your disk.

The gateway runs each local model as a managed llama-server child process. The gateway selects the correct prebuilt binary for your operating system and CPU architecture. Windows and Linux use Vulkan builds. macOS uses Metal builds on both Intel and Apple Silicon. Windows ARM64 uses a CPU build, because the pinned release has no Vulkan build for that platform. Windows x86-64 CUDA builds stage an embedded CUDA bundle instead of downloading a server archive.

Local models appear in the gateway routing table exactly like remote models. You call them through the same OpenAI-compatible endpoints. Chat completions, streaming, embeddings, and reranking work the same way for local and remote models.

The Config UI is your operator surface. It shows pending downloads, chat template decisions, and model metadata. There is no separate command-line tool for local models.

## Adding and downloading models

To add a model, give its `[[local_model]]` entry a `source`. The simplest source is an HTTP(S) URL to a GGUF file.

````toml
[[local_model]]
source = "https://huggingface.co/unsloth/gemma-4-31b-it-GGUF/resolve/main/gemma-4-31b-it-Q4_K_M.gguf"
````

You can also point `source` at a file already on your disk. Paths under your home directory work. The gateway expands `~` for you. A path source skips the download stage.

````toml
[[local_model]]
source = "~/models/my-model.gguf"
````

Pin a model to an exact SHA-256 digest with the `sha256` key. The gateway verifies the weights after download and on every cache hit. A pin is exactly 64 lowercase hexadecimal characters.

````toml
[[local_model]]
source = "https://huggingface.co/unsloth/gemma-4-31b-it-GGUF/resolve/main/gemma-4-31b-it-Q4_K_M.gguf"
sha256 = "9f2c1ab4e0d7..."
````

Downloads land in a private cache. The default cache root is `~/.promptforge`. You choose a different location with `[local].cache_dir`.

````toml
[local]
cache_dir = "~/models-cache"
````

The cache is private to your account. On Unix the gateway sets the root to mode 0700 and re-checks it. On Windows the gateway strips inherited permissions and grants your account sole full control. If the cache cannot be made private, the gateway refuses to operate.

A download never appears half-written. The gateway stages each artifact in a `.part` location and renames it into place only after verification. A verified marker then lets later runs skip the re-hash of multi-gigabyte weights. If a cached model fails its pin, the gateway repairs it with a clean re-download instead of failing.

For gated Hugging Face models, set `HF_TOKEN` in the environment. `HUGGING_FACE_HUB_TOKEN` is the fallback. The token is attached only to HTTPS requests to huggingface.co and its subdomains.

While a model or server is provisioned, you can watch live progress of the download, verify, and extract stages. Downloads allow a 30-second connect timeout and a 2-hour whole-request ceiling. Each artifact is capped at 256 GiB.

## Chat templates

A chat template formats the conversation for the model. The wrong template produces broken output, so the gateway resolves the template for you.

With no configuration at all, the gateway uses the chat template embedded in the GGUF file. For over a hundred popular open-weight models, the gateway also resolves the correct template family automatically from the exact Hugging Face repository identifier. Matching is exact only. Partial names are rejected, so a model is never silently assigned the wrong template.

You can override the default in two ways. Set `chat_template_file` to a `builtin:<family>` alias to select one of the twelve bundled families.

````toml
[[local_model]]
source = "~/models/my-model.gguf"
chat_template_file = "builtin:qwen3"
````

The bundled catalog covers ChatML, Llama 3, Llama 3.1/3.2/3.3, Qwen 2.5, Qwen 3, Gemma 3, Gemma 4, Mistral, Phi 3/3.5, Phi 4, GPT-OSS, and Zephyr.

Or set `chat_template_file` to a path to your own Jinja template file. A custom path overrides everything else.

````toml
[[local_model]]
source = "~/models/my-model.gguf"
chat_template_file = "~/templates/my-template.jinja"
````

One repair is automatic. A Gemma 4 GGUF with a known-broken embedded template gets a bundled known-good replacement, matched by the template's content hash. Renamed or re-uploaded repositories still get the fix. The replacement templates ship inside the gateway, so the fix works offline.

The Config UI can preview which template a model will use before the model file downloads. It also shows a plain-language reason for the decision and reports the source as `embedded`, `known-override`, `builtin`, or `custom`.

If no usable template exists, startup fails with a message that lists the remedies: set a custom Jinja path, set a `builtin:<family>` alias, or use a GGUF with an embedded template. An unknown `builtin:` alias fails with a message that lists every valid family name.

## Running and supervising models

Start every configured local model with one action. The gateway provisions the pinned server binary, downloads and verifies each model, and spawns the child processes. If no `[[local_model]]` entry exists, startup does nothing and downloads nothing.

A server is ready only after it passes an authenticated identity check on its health endpoint, not just an open port. Each launch attempt gets a fresh random model alias and bearer API key, so the local server answers only this gateway instance. Startup allows up to 4 fresh-port attempts on bind collisions and a 180-second readiness deadline. You can interrupt a slow startup with Ctrl-C.

Best-effort startup keeps one failing model from blocking the rest. You can then inspect exactly which models failed and why.

If a local server crashes, the gateway respawns it automatically and retries the failed request. The caller does not see the outage. A respawn reuses the same port, alias, and API key, so routing stays valid. A 3-second cooldown between respawns prevents a dead model from restart-looping.

Shut down all local children explicitly to free VRAM before you switch profiles. Tearing down the runtime kills every managed child automatically.

You can read bounded captured-output tails of each running child, keyed by model name. Server logs are captured at forced verbosity, so GPU device reports and tensor-offload evidence appear in the diagnostics. API keys are redacted from logs and diagnostics.

### Model kinds and tuning

Chat is the default mode. Set `kind = "embedding"` to serve an embedding model through the gateway embeddings endpoint. Set `kind = "classifier"` to serve a reranking model through the gateway rerank endpoint.

````toml
[[local_model]]
source = "~/models/bge-reranker.gguf"
kind = "classifier"
````

Cap concurrent requests per model with `parallel`. The default is 1. Excess requests queue instead of being rejected. Bind several models to one shared `dominion` so they compete for a single concurrency pool, such as one GPU's worth of slots.

````toml
[[local_model]]
source = "~/models/my-model.gguf"
parallel = 4
dominion = "gpu0"
````

Tune per-model inference from config: `context`, `n_predict`, `gpu_layers`, `flash_attention`, `cache_type_k`, `cache_type_v`, and thinking mode. Thinking mode switches the sampling presets automatically. Thinking uses temperature 1.0 and top-p 0.95. Non-thinking uses temperature 0.7 and top-p 0.8.

Two companions extend a chat model. Declare a pinned draft model under `[local_model.speculative]` to speed up generation with speculative decoding. Declare a pinned multimodal projector companion to add vision input. A bad companion aborts startup before any child spawns.

Tool calling needs no configuration. The gateway probes each freshly started chat server and detects the correct tool-call dialect automatically. A model whose dialect cannot be determined fails loudly at startup.

## Managing the cache

All downloaded artifacts live under the cache root. Cache slots are keyed by a hash of the full source URL, so two URLs that share a filename never collide.

You can check whether a URL is already cached, optionally against a digest pin, without triggering a download. You can list every cached blob with its source URL, SHA-256, and size. Listing reads only metadata sidecars, so it stays cheap even for multi-gigabyte entries.

Delete a cached blob by its SHA-256 digest. The blob, its metadata sidecar, and its verify marker are removed in one operation. Run the orphan scan to find files on disk that no configured local model references, so you can reclaim disk space.

Cached artifacts are served to clients through the gateway `/v1/cache` routes.

Each GGUF downloaded from Hugging Face gets a metadata sidecar beside it. Open it in any text editor to see the model's source URL, fetch timestamp, chat template, and model card excerpt.

## Limits and safety rules

The managed server is a single pinned llama.cpp release, b10082. Every platform asset carries a hardcoded SHA-256. The bundled chat templates are validated against b10082 only. A platform with no managed build produces an explicit error naming your OS and CPU architecture.

Every cache path is confined to the cache root. Paths with `..` or absolute prefixes are rejected. Every path component is checked for symlinks and reparse points. Deletion refuses symlink or reparse-point targets. Archives are extracted defensively: zip-slip entries, absolute paths, symlinks, and device nodes are rejected.

GGUF header parsing is bounded. Metadata entries and tensors are capped at 65,536. Strings are capped at 4,096 bytes. Embedded chat templates are capped at 1 MiB. Big-endian GGUF files are not supported. A malformed file fails with a typed error that names the file and the reason.

CUDA on Windows requires an installed CUDA Toolkit. The versioned variable, for example `CUDA_PATH_V13_3`, takes precedence over the generic `CUDA_PATH`. A missing toolkit produces an error that names the exact version and the variables to set.

Every failure produces a specific, actionable message. A digest mismatch shows the expected and actual hashes. A cache-privacy failure names the root and tells you to restrict it and re-run. A readiness timeout shows the deadline. Errors are classified as retryable or permanent, so transient faults are retried and permanent faults are reported as-is.
