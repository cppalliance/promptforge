# The Gateway

---

# Install and Run

This chapter teaches you to get the gateway running on your machine: how to install it, how to start it with a config file and a profile, and how to confirm it is healthy. You do these things every time you bring the gateway up, so they are worth learning well.

## Install the binary

The gateway is a single binary named `promptforge-gateway`. It serves an OpenAI-shaped inference API. Install it with cargo:

````
cargo install gateway
````

Confirm the install by printing the version:

````
promptforge-gateway --version
````

## Start the gateway

Start the gateway with one subcommand that names a config file and a profile:

````
promptforge-gateway serve gateway.toml --profile main
````

The first argument is the path to the config file. The `--profile` flag names the profile to activate. The gateway always starts from one config file and one active profile.

You can supply both values through environment variables instead of command-line arguments. The config path comes from the positional argument or from `PROMPTFORGE_GATEWAY_CONFIG`; the command line wins when both are set. The profile comes from `--profile`, then `PROMPTFORGE_PROFILE`, then the sibling state file the gateway keeps beside the config.

You can also start the gateway with no config file at all. When no `gateway.toml` exists beside the executable, in the working directory, or in the user profile's `.promptforge` directory, the first run writes a default config there - loopback-only on an OS-assigned port, with a fresh random bearer key and `trust_loopback = true` so callers on the same machine need no key - and boots from it. The generated file notes the caveat beside that line: on a shared machine any other OS account can then use the gateway, and `trust_loopback = false` requires the key from everyone. The generated config selects a profile named `default`, so a bare first boot needs no flags.

## The system tray

On a desktop system the gateway's face is the system tray. The icon shows the gateway's state, and its menu carries a status line, a Workshop item that launches the Workshop application when the installer laid it beside the gateway, a Settings item that opens the configuration UI in your browser, a Launch at Login toggle, and Quit. A gateway started at login never opens a browser or a window.

For servers and CI, `--no-tray` keeps the plain headless loop. In a tray-less environment, `--print-url` prints the Settings URL to stdout once the gateway is bound. `--browser` opens the Settings page in your default browser once bound; the installer uses it on a Gateway-only install's first run. Launching `promptforge-gateway` while one is already running never starts a second copy: it opens the running gateway's Settings page instead.

After every successful bind the gateway writes a connection file (`gateway.json` in the run directory under the state directory) carrying its port, bearer key, and process id. PromptForge components read that file to attach to the running gateway instead of starting a second one, and a clean shutdown removes it.

## Check that it is healthy

Once the gateway is serving, probe its health endpoint:

````
curl http://127.0.0.1:8081/health
````

GET /health needs no credentials. It always answers 200 while the gateway is serving.

Every /v1 route is authenticated with the shared bearer key from the config file. The address in this request is the `bind` value from the `[server]` section of the config file; `127.0.0.1:8081` is an example bind. A request with a wrong token is rejected with status 401 and error code `unauthorized`, from any peer:

````
curl -H "Authorization: Bearer wrong-token" http://127.0.0.1:8081/v1/models
````

From the gateway's own machine you can leave the key out entirely. With the default `trust_loopback = true`, a loopback request that presents no credential is admitted:

````
curl http://127.0.0.1:8081/v1/models
````

This convenience has one cost: on a shared machine, any other OS account can use the gateway the same way, including reading upstream API keys from the admin config surface. Set `trust_loopback = false` in `[server]` to require the key from every caller. The configuration chapter covers the rule in full.

## Choose what to build

Build-time feature flags decide which capabilities exist in the binary. The flags `local`, `web-search`, `stt`, and `config-ui` are on by default. A headless build without `local` refuses any configuration that declares local models; the refusal happens at startup and again on any profile switch.

## Run it as a service on Linux

On Linux the release archive contains a sample systemd unit. The unit runs the gateway as a service with a fixed config path and profile, and restarts it automatically on failure:

````
ExecStart=/usr/local/bin/promptforge-gateway serve /etc/promptforge/gateway.toml --profile main
Restart=on-failure
RestartSec=5
````

The gateway holds vendor credentials, so run it as a dedicated unprivileged user. The sample unit does this with `DynamicUser=yes` and keeps state in a systemd-managed state directory (`StateDirectory=promptforge`).

## Watch the logs

Control log verbosity through the standard `RUST_LOG` environment filter. The speech library logs at warn level by default, so it stays quiet unless you ask for more.

Startup failures appear on stderr with the full cause chain: one `error:` line followed by one `caused by:` line per cause. Once the gateway is serving, the log shows the bound address. If you configured port 0, the log reports the real bound port.

## Stop the gateway

From the tray, choose Quit. From a script or another PromptForge component, send an authenticated POST to the `/shutdown` route with the bearer key; it answers 202 and then the server goes down. Under `--no-tray`, Ctrl-C stops the gateway cleanly. Every path drains in-flight requests before the exit.

---

# The Configuration File

This chapter teaches you the shape of the one file that configures the whole gateway. You will learn the version key, the server section, how to keep secrets out of the file, when a same-machine caller needs no key, and the loopback wall that guards the admin surface. Every other chapter adds sections to this file, so a solid mental model here pays off everywhere.

## One file, one version

You configure the gateway in a single version-2 `gateway.toml` file. The file owns the global settings, the complete model catalog, and the profiles. The file must declare its version on the first line:

````
config-version = 2
````

Any other version fails to load. There is no silent upgrade path.

## A minimal configuration

A minimal configuration has one `[server]` section, one or more `[[endpoint]]` backends, and one or more `[[model]]` entries that map public names to upstream aliases:

````
config-version = 2

[server]
bind = "127.0.0.1:8081"
api_key = "${GATEWAY_KEY}"

[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"

[[model]]
name = "gpt-5"
kind = "chat"
description = "GPT-5 via OpenAI"
context = 272000
thinking = "switchable"
upstream = "gpt-5"
endpoints = ["openai"]
````

The `[server]` section sets the socket address and the shared bearer key. Every request from another machine must present the key, and a key that is presented is always checked. The key must not be empty. A third field, `trust_loopback`, controls whether callers on the gateway's own machine may skip the key. It defaults to true, which on a shared machine also admits every other OS account there; set `trust_loopback = false` to require the key from everyone. The rule is covered in full below.

The model catalog lives in the same file. Remote models are `[[model]]` entries. Local models are `[[local_model]]` entries. Speech models are `[[stt_model]]` entries. Later chapters cover each kind.

Keep the sections in the canonical order: `config-version`, `[server]`, `[workshop]`, `[local]`, `[tools]`, `[[dominion]]`, `[[endpoint]]`, `[[model]]`, `[[local_model]]`, `[[stt_model]]`, `[[profile]]`. The order minimizes merge noise when two people edit the file.

## Keep secrets out of the file

Reference environment variables in string values with `${VAR}` syntax:

````
api_key = "${OPENAI_API_KEY}"
````

A literal dollar sign is written `$$`. Interpolation runs only on string values, after the TOML is parsed, so a variable reference inside a comment or a key is never expanded. An unclosed `${...}` fails the load. A reference to an unset variable fails the load with a distinct error that names the variable.

At startup the gateway loads the config-sibling `.env` file into the process environment before it reads the config. Variables already set in the environment win. A missing variable surfaces later as the unresolved `${VAR}` error.

Secrets never serialize. When the gateway renders the configuration, every secret field shows `***` instead of credential material. You can view the running configuration rendered as JSON in TOML shape, and you can list which config fields reference each `${VAR}` variable; the values are never exposed.

## Validation never lets a bad file load

A configuration never loads without passing validation. Unknown keys in any section are rejected, never ignored. Removed layout features, such as include chains or a sibling profiles directory, fail with hard-break diagnostics that name the file, the removed key, the source line, and the replacement layout. Removed legacy keys such as `[queue]` or an endpoint's `concurrency` fail at parse time. An old config cannot silently load.

You can classify a load failure into stable kinds: unreadable file, invalid TOML, malformed interpolation, unset environment variable, failed semantic check, removed layout feature, or shadow write failure.

Two field rules are worth memorizing early. A `sha256` pin must be exactly 64 hexadecimal characters; uppercase and surrounding whitespace are accepted and normalized to lowercase. And a `[[model]]` entry without a `description` or a `context` is rejected at load.

## Loopback trust

By default a caller on the gateway's own machine needs no key. With `trust_loopback = true` (the default, and what the first-run config writes), a request from a loopback peer that presents no credential at all is admitted on every route, the admin surface included. That is what lets `curl http://127.0.0.1:8081/v1/models`, the SDK with only `PROMPTFORGE_GATEWAY_URL` set, and the config UI on its own origin work without a key.

The trust is narrow on purpose. It applies only when the request carries no `Authorization` header: a presented-but-wrong bearer is still rejected with 401, even from loopback, so a stale key is always detected. And it applies only when the request's fetch metadata allows ambient access: no `Sec-Fetch-Site` header (curl, the SDK, any non-browser client) or a value of `same-origin` or `none` (the config UI, a typed URL). A page on another origin sends `cross-site`, and browsers never let a page strip that header, so a web page cannot ride your loopback peer into the admin surface. A request with no peer address fails closed and needs the key.

The cost is the shared-machine case. On a machine with more than one OS account, any other account can use your gateway, including reading upstream API keys from the admin config surface. If that describes your machine, set `trust_loopback = false` to require the bearer key from every caller, or bind the gateway off loopback:

````
[server]
bind = "127.0.0.1:8081"
api_key = "${GATEWAY_KEY}"
trust_loopback = false
````

`[server]` is process-owned, so a change to `trust_loopback` takes effect on the next restart.

## The loopback wall

The admin config endpoints sit behind a loopback wall in every build. A non-loopback peer gets 403 before bearer auth even runs. The wall covers config read and write, the env file, pending state, apply and revert, orphans, system metrics, model info, chat templates, the Hugging Face proxy, profile create and delete, and reveal. The wall fails closed: a request with no peer address is refused. Loopback trust adds a rule to authentication; it removes no wall.

## Derived addresses

An unspecified bind IP such as 0.0.0.0 or :: becomes the matching loopback address in derived client URLs. Same-host consumers, including a hosted workshop, always get a dialable URL.

---

# Remote Models and Endpoints

This chapter teaches you to declare remote backends and the models they serve. You will learn endpoint entries, model entries, and the catalog your callers see. Remote models are the simplest way to get the gateway serving, so they come first.

## Declare an endpoint

A remote backend is a `[[endpoint]]` entry. Start with one:

````
[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
````

Each entry has an `id`, a `protocol` of `openai`, a `base_url`, an `api_key`, and an optional `dominion` binding. A trailing slash on the base URL is trimmed. Endpoint ids must be non-empty and unique. Each `base_url` must be an absolute http or https URL with a host; values like `not-a-url` or `ftp://example.com` fail validation.

## Declare a model

A remote model is a `[[model]]` entry that maps a public name to the alias the backend knows:

````
[[model]]
name = "gpt-5"
kind = "chat"
description = "GPT-5 via OpenAI"
context = 272000
thinking = "switchable"
upstream = "gpt-5"
endpoints = ["openai"]
````

Each entry has a `name`, a `kind`, a `description`, a `context` size, a `thinking` mode, an `upstream` alias, a list of `endpoints`, an optional `default_max_tokens`, and an optional `tool_dialect`. The upstream alias is the string the backend knows the model by.

Every remote model must list at least one endpoint, and every endpoint it names must be defined. Model names must be unique across remote and local models, so one name always refers to one model.

## Kinds and thinking modes

Every model carries a `kind`: `chat`, `embedding`, or `classifier`. The kind scopes which fields are meaningful. Chat-only fields such as `thinking` and `default_max_tokens` are rejected for non-chat kinds at load time.

Record each chat model's thinking behavior as `never`, `always`, or `switchable`. Switchable means the client may toggle thinking per request.

## Tool dialects

The default `openai` tool dialect forwards tool definitions to the backend verbatim. For a backend without a native tool array, set the emulating dialect on a chat model:

````
tool_dialect = "gemma3_tool_code"
````

With this dialect the gateway injects a tool guide into the system prompt, strips the tool fields from the outgoing request, and parses tool fences from the reply.

## Advertise capabilities

You can advertise per-model capabilities that surface verbatim on GET /v1/models, so clients can shape requests before sending them:

````
[model.capabilities]
max_output = 16384
default_temperature = 1.0
images = true
parallel_tool_calls = true
effort_levels = ["low", "medium", "high"]
default_effort = "medium"
adaptive_thinking = true
````

The capability fields are `max_output`, `default_temperature`, `images`, `parallel_tool_calls`, `effort_levels`, `default_effort`, and `adaptive_thinking`. They obey cross-field rules at load time. A `default_effort` without `effort_levels` fails. A `default_effort` not listed in `effort_levels` fails. Effort fields fail when thinking is `never`. A `max_output` larger than `context` fails; an exact fit passes.

Enumerated fields accept a fixed spelling vocabulary. Use the spellings verbatim: protocol `openai`; thinking `never`, `always`, or `switchable`; tool_dialect `openai` or `gemma3_tool_code`; model kind `chat`, `embedding`, or `classifier`.

## What the caller sees

Callers observe the catalog at GET /v1/models:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/v1/models
````

Each configured model carries its caller-facing id, its workload kind, its description, its context window size, its thinking mode, and its capability metadata.

When a caller sends a chat, embedding, or rerank request, the gateway forwards it to the backend paths `chat/completions`, `embeddings`, or `rerank` relative to the configured base URL. The public model name is rewritten to the upstream alias. The caller's bearer token is never sent upstream.

---

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

Startup reports a structured progress tree. One subtree covers the llama-server runtime, and each local model gets download, verify, and ready stages. Progress renders as tracing log lines on every stream.

Startup is best-effort. Every model that launched keeps serving, and each model that failed is reported by name with its error. One bad model never blocks the rest. Startup failures are classified as plausibly transient or permanent, and the classification annotates the respawn diagnostics you see in the logs.

Each child server listens only on loopback, and each launch uses a fresh random alias and bearer key, so other processes on the machine cannot ride the local endpoint. Responses still carry your configured model name. Startup waits up to 180 seconds for a child to become ready, and a port collision retries on a fresh port up to four times.

A child that dies is transparently respawned on the same port, alias, and key, with a 3 second cooldown between attempts so a crash loop cannot storm. Only transport-level deaths trigger a respawn, and an explicitly shut-down child is never respawned. A profile switch cancels and terminates even an in-flight respawn. Teardown is bounded to 5 seconds, so shutdown and profile switches never hang.

Child stdout and stderr are captured into bounded tails with the credential redacted. You can pull the tails per model as diagnostics; they include the CUDA device report and per-model GPU offload lines. At startup the gateway also probes each local chat model to detect native tool-call support and picks the correct tool-calling dialect from the evidence.

On Windows the child runs at below-normal priority with no console window, so weight loading and inference yield to interactive desktop use.

## What callers can do

Local chat completions accept deterministic sampling parameters such as `temperature`, `seed`, `presence_penalty`, and `max_tokens`. They also accept tool definitions and, with a projector, image inputs. Chat completions on a speculative-drafted model expose decoding statistics in the response's timings extension: `draft_n` and `draft_n_accepted`.

---

# Speech-to-Text

This chapter teaches you the gateway's transcription surface: how to declare speech models, how the interim and final roles work together, and what the transcription endpoint serves. Speech builds on local models, because speech models are provisioned and cached the same way.

## Declare speech models

A speech-to-text model is a `[[stt_model]]` entry:

````
[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
vram_gb = 1.0
````

Each entry has a `name`, a `role` of `interim` or `final`, a `source`, an optional `sha256` pin, a `vram_gb` estimate, and an optional `dominion` binding. The interim role transcribes while a take is still recording. The final role crystallizes completed audio.

A profile may select at most one interim and one final STT model. A final model requires an interim partner. Interim-only is a supported degraded mode. You can restore a built-in recommended pair at any time: whisper-base-en for interim and whisper-small-en for final, both carrying canonical whisper.cpp URLs and SHA-256 pins.

## Tune push-to-talk capture

Tune capture in the optional `[workshop.stt]` section:

````
[workshop.stt]
window_seconds = 15
interval_ms = 500
vocabulary = ["MCP", "GGUF", "Lua"]
````

The `window_seconds` key sets the seconds of trailing audio transcribed per pass (default 15), and `interval_ms` sets the milliseconds between passes (default 500). Each must be at least 1; a zero value fails startup. The `vocabulary` lists domain terms that bias both transcription workers toward those terms. An empty list disables biasing. A vocabulary that exceeds the model's prompt budget is truncated, and a warning is logged.

## The transcription endpoint

With the default-on `stt` feature the gateway serves OpenAI-compatible audio transcription at POST /v1/audio/transcriptions. The multipart form accepts `file`, `model`, `language`, `prompt`, `temperature`, `response_format`, and the repeated field `timestamp_granularities[]`.

Uploads are capped at 25 MiB; an over-limit upload is answered with "audio file exceeds the 25 MiB limit". Only 16 kHz mono WAV audio is accepted. Other sample rates or channel counts are rejected with a message naming what was received.

Two response shapes are offered. The `json` shape returns text only. The `verbose_json` shape returns task, language, duration, text, segments, and words. Segment timestamps are on by default, and word timestamps are always empty. A transcription request for a model not loaded in the active profile is rejected as an unknown model. A caller-supplied `temperature` must be a finite non-negative number. The `prompt` hint is accepted but ignored by the current English whisper workers.

## The runtime

Speech-to-text runs on a separately pinned whisper.cpp library bundle, b4938. A library that does not match the pinned layout fails to load, and only 64-bit targets are supported. Model artifacts and the runtime are downloaded and verified into the configured cache directory at startup, with progress reporting. Each model file is prewarmed and then loaded, with progress per model.

STT startup failures are named by stage: opening the artifact store, provisioning the whisper library, provisioning a named model, a missing interim partner, an unsupported role, or engine load. Library load failures name the failing path or symbol in the logs.

## How a take is transcribed

A recorded take is split into speech segments at silence boundaries. A segment closes only after 2 seconds of trailing silence, so sentence-internal pauses survive. Speech bursts shorter than 250 ms are discarded as clicks. Audio quieter than -60 dBFS is treated as silence and never sent to the model, and fragments shorter than half a second are gated out.

With a final model configured, completed speech segments are re-transcribed in the background while the take still records, and each segment's text is reported as it finishes. Without a final model, the stop falls back to the interim model. Silent or very short fragments are skipped so the model does not invent text for them. Transcription is pinned to English, and translation is disabled.

## The streaming socket

The streaming speech-to-text WebSocket at /stt exists for the workshop listener, but no product binary currently serves it: the desktop application hosts the workshop server itself, and voice input migrates into that server in a later change. Until then the socket and its `GET /stt/capability` probe go unanswered, and dictation in the Workshop stays blocked. What follows is the wire contract where the socket is served.

The client drives the socket with the bare text messages `start` and `stop` and binary little-endian f32 PCM audio frames. The wire contract has a `stream` frame announcing each take, `interim` frames carrying committed and tentative transcripts, and a `final` frame with the transcript and frame count. Frames carry a per-connection generation counter, and committed text is append-only across interim frames.

The /stt socket refuses cross-site browser connections: the upgrade performs an Origin allowlist check and answers 403.

During a take the status bar shows "Listening...", then "Transcribing...", then "Finalizing transcript...", and failures appear as notices. A take that overruns the interim window without a final model is truncated; the warning names the window length and the dropped lead in seconds.

Switching the active profile provisions and loads the selected speech models. Switching away unloads the engine and releases the model memory.

---

# Profiles and Switching

This chapter teaches you profiles: named checklists that decide which models the gateway serves, and how to switch between them at runtime. Profiles are how one config file serves a work machine, a travel laptop, and a demo box without editing a single model entry.

## Define a profile

A profile is a `[[profile]]` entry that owns only a `name` and a `models` list:

````
[[profile]]
name = "work"
models = ["gpt-5", "qwen3-local", "whisper-base-en", "whisper-small-en"]

[[profile]]
name = "travel"
models = ["gpt-5"]
````

Membership alone decides which models route, spawn, or load. Profiles carry no per-field overrides. A profile selects a subset of the catalog across remote, local, and STT models, and every name it lists must exist exactly once. Duplicate profile names and duplicate members fail validation.

Profile names must be a single safe path component: no surrounding whitespace, not empty, not `.` or `..`, and no path separators. One spelling works in URLs, state files, and labels.

Every profile is validated at load. Names are unique and legal, every listed model exists, and the local and STT subsets are checked against dominion VRAM budgets. A live switch can never land on an invalid profile.

## Where the active profile lives

The active profile lives in a sibling state file, not in the config. A `gateway.toml` maps to a `gateway.state.toml` holding one canonical key:

````
active_profile = "work"
````

The selection survives restarts.

At startup the profile is chosen by precedence: the `--profile` command-line flag, then the `PROMPTFORGE_PROFILE` environment variable, then the sibling state file. With none set, startup refuses and lists the defined profiles. A stale state file naming a deleted profile fails startup with an error naming the stale value and the defined profiles.

## Switch at runtime

Switch the active profile over HTTP:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" \
  -H "Content-Type: application/json" \
  -d '{"profile": "travel"}' \
  http://127.0.0.1:8081/admin/switch-profile
````

The switch streams its stages as a live SSE event stream: `loading-profile`, `stopping-models`, `starting-models`, and one terminal event. The choice persists to the state file, and the switch runs to completion even if the client disconnects. Switching uses the in-memory catalog; the config file is never re-read from disk.

Activating a profile narrows the served remote, local, and STT catalogs to that profile's member list. Selecting an undefined profile fails with the list of defined profiles.

In-flight inference requests get a bounded drain of up to 30 seconds during a switch. Stragglers are then cancelled, and a caller cancelled this way receives a dedicated error. Switching tears down the old profile's children deterministically, and their VRAM is freed before the replacement profile starts. When a switch starts only some local models, the terminal event names which models loaded and which failed.

---

# Dominions and Queues

This chapter teaches you dominions: named compute pools that cap concurrency, park or reject excess callers, and schedule waiting clients fairly. Dominions are how you keep one busy model from starving the rest.

## Declare a dominion

A dominion is a `[[dominion]]` entry:

````
[[dominion]]
id = "pool-r"
kind = "remote"
max_concurrency = 4
max_queue = 100
policy = "queue"
fair_scheduling = true
````

Each entry has an `id`, a `kind` of `remote` or `local`, a `max_concurrency`, a `max_queue` defaulting to 100, a `policy` of `queue` or `reject` defaulting to `queue`, a `fair_scheduling` flag defaulting to true, and a `vram_gb` budget for local pools.

Bind an endpoint or a local model to a dominion by name:

````
[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
dominion = "pool-r"
````

Endpoints bind to remote dominions, and local models bind to local dominions. A wrong-kind or undefined binding is rejected. An endpoint or local model without a dominion binding is unlimited; it behaves as when no cap is set at all.

## Budget VRAM

A local dominion can carry a `vram_gb` budget, and each profile's selected models must fit within it:

````
[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 2
vram_gb = 24
````

An overflow fails validation with an error naming the dominion and the excess. Fractional estimates such as 1.22 are accepted. Zero, negative, NaN, and infinite estimates fail.

## Choose a full-capacity policy

The default `queue` policy parks callers up to the depth limit. The `reject` policy turns the caller away immediately, and the gateway answers 429:

````
policy = "reject"
````

You can distinguish admission failures by status code. A full waiting queue answers 503 with code `queue_full`. A fail-fast rejection answers 429 with code `queue_rejected`. A queue torn down while the caller waited reports the queue as unavailable. A profile switch that cancels an in-flight request gets its own error: the gateway answers 503 with code `profile_switch` and the message "request cancelled for profile switch", distinct from `queue_full` and `queue_rejected`.

## Schedule fairly

Turn on fair scheduling so waiting callers are served in per-client round-robin order, keyed by the `X-PromptForge-Client` request header:

````
fair_scheduling = true
````

The header is a self-asserted hint. Values over 64 bytes or outside the alphanumeric, dash, underscore, dot, and colon charset fold into the shared `default` bucket. At most 32 distinct client labels are tracked.

## How slots behave

A streaming request holds its dominion concurrency slot for the stream's whole lifetime, so a second request waits until the first stream ends. A cancelled queued request frees its waiting slot, and capacity recovers without a restart.

---

# Editing Configuration Safely

This chapter teaches you the safe-edit surface: how the gateway stages edits in shadow files, how you preview and apply them, and how you recover when an edit is wrong. Editing through this surface means a bad config can never take down a running gateway.

## Shadow files

Pending admin edits are staged in shadow files: `gateway.toml.next` and `gateway.state.toml.next`. No save touches a real file until promotion.

Stage a full config edit with PUT /admin/config. The request takes the same JSON shape that GET /admin/config returns. Secrets left as the redacted marker `***` are restored from the current values, and a marker with no existing value fails validation. The merged result is validated like a real load before any shadow is written.

## Preview before you apply

Preview the merged pending configuration with secrets still redacted:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-pending
````

Poll a cheap dirty report of pending shadow files and changed sections, including `active_profile`:

````
curl -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-dirty
````

## Apply

Applying a pending edit is an explicit promote step:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-apply
````

The real file is replaced atomically. On platforms where rename cannot overwrite, a backup-and-restore fallback preserves the old file. The reply carries `applied`, `reloaded`, and `restart_required`. The reply tells you when an edit needs a process restart to take effect: an env shadow or a change to `[server]` or `[workshop]` requires a restart. The apply's reload stages stream on the live progress stream; the apply response carries only the outcome.

An apply that changes the config or the state runs as a command on the gateway's command queue, the same queue that runs profile switches and boot provisioning. The request waits for the command's outcome, so the call above still returns when the apply is done. While the command runs, `GET /admin/status` reports it as the active command named `apply-config`, and the config UI's Apply overlay follows its stages and carries a Cancel button. `POST /admin/queue/cancel` stops it; the request then answers 503 with error code `apply_cancelled`. An apply supersedes any profile switch in flight, including the boot load, because the applied configuration is the one you want running; a profile switch requested during an apply waits behind it. An apply that touches only the env file, or only a process-owned section, needs no reload and runs inline without a command.

Promotion happens at the end. The shadow files are read into memory when the apply is requested, the new configuration is downloaded and started, and only then are the captured bytes written to the real files and the shadows removed. A cancelled or failed apply therefore promotes nothing: every shadow stays on disk, the pending count stays where it was, and the next Apply runs the whole thing again. A save that lands while an apply is in flight is kept as the next pending change, never silently lost and never half-applied.

## Revert

Discard every staged edit without touching the real files:

````
curl -X POST -H "Authorization: Bearer $GATEWAY_KEY" http://127.0.0.1:8081/admin/config-revert
````

The reply names the deleted shadow files. Deleting the shadows is the whole revert.

## Profiles and shadows

You can switch the active profile immediately without consuming an unapplied state shadow staged by the config UI. Loading prefers shadow files over real files, while command-line and environment profile selections still outrank pending state.

## The .env file

Read and stage the gateway's global `.env` file over the same surface. GET /admin/env returns the file with plaintext values and shows which config fields reference each variable. PUT /admin/env stages a `.env.next` shadow that takes effect after restart. Variable names must use letters, digits, and underscores, and must not start with a digit. Values must round-trip through the dotenv parser.

## Failure behavior

You are protected from half-applied state. Saves, revert, and the apply's snapshot and commit steps serialize on one lock, and applies serialize with profile switches on the command queue. An invalid pending config is never promoted; the request fails before any command exists. A failed or cancelled apply leaves every shadow on disk for correction, retry, or revert. A revert issued during an apply cancels the apply first, so the apply's commit never writes over files you just reverted. A failed state-shadow write rolls the config shadow back to its previous contents.

---

# The Configuration UI

The gateway serves a browser UI for configuration: you reach it over HTTP, sign in with your API key when the gateway asks for one, and edit every part of the configuration through its views. The UI rides the safe-edit surface from the previous chapter, so everything you do there moves through pending shadows and Apply.

## Reach the UI

The gateway serves the configuration UI at /config on its own port; there is no second listener. The UI is an optional feature you compile in with `config-ui`, which is on by default. GET /config redirects permanently to /config/. Five asset endpoints live under /config: the index page at /, the bundled script at /app.js, the stylesheet at /app.css, and the program icon at /icons/promptforge-icon.png with its high-DPI render at /icons/promptforge-icon@2x.png.

The UI pages need no bearer token, but every asset route answers 403 Forbidden to any peer that is not loopback. The UI is reachable only from the gateway machine itself, and the check fails closed.

## Sign in

With the default `trust_loopback = true`, the UI opens straight into the shell: it runs on the gateway's own machine, and the gateway admits a loopback caller that presents no key. On a shared machine that same trust admits every other OS account, so an operator there sets `trust_loopback = false`; the UI then asks for the key. On first load without a stored key, you see a "PromptForge Gateway" sign-in card with a labeled API key password field and a Connect button. A wrong key shows "Invalid API key". An unreachable gateway shows "Gateway unreachable". The verified key is stored for the browser session. Any later 401 from the gateway clears the stored key and returns you to the key prompt.

## Get oriented

You navigate six top-level views from the tab bar: Settings, Discover, Local, Remote, Profiles, and Secrets. Every view has a bookmarkable hash URL, including a specific model's detail page and a specific Settings section. An unrecognized hash is rewritten to #/local.

A connection dot in the tab bar shows whether the gateway is reachable. The tab bar shows the running UI's version as a muted label, or vdev for a development build. Notifications appear as toasts that dismiss themselves after four seconds. Destructive actions require confirmation in a modal dialog that names the target; focus lands on Cancel as the safe default, and Escape or a backdrop click cancels. Every dropdown works entirely from the keyboard, including typing the first letters of an option to jump to it. The UI is always dark, the reduced-motion system preference disables essentially all animation, and byte sizes appear in human-readable units.

## The three states of an edit

Edits move through three states: unsaved edits held in the browser, saved pending shadows on the gateway, and the applied running configuration. When pending changes exist, the tab bar shows an Apply button labeled with the pending file count beside a Revert All button. When a previous session left unapplied changes, a banner offers Review, Apply, and Revert All.

Pressing Apply opens a progress overlay that follows the gateway's live progress stream stage by stage until the apply finishes or fails. The overlay carries a Cancel button; pressing it stops the apply on the gateway, and the overlay reports that the apply was cancelled and your pending changes are still staged. A failed stage holds on the error message for a moment before the overlay closes. When an applied configuration requires a restart, a banner reads "Restart the gateway to apply these changes." and clears itself once the gateway comes back on a new config generation.

Open the Review dialog to list every pending configuration change as a table of path, running value, and pending value. Secret values are never displayed.

## Profiles

The tab bar shows the active profile name. A menu lists every profile with the pending choice checked. Choosing another profile stages `active_profile` as a pending change that takes effect on Apply. A failed staging surfaces an error toast and leaves the current selection unchanged.

In the Profiles view you edit each profile as an ordered subset of the global model catalog through Available and Chosen shuttle listboxes. The listboxes support multi-select, roving focus, typeahead, selection counts, and per-pane search. The profile saves in global catalog order, not click order. A new profile starts Empty or as a Copy of an existing profile. You cannot delete the profile currently staged as active. The Set Active button stages the active profile; once staged it reads "Selected for Apply", and the switch lands on Apply.

The Profiles view shows an Estimated VRAM summary that sums declared model weights. Per-dominion budget rows warn at 80 percent and error when over. KV cache grows with context length, so 20 percent headroom is recommended.

## Discover

The Discover view searches Hugging Face. The search box accepts keywords, a `user/repo` form, or a pasted hub URL, and keystrokes collapse into one search after a 300 ms debounce. The GGUF filter chip is locked on because the gateway serves GGUF inference only. Chat is the default workload filter; the filters cover Chat, Embedding, Reranker, STT, Image, and TTS. Result rows show the publisher avatar, a parameter-count pill, compact download and like counts, and a relative updated time. Sorts are Most downloads, Trending, and Newest.

A model's GGUF files are grouped into named quantizations with exact summed byte sizes and the LFS SHA-256 for single-file quants, listed smallest first. Each quant shows a fit badge computed against the gateway's system snapshot: Fits GPU, Partial offload, CPU only, or Too large. One Recommended star marks the largest quant that fully fits free VRAM. A multi-part GGUF cannot be downloaded as one model; the button is disabled with an explaining tooltip. You can read model cards in the view, rendered as sanitized HTML so embedded scripts and event handlers cannot execute.

A Download click stages a pending model entry carrying the hub resolve URL, the LFS digest, and the listing size as `vram_gb`; Apply owns the actual transfer. Staging a discovered model also adds it to the active profile's checklist, so Apply provisions and serves it. The staged entry prefills a mapped built-in chat template when the server-side catalog matches the repo. An STT-filtered download stages a first-class `stt_model` entry with the interim role. Without a configured HF token you see a banner linking to the Secrets view instead of search results.

## Local and Remote

The Local and Remote views show the gateway's own catalog subsets: Local lists your local and speech-to-text entries, and Remote lists your remote entries. STT entries carry a Mic badge so you can pick them out at a glance. Filter chips narrow the list to All, Chat, or STT, a search box filters the rows after a short debounce, and a sort dropdown orders the list by Name, Size, or Kind.

Each model row shows a running-status dot, a kind badge, and capability pills. A quant badge read from the GGUF filename names the quantization, such as Q4_K_M. A model that exists only as a draft carries an "unsaved" badge until you save it.

## Secrets

The Secrets view manages the one global `.env` file. Variables appear as masked password rows with per-row reveal and delete. Save stages a pending shadow that takes effect only after Apply plus a gateway restart. New variable names must use letters, digits, and underscores, and must not start with a digit. Each variable shows "used by" annotations naming the configuration entries that reference it. A dedicated Hugging Face card configures `HF_TOKEN`, and its Test Connection probes the token the running gateway holds and reports Not set, Valid, Invalid, or Connection failed.

## Settings

The Settings view has seven sections: System, Gateway, Workshop, Dominions, Endpoints, Tools, and About. You land on System by default.

The System panel shows live metric tiles: CPU, RAM, VRAM with the GPU name, and disk usage with the cache path. The tiles refresh every 5 seconds, and a failed refresh keeps the last snapshot. Metric bars recolor by load: warning in the 70 to 89 percent band and danger at 90 percent or more.

The Gateway card edits the bind address, the API key, and the Trust loopback connections switch. The switch is on by default and admits callers on this machine that present no key; its help text states the cost, that on a shared machine any other OS account can then use the gateway, and turning it off requires the key from every caller. A note says the boot configuration cannot hot-reload, and changing the API key warns that the new key will be required after restart. The typed key leaves the DOM once saved. Stored secrets render as a masked readout with a Change button; leaving the input empty keeps the existing key, and an Eye toggle reveals and re-hides the secret.

The Dominions and Endpoints cards show used-by chips that count dependents, and a delete confirmation names them. A local-kind dominion reveals the `vram_gb` budget field, and switching the kind to remote hides it. An endpoint binds to a dominion from a dropdown offering only remote-kind dominions plus None. The endpoint protocol dropdown is locked to `openai`, and the endpoint API key stays redacted through saves until Change reveals the input.

The Tools section configures web search with the provider locked to Brave and the defaults documented on the card. The Storage card edits the cache directory beside live cache-drive usage, with a warning that changing the directory does not move existing files.

The About panel shows the medallion, the baked version or "dev", and the Boost Software License link. The Config UI card reports the UI as compiled in by the `config-ui` feature, served on the gateway's own port, loopback only, with the URL derived from the bind. The Workshop card edits the `[workshop]` section's one live content, the STT capture tuning - the gateway hosts no workshop listener, so the section's old `bind` and `open_browser` settings are inert and stay out of the editor. Adding the tuning seeds window_seconds 15, interval_ms 500, and an empty vocabulary.

## Editing a model

You edit a local model through sections for GPU, generation, source, and capabilities in the model detail view. An unconfigured optional section offers an Add button. The chat template control offers Auto, a built-in template family, or a custom .jinja path, with a read-only summary naming the effective source, the detected family, and the reason.

The model name edits inline in the detail header, and the header shows the model's status: Unsaved, Running, or Stopped. Each edited field carries a dirty dot and a per-field reset. Each saved-but-unapplied field carries a pending chip whose tooltip shows the running value. Deleting a model confirms a dialog naming the model and every affected profile, and the save removes every dangling profile reference in the same payload. A downloaded model shows its cached size and path with a Delete file action. A path source gets a reveal-in-folder button; URL sources get none. Capability pills show images and thinking mode, and the images pill is implied and locked when a multimodal projector is configured. The `gpu_layers` slider readout carries the GGUF layer total, and typing "Max" maps to the maximum.

The controls follow the shape of the value. Numeric settings pair a slider with a typed readout, typed values clamp to the allowed range, wide-range settings such as the context window use a logarithmic scale, and some sliders offer a rightmost "Max" detent. List-valued settings such as a model's endpoint list are edited as removable chips. Fields with a fixed choice set accept only the listed values. Boolean settings use an on/off switch. A setting can be disabled until a sibling field holds a required value, or hidden until a predicate passes, so you only see applicable controls.

Retyping a field's original value clears its unsaved edit, and you can reset one field or a whole entry. A new model entry starts as an unsaved draft, and name collisions get auto-suffixed. Every settings save carries the complete single-file configuration, so one section's save never erases another staged section.

An orphan section lists unconfigured files on disk with Adopt and Delete actions per file; Delete is disabled when the file has no verified digest. The UI shows whether a model's source file is already downloaded. On gateways built without local-model features, missing orphan and chat-template endpoints degrade to empty lists instead of breaking the UI. You can restore the recommended speech-to-text model pair, digest-pinned, over the existing STT catalog entries from the UI.

## Panel mode

The configuration UI runs in two modes. Standalone mode runs in a browser tab. Panel mode embeds the UI inside the Workshop with `?mode=panel`. In panel mode your API key never enters the frame; every gateway call rides a postMessage bridge to the Workshop, and the panel only talks to a loopback workshop origin. Bridged calls fail after a 30 second reply deadline rather than hanging. Apply and Revert actions are announced to the workshop's status bar, and the workshop pushes its theme and an initial route into the embedded panel once the bridge is up.

---

# Serving and Observing

This chapter teaches you the running gateway: the HTTP endpoints it serves, the tools it can host, and the health, logs, and observability surface you operate day to day. You already run a configured gateway with a profile and its models.

## Web search

Enable the built-in web-search tool with a `[tools.web_search]` section:

````
[tools.web_search]
provider = "brave"
api_key = "${BRAVE_API_KEY}"
default_count = 10
max_count = 20
max_per_host = 2
strip_tracking = true
````

The provider is locked to `brave`. The `base_url` defaults to the Brave Search endpoint and must be an HTTP(S) URL. The `default_count` must not exceed `max_count`. Freshness and safesearch defaults are closed vocabularies, not free text. The gateway calls the Brave Search API at `{base_url}/web/search` with the configured API key sent in the `X-Subscription-Token` header.

Callers run a web search through POST /v1/tools/web_search. The request body carries a `query` and optional `count`, `freshness`, `country`, `search_lang`, `safesearch`, `include_domains`, and `exclude_domains`. Unknown fields are rejected. The query is trimmed, rejected when empty, and capped at 512 characters. Caller knobs are validated before any provider call: freshness must be `pd`, `pw`, `pm`, `py`, or a date range; safesearch must be `off`, `moderate`, or `strict`; country is a 2-letter code; the search language is a 2 or 3 letter code; each domain entry must be a bare valid domain. The count defaults to `default_count` and clamps into 1 through `max_count`. The gateway over-fetches up to three times the requested count, capped at `max_count`, so post-processing filters still yield enough results. Omitted freshness and safesearch fall back to the configured defaults.

Results carry `title`, `url`, `site_name`, and `extra_snippets`. Result text is sanitized and capped, results are diversified by host at `max_per_host`, and a result whose URL is not navigable or is over 2048 characters is dropped. When `strip_tracking` is on, known tracking parameters such as `utm_*`, `fbclid`, `gclid`, `mc_cid`, and `mc_eid` are removed from result URLs. Include and exclude domain lists match the host itself or any subdomain.

When no `[tools.web_search]` section is configured, the route answers 404. The route exists only in builds compiled with the `web-search` feature. Search provider failures surface with a `web_search: ` prefix on the error, so you can distinguish search upstream errors from other gateway errors. The search service is built from the active profile's `[tools.web_search]` section and reloads on profile switch. The provider credential never appears in logs.

## The deprecated [workshop] section

The gateway never hosts the workshop: the desktop application embeds the workshop server itself, and the standalone `workshop-server` binary serves the UI for a browser. A boot config carried over from an older version may still declare a `[workshop]` section. The section keeps parsing - an existing config must not fail - and the gateway logs a deprecation warning at startup naming what changed: the section's `bind` and `open_browser` settings are inert, while the `[workshop.stt]` capture tuning still applies to the speech engine.

## Manage the cache

Manage the blob cache through the gateway's cache routes. GET /v1/cache lists entries with source URL, path, SHA-256, and size. Only blobs carrying a `.meta.json` sidecar appear in the listing, and listing reads the sidecar metadata only; it never re-hashes the blobs. POST /v1/cache downloads a blob with an optional pin and streams progress events ending in a ready event. DELETE removes one blob by digest. Cache downloads validate the source URL and the pin before any network access. A cache download lands in the same slot layout that local model provisioning uses, so a cache download is a provisioning cache hit for the same URL, and vice versa.

GET /admin/orphans lists cache files that no configured model references, so leftovers can be adopted or deleted. GET /admin/model-info reports a GGUF file's header summary (architecture, layer count, parameter count, and chat template) without loading the model; only files inside the artifact cache can be inspected, and escaping or missing paths are refused. POST /admin/reveal opens the host's file manager at a model or config file; reveal requests are confined three ways: loopback-only, bearer key required, and the path must canonicalize to strictly inside the artifact cache.

The gateway restricts the cache root to your own account at startup and refuses to run when it cannot, failing with a cache-not-private error.

## Status, progress, and metrics

GET /admin/status reports the active profile, the models it exposes, and a config generation that changes when the gateway restarts. GET /admin/profiles lists the profiles in the loaded catalog.

GET /admin/progress streams every long-running operation in the process as one server-sent event stream. A fresh subscriber first receives live operations replayed, then every event. Heartbeat comment lines arrive every 15 seconds while idle.

Download progress renders as tracing log lines on every stream.

GET /admin/system reports host metrics: CPU, RAM, the cache drive, and the first NVIDIA GPU's VRAM. The GPU field is absent, never an error, when no capable driver is present. You can also pull bounded captured stdout and stderr tails for each running local model as diagnostics.

GET /admin/chat-templates returns a bearer-authenticated catalog of chat template families, known model-to-family mappings, and each pending local model's effective template decision.

You can search Hugging Face and read model details and READMEs through the gateway's hub proxy. A missing or invalid `HF_TOKEN` surfaces as a distinct "set HF_TOKEN" error. Hub search queries are validated against a closed allowlist before any upstream call, and repository paths must be an exact owner/name pair of hub-legal segments.

## Errors and limits

Every request failure reaches the client in the OpenAI error envelope: an object with `message`, `type`, and `code` under `error`, with a stable HTTP status. Examples: 401 `unauthorized`, 404 `model_not_found`, 400 `malformed_request`, 400 `kind_mismatch`, 429 `queue_rejected`, 503 `queue_full`, 503 `profile_switch`, 503 `partial_start`, 422 `config_write_rejected`, and 422 `model_info_error`.

Outbound calls to any backend have fixed timeouts: 10 seconds to connect and 120 seconds for a whole non-streaming request. Streaming connections are bounded only by the connect timeout. Response bodies the gateway reads are capped: 64 KiB for error bodies and 4 MiB for success JSON bodies.

Malformed client requests are rejected at the boundary. An empty model name, an empty messages array, an unsupported message role, or a message with neither content nor a tool call all fail validation. Request fields the gateway does not name pass through to the backend verbatim, while the reserved keys `model`, `messages`, and `stream` may not be smuggled in twice. Embeddings requests accept one string or a batch of strings, with an optional `encoding_format` of `float` or `base64`; an empty batch is rejected. Rerank requests carry a query, a document set, and an optional `top_n` limit; an empty query or document set is rejected.

## Reading failures

The error code distinguishes a connection that never reached the provider from a mid-flight failure. The first is safe to retry; nothing was billed. The second is not safe to retry blindly. A backend's own client-error status, for example 429, passes through to the caller with code `upstream_client_error` instead of a generic 502. A model of the wrong kind is refused with 400 `kind_mismatch` before any upstream call. A request for a workload the resolved model cannot serve is rejected with 400 `model_unavailable`.

When the gateway recovers from a malformed tool fence in an emulated tool dialect, the response message carries a `gateway_warning` extension field. The turn never fails, and protocol junk never appears as final text. Streaming clients still receive tool calls from an emulated-dialect model: the gateway buffers one upstream round trip and re-emits the rewritten response as synthetic chunks, with a trailing summary chunk carrying usage and timings.

A malformed upstream stream chunk is logged and skipped without ending the stream. A mid-stream transport failure ends the stream with an error. An upstream error status fails a streaming request before any chunk is delivered, returned as a JSON 502, never as a stream that dies mid-flight. A client disconnect cancels the upstream request.
