# PromptForge User Guide

PromptForge turns Markdown files into executable AI prompt pipelines. This guide covers every component: the CLI that runs prompts, the gateway that talks to model backends, the core library that parses and executes prompt files, the MCP server, the tool picker, the web fetch tool, and the development runner.

---

## promptforge-cli User Guide

PromptForge CLI is a command-line tool that runs PromptForge prompt files against LLM providers. You point it at a prompt file, and it runs the prompt in a single process. There is no server to start, no connection to manage, and no configuration file to write. Your credentials stay in environment variables, never on the command line. Built-in web tools let your prompts fetch pages and search the web. If you can type one command in a shell, you can run any PromptForge prompt.

### What promptforge-cli Does

You run a PromptForge prompt file from the command line by pointing the tool at the file. The tool parses the prompt and executes its sections top to bottom in a single process. Running a prompt is a single command.

The tool runs only genuine PromptForge prompts. If a file's frontmatter does not declare a `promptforge:` version, the tool refuses to run it.

On success, the prompt's returned value is printed to stdout. Stdout contains exactly that returned value and nothing else. Errors go to stderr. This contract makes the tool safe to use in scripts and pipelines.

### Getting Started

Install the tool from crates.io. The install produces an executable named `promptforge`. The install requires Rust 1.89 or later.

Run a prompt file with the `run` subcommand:

````console
promptforge run prompts/hello.md
````

The tool reads the file from your local filesystem, executes it, and prints the result. You address prompts directly by file path. There is no configuration file, no name resolution rule, and no catalog lookup.

You can pass a raw input string to the prompt as an optional positional argument after the file path:

````console
promptforge run prompts/staker.md "Bloomberg"
````

Inside the prompt, the input is exposed as `args`. It defaults to empty. The tool does not inspect, split, or coerce the input. An input that contains spaces must be quoted as a single shell argument.

### Configuring the Gateway

You connect to a remote PromptForge gateway by setting two environment variables together:

````console
export PROMPTFORGE_GATEWAY_URL="https://gateway.example.com/v1"
export PROMPTFORGE_GATEWAY_API_KEY="your-bearer-token"
````

This enables the `web_search` tool and the remote model catalog for inference through the gateway.

Credentials are accepted only through environment variables, never through command-line flags. This keeps tokens out of argv, process listings, and shell history.

You run entirely local-only, with no network access, by leaving the gateway API key unset or blank. A gateway URL set without a key also yields local-only mode. Local-only mode yields an empty model catalog and a local tool set.

The error cases are strict. A key set without a URL is a startup error, not a fallback. A gateway endpoint that is not a valid URL fails at startup, before the run begins. Blank or whitespace-only credential values are treated as absent. Whitespace around otherwise valid values is tolerated.

The bearer token never appears in logs or diagnostic output. When the tool renders the gateway configuration for diagnostics, it shows the endpoint but replaces the token with a redaction marker.

### Built-in Tools

Any prompt can fetch a web page and return its main content as markdown using the built-in `web_fetch` tool. It runs locally. It is always available in every mode. It needs no credentials.

A prompt can search the web and receive a list of results with title, url, and description using the `web_search` tool. The tool offers `web_search` only when gateway credentials are configured.

If a prompt explicitly binds to a tool that is not available, the run fails before any section executes. For example, a prompt that explicitly binds to `web_search` without gateway credentials fails with an absent-capability error.

### Selecting Tools for a Run

The tool selects tools semantically for each prompt. During startup, it loads an embedding model and builds a semantic tool picker over the available tool catalog.

Capability binding is automatic. When `web_search` is unavailable, a prompt's search capability request falls back to `web_fetch`. When the gateway is configured, the search capability binds to `web_search`. You do not configure this mapping. The tool derives it from the available catalog.

### Startup Progress

While stderr is an interactive terminal, you see live progress bars during startup. The startup sequence has three labeled phases: "model catalog", "embedding model", and "tool index". Each bar shows its phase name and a numeric percentage. Finished phases disappear as their bars are cleared.

The bars are suppressed entirely when output is piped. This keeps stderr clean for scripts. If the progress display itself fails to start, the tool prints a warning and the run proceeds without bars. A progress failure never fails the run.

### Cancelling a Run and Exit Codes

You can interrupt a running prompt with Ctrl-C. This cooperatively cancels the run. If the Ctrl-C listener cannot be installed, the tool prints a stderr warning that the run is not cancellable, and the run proceeds.

Scripts can branch on four exit codes:

| Exit code | Meaning |
|---|---|
| 0 | Success |
| 1 | Operational failure (unreadable file, not a prompt, parse error, setup failure, execution failure) |
| 2 | Usage error (missing file argument, unknown subcommand) |
| 130 | Cancelled with Ctrl-C |

In a script, check `$?` to branch on success or failure. Remember the output contract: on success, stdout holds exactly the returned value. Errors go to stderr as an error chain.

### State and Observability

By default, each run uses a fresh in-memory store. A prompt's state lives exactly as long as the process. Nothing persists or accumulates across runs.

You can persist the prompt's store across runs with the `--store DIR` option:

````console
promptforge run prompts/staker.md "Bloomberg" --store ./state
````

This switches from the default ephemeral in-memory store to a persistent file-backed store in the directory you name.

Each run generates a unique execution ID, a 36-character string prefixed with `cli-`. Use it to correlate observations within a single invocation.

During the run itself, the tool produces no progress output. Long runs are silent until the result appears. Silence in between is expected.

### Errors and Invocation Edge Cases

Error messages name the failing stage. Examples include `"read prompt file <path>"`, `"parse prompt file <path>"`, `"fetch the model catalog"`, and `"load the tool embedding model"`. Each error goes to stderr as an error chain, so you see the full context of the failure.

The invocation parser is strict. A missing file argument is an error. An unknown subcommand is an error. Extra trailing arguments are an error. None of these are silently ignored.

The frontmatter gate applies to every run. A file whose frontmatter declares no `promptforge:` version is rejected before execution. This is how the tool guarantees it runs only genuine PromptForge prompts.

---

## Gateway Configuration

One file runs the whole gateway. `gateway.toml` holds your bind address, your credentials, your complete model catalog, and every deployment profile. Edit the file, and the gateway parses it, expands environment variables, and validates every rule before anything runs. A file that fails any check never starts a gateway. This guide teaches you the full schema, section by section, with working examples you can copy and edit. Operators normally use the Config UI to edit configuration. This guide is for advanced users who edit `gateway.toml` directly. The canonical commented example is `gateway.local.example.toml` at the workspace root. It is guaranteed to parse and validate against the current schema.

### One File, One Version

You pin the on-disk schema by writing `config-version = 2` at the top of the file. Version 2 is the profile-and-STT layout. Any other value, or a missing version, fails with migration guidance.

One file owns everything. Global settings live in `[server]`, `[workshop]`, `[local]`, and `[tools]`. Remote providers live in `[[endpoint]]`. Compute pools live in `[[dominion]]`. The model catalog lives in `[[model]]`, `[[local_model]]`, and `[[stt_model]]`. Deployment variants live in `[[profile]]`. Keep sections in this canonical order. The Config UI writes this order, so equivalent edits stay merge-friendly.

The active profile is state, not config. It lives in a sibling file named `<config-stem>.state.toml` with a single key, `active_profile = "name"`. You switch profiles without editing `gateway.toml`, and the gateway remembers the selection across restarts.

### The Core Sections

`[server]` sets the socket address the gateway binds to and the shared bearer key every `/v1/*` request must present. `[workshop]` runs an embedded workshop UI on its own bind address. `[local]` chooses the cache directory for downloaded models and the pinned llama.cpp install. `[tools]` holds optional built-in tools such as web search.

The smallest valid config needs only the version, the bind address, and the API key:

````toml
config-version = 2

[server]
bind = "127.0.0.1:8080"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
````

Everything else has defaults or is optional. The API key must not be empty. The gateway never starts without authentication.

### Validation at Load

Validation is upfront and total. Load reads the file, expands variables, and checks every rule before any profile runs. Every profile is validated, not only the active one. This includes VRAM budgets and STT role pairing. A broken inactive profile cannot slip through.

Misspelled or unknown keys are rejected everywhere, so typos fail fast at load. Removed v1 constructs are hard-break errors that name the file, the key, and the exact line. A configuration that fails any check never produces a running gateway.

### Server, Secrets, and Interpolation

You set the bind address and API key in `[server]`:

````toml
[server]
bind = "127.0.0.1:8080"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
````

You reference environment variables in any string value with `${VAR}` syntax, including strings nested inside arrays and tables. Keep secrets and host-specific settings out of the file this way. Write `$$` for a literal dollar sign. An unset variable is a startup error that names the variable. An unclosed `${...` is a distinct malformed-interpolation error. Text in comments and keys is never expanded.

Secrets are safe by construction. API keys render as `redacted` in logs and debug output. They serialize as `"***"` in exported JSON. Error messages may name fields, model names, and sources, but never render a secret. When you edit a config that shows `"***"`, leave the marker in place. The real secret carries over on save. Secrets match entries by name or id, so reordering models or endpoints does not lose them.

If you bind `0.0.0.0` or `::`, same-host clients such as the workshop use the matching loopback URL automatically. You configure no separate client address.

### Endpoints and Remote Models

You declare each remote provider as an `[[endpoint]]` with an id you choose, the wire protocol, the base URL, and the API key:

````toml
[[endpoint]]
id = "openai"
protocol = "openai"
base_url = "https://api.openai.com/v1"
api_key = "${OPENAI_API_KEY}"
````

`protocol = "openai"` is the only protocol. The gateway targets OpenAI-compatible backends. Base URLs must be absolute `http` or `https` URLs with a real host.

You publish caller-facing model names with `[[model]]` entries. Each entry carries a description, a context window, the upstream model string the backend knows, and the endpoint ids that serve it:

````toml
[[model]]
name = "gpt-5"
description = "General-purpose chat model"
context = 200000
upstream = "gpt-5"
endpoints = ["openai"]
````

This decouples what clients request from what the provider calls the model. The `endpoints` list must name defined endpoint ids, must not be empty, and must not repeat an endpoint. Every model needs a description and a nonzero context size.

You classify each model by workload kind: `chat`, `embedding`, or `classifier`. Kind defaults to `chat`. You declare thinking tokens as `never`, `always`, or `switchable` (caller-toggleable per request). The default is `never`. You pick the tool-calling dialect per model: `openai` (the default) for native wire tool calls, or `gemma3_tool_code`, an emulated content-fence mode for models like Gemma 3 that lack native tool arrays.

A full chat model adds capability metadata and per-model defaults:

````toml
[[model]]
name = "gpt-5-reasoning"
description = "Reasoning chat model with effort levels"
kind = "chat"
context = 200000
upstream = "gpt-5"
endpoints = ["openai"]
thinking = "switchable"
tool_dialect = "openai"
default_max_tokens = 8192
max_output = 32768
default_temperature = 0.7
images = true
parallel_tool_calls = true
effort_levels = ["low", "medium", "high"]
default_effort = "medium"
adaptive_thinking = false
````

Capabilities surface verbatim on `GET /v1/models`. All capability fields are optional and default to absent. `default_max_tokens` applies when the caller omits a token limit. `max_output` may not exceed the context window. Effort knobs require thinking: you set `effort_levels` and `default_effort` only on models whose thinking is `always` or `switchable`, and `default_effort` must name a listed level. Chat-only fields are rejected on embedding and classifier models. These are `thinking`, effort knobs, `adaptive_thinking`, `tool_dialect`, and `default_max_tokens`.

### Local Models and Companions

You serve local GGUF models through gateway-managed `llama-server` child processes. Declare a `[[local_model]]` with a name, description, source, and context size:

````toml
[[local_model]]
name = "qwen-local"
description = "Local chat model"
source = "https://huggingface.co/unsloth/Qwen3.5-9B-GGUF/resolve/main/Qwen3.5-9B-Q4_K_M.gguf"
sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
context = 65536
````

The source is an https download URL or a local filesystem path. A remote source must carry a `sha256` pin, verified after download. Local paths are operator-controlled and may be unpinned. Plaintext `http` sources are rejected. Any pin present must be exactly 64 lowercase hex characters.

You tune llama.cpp inference per model:

````toml
[[local_model]]
name = "qwen-local"
description = "Local chat model"
source = "/models/qwen.gguf"
context = 65536
gpu_layers = 99
flash_attention = true
cache_type_k = "q8_0"
cache_type_v = "q4_0"
parallel = 1
n_predict = 8192
chat_template_file = "templates/qwen.jinja"
````

Defaults: `gpu_layers` 99, `flash_attention` on, `cache_type_k` `q8_0`, `cache_type_v` `q4_0`, `n_predict` 8192, `parallel` 1. The `parallel` value sets the child process parallelism. It also caps the gateway queue for the model when the model has no compute pool binding (pools are covered in Dominions and VRAM Budgeting). Use `chat_template_file` when the GGUF embeds a template without tool-calling support.

You attach a speculative-decoding drafter to accelerate generation:

````toml
[local_model.speculative]
type = "draft-mtp"
source = "/models/qwen-mtp.gguf"
draft_max = 3
````

`draft-mtp` is the only supported speculation type. `draft_max` is bounded from 1 to 16. You attach a multimodal projector to give a local chat model image input:

````toml
[local_model.multimodal_projector]
source = "/models/qwen-mmproj.gguf"
````

Once attached, the model automatically advertises image capability in the catalog. Both companions follow the same source rules: https URL or local path, with a `sha256` pin required for remote sources. Companions attach only to chat-kind local models.

You choose the cache location with `[local] cache_dir`. Omit it for the default `~/.promptforge` (`%USERPROFILE%\.promptforge` on Windows). Models land in `<cache_dir>/models`. The pinned llama.cpp install lands in `<cache_dir>/llama.cpp`.

### Speech-to-Text Models

You declare speech-to-text models in the same catalog as chat models:

````toml
[[stt_model]]
name = "whisper-base-en"
role = "interim"
source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin"
sha256 = "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002"
vram_gb = 1.0
````

This entry is the built-in recommended interim model, shown verbatim.

Each STT model fills one of two pipeline slots. `interim` is a low-latency model used while a take is still recording. `final` is a higher-accuracy model that crystallizes completed audio. Each profile selects at most one interim and one final STT model. Interim-only is allowed as a degraded mode. Final-only is rejected because streaming requires an interim model, and the error names the fix.

A recommended digest-pinned pair ships built in: `whisper-base-en` for interim and `whisper-small-en` for final. The pair carries canonical whisper.cpp download URLs, SHA-256 pins, and conservative VRAM estimates of 1.0 and 2.0 GiB with headroom above resident footprints, so speech-to-text can run CPU-friendly. The Config UI offers a restore action that re-seeds this curated pair instead of hand-writing entries.

Remote STT sources may omit the pin. This asymmetry with local models and companions is intentional. `[workshop.stt]` holds capture tuning only. Model selection lives in the catalog and profiles, not in the workshop section.

### Dominions and VRAM Budgeting

You declare named compute pools as `[[dominion]]` entries. A dominion is either `local` (a GPU pool) or `remote` (a provider pool):

````toml
[[dominion]]
id = "gpu0"
kind = "local"
max_concurrency = 4
max_queue = 100
policy = "queue"
fair_scheduling = true
vram_gb = 24

[[dominion]]
id = "remote-pool"
kind = "remote"
max_concurrency = 10
````

You bind remote endpoints to remote dominions and local or STT models to local dominions by id. Add `dominion = "gpu0"` to a `[[local_model]]` or `[[stt_model]]` entry, or `dominion = "remote-pool"` to an `[[endpoint]]` entry. An endpoint or model with no binding gets unlimited pass-through. A binding to an undefined dominion, or to a dominion of the wrong kind, is rejected.

Per dominion you set `max_concurrency` (or leave it unlimited), `max_queue` (default 100), a full-queue `policy` of `queue` (wait) or `reject` (fail fast), and `fair_scheduling` (round-robin by client key, on by default). Limits must be at least 1 when set.

A local dominion can carry a `vram_gb` budget. The gateway then checks every profile's selected local and STT models against that budget and rejects any profile that would over-book the GPU. The error names the profile, the dominion, and the exact overshoot. A model bound to a budgeted dominion must declare its own `vram_gb` estimate. An exact fit is accepted. Models bound to an unbudgeted dominion need no estimate. Different profiles may collectively over-book a GPU as long as each profile's own selection fits. A remote dominion that declares `vram_gb` is rejected.

### Profiles and Startup Selection

You define named profiles so one `gateway.toml` holds multiple deployment variants. A profile is a pure checklist: a name and a `models` list selecting entries from the global catalog:

````toml
[[profile]]
name = "work"
models = ["gpt-5", "qwen-local", "whisper-base-en", "whisper-small-en"]

[[profile]]
name = "travel"
models = ["gpt-5"]
````

A profile owns no settings. Profiles can mix remote, local, and STT models. An empty checklist is allowed. Every name must resolve across the combined catalog, and duplicates within one profile are rejected. The global catalog holds every defined model. The active profile filters it down to what a deployment actually exposes.

Profile names must be a single safe path component: no path separators, no `.` or `..`, no surrounding whitespace, no NUL bytes. Each violation is rejected with a specific reason.

Startup selection follows a fixed precedence. The `--profile` command-line flag wins, then the `PROMPTFORGE_PROFILE` environment variable, then the persisted state file. If no profile is selected anywhere, startup fails with an error listing the defined profiles and the three ways to select one. If the persisted or requested profile does not exist, the error names the stale value and lists the defined profiles.

### Loading, JSON, and Pending Edits

Loading reads the single file, expands variables, validates everything, and activates the selected profile in one step. Tooling can validate a TOML document in memory without touching disk. Tooling can switch the active profile on an already-loaded configuration without re-reading anything.

You export the resolved configuration as JSON for inspection or piping into other tooling. Every secret field redacts to `"***"`. The JSON uses the same key names as the TOML.

Admin edits are staged as pending shadow files: `gateway.toml.next` and `gateway.state.toml.next`. No save touches the live config until the staged edit is explicitly promoted. Promotion is atomic, with automatic backup-and-restore if the replacement fails midway. A staged edit is fully validated against the real schema and profile definitions before it reaches disk. A bad edit leaves no shadow behind.

You preview a pending edit exactly as it would run. Command-line and environment profile overrides still win. A pending-changes report lists which files have staged edits and which top-level sections will change, including a pending profile switch. A single admin document can carry both the global config and an `active_profile` choice. The profile choice is split out into the pending state file automatically, preserving the config/state boundary. A profile still recorded as active cannot be deleted in a pending edit.

### Built-In Tools: Web Search and Workshop

You give models a built-in web-search tool with an optional `[tools.web_search]` section backed by the Brave Search API:

````toml
[tools.web_search]
provider = "brave"
api_key = "${BRAVE_SEARCH_API_KEY}"
default_count = 10
max_count = 20
max_per_host = 2
default_freshness = "pw"
default_safesearch = "moderate"
strip_tracking = true
````

`provider = "brave"` is the only provider. The base URL defaults to `https://api.search.brave.com/res/v1`, and you can override it with a custom absolute URL. `default_count` and `max_count` must each be at least 1, and the default may not exceed the maximum. `max_per_host` caps results per hostname group (default 2). `default_freshness` accepts only `pd`, `pw`, `pm`, `py`, an explicit `YYYY-MM-DDtoYYYY-MM-DD` range, or empty. `default_safesearch` accepts only `off`, `moderate`, `strict`, or empty. `strip_tracking` scrubs known tracking parameters from result URLs and is on by default.

You run the embedded workshop UI with an optional `[workshop]` section:

````toml
[workshop]
bind = "127.0.0.1:7910"
open_browser = false

[workshop.stt]
window_seconds = 15
interval_ms = 500
vocabulary = ["PromptForge", "WG21", "GGUF"]
````

The workshop binds `127.0.0.1:7910` by default. Set `open_browser = true` to open the system browser once the UI is serving. The workshop derives its gateway connection from `[server]`: same address, same API key. No credential is duplicated and none can drift.

`[workshop.stt]` tunes live capture. `window_seconds` sets the seconds of trailing audio per interim pass (default 15). `interval_ms` sets the milliseconds between passes (default 500). `vocabulary` lists domain terms the transcriber is biased toward; an empty list disables biasing. `[workshop.tape]` is accepted for compatibility and ignored: agent sessions persist their event logs under the workshop's state directory instead.

### Errors and Migration

Every load failure classifies into one of seven stable categories: the file could not be read, the TOML did not parse, an interpolation was malformed, an environment variable was unset, a semantic check failed, a removed layout feature was found, or a shadow file could not be written. Tooling reacts to the category without parsing error text.

Removed v1 constructs are hard-break errors, not silent ignores. Each diagnostic names the file, the removed key, the exact line, and the replacement layout:

- `include` key: use one `gateway.toml` with `[[profile]]` checklist entries.
- A sibling `profiles/` directory: move every profile into this file as a `[[profile]]` checklist.
- A top-level `models` allowlist: move the checklist into a `[[profile]]` `models` key.
- `[workshop.voice]` model keys: use `[workshop.stt]` tuning and a global `[[stt_model]]` entry.
- A missing or wrong `config-version`: set `config-version = 2`.

Legacy `[queue]` and `[[device]]` sections, and per-endpoint or per-model device keys, fail as parse errors. Queue and device settings moved into `[[dominion]]`.

---

## Running Local Models With The PromptForge Gateway

The PromptForge gateway can run LLM inference entirely on your own machine. You declare a model once. The gateway downloads the weights, verifies them against your SHA-256 pin, installs a prebuilt llama-server build matched to your platform, and picks the correct chat template automatically. You install nothing and compile nothing. This guide shows you how to add models, control their chat templates, run and supervise them, and manage the cache.

### Local models at a glance

A local model is an entry in the gateway configuration. You declare each one in its own `[[local_model]]` table. The entry names a source: an HTTP(S) URL to a GGUF file, or a path to a GGUF file already on your disk.

The gateway runs each local model as a managed llama-server child process. The gateway selects the correct prebuilt binary for your operating system and CPU architecture. Windows and Linux use Vulkan builds. macOS uses Metal builds on both Intel and Apple Silicon. Windows ARM64 uses a CPU build, because the pinned release has no Vulkan build for that platform. Windows x86-64 CUDA builds stage an embedded CUDA bundle instead of downloading a server archive.

Local models appear in the gateway routing table exactly like remote models. You call them through the same OpenAI-compatible endpoints. Chat completions, streaming, embeddings, and reranking work the same way for local and remote models.

The Config UI is your operator surface. It shows pending downloads, chat template decisions, and model metadata. There is no separate command-line tool for local models.

### Adding and downloading models

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

### Chat templates

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

### Running and supervising models

Start every configured local model with one action. The gateway provisions the pinned server binary, downloads and verifies each model, and spawns the child processes. If no `[[local_model]]` entry exists, startup does nothing and downloads nothing.

A server is ready only after it passes an authenticated identity check on its health endpoint, not just an open port. Each launch attempt gets a fresh random model alias and bearer API key, so the local server answers only this gateway instance. Startup allows up to 4 fresh-port attempts on bind collisions and a 180-second readiness deadline. You can interrupt a slow startup with Ctrl-C.

Best-effort startup keeps one failing model from blocking the rest. You can then inspect exactly which models failed and why.

If a local server crashes, the gateway respawns it automatically and retries the failed request. The caller does not see the outage. A respawn reuses the same port, alias, and API key, so routing stays valid. A 3-second cooldown between respawns prevents a dead model from restart-looping.

Shut down all local children explicitly to free VRAM before you switch profiles. Tearing down the runtime kills every managed child automatically.

You can read bounded captured-output tails of each running child, keyed by model name. Server logs are captured at forced verbosity, so GPU device reports and tensor-offload evidence appear in the diagnostics. API keys are redacted from logs and diagnostics.

#### Model kinds and tuning

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

### Managing the cache

All downloaded artifacts live under the cache root. Cache slots are keyed by a hash of the full source URL, so two URLs that share a filename never collide.

You can check whether a URL is already cached, optionally against a digest pin, without triggering a download. You can list every cached blob with its source URL, SHA-256, and size. Listing reads only metadata sidecars, so it stays cheap even for multi-gigabyte entries.

Delete a cached blob by its SHA-256 digest. The blob, its metadata sidecar, and its verify marker are removed in one operation. Run the orphan scan to find files on disk that no configured local model references, so you can reclaim disk space.

Cached artifacts are served to clients through the gateway `/v1/cache` routes.

Each GGUF downloaded from Hugging Face gets a metadata sidecar beside it. Open it in any text editor to see the model's source URL, fetch timestamp, chat template, and model card excerpt.

### Limits and safety rules

The managed server is a single pinned llama.cpp release, b10082. Every platform asset carries a hardcoded SHA-256. The bundled chat templates are validated against b10082 only. A platform with no managed build produces an explicit error naming your OS and CPU architecture.

Every cache path is confined to the cache root. Paths with `..` or absolute prefixes are rejected. Every path component is checked for symlinks and reparse points. Deletion refuses symlink or reparse-point targets. Archives are extracted defensively: zip-slip entries, absolute paths, symlinks, and device nodes are rejected.

GGUF header parsing is bounded. Metadata entries and tensors are capped at 65,536. Strings are capped at 4,096 bytes. Embedded chat templates are capped at 1 MiB. Big-endian GGUF files are not supported. A malformed file fails with a typed error that names the file and the reason.

CUDA on Windows requires an installed CUDA Toolkit. The versioned variable, for example `CUDA_PATH_V13_3`, takes precedence over the generic `CUDA_PATH`. A missing toolkit produces an error that names the exact version and the variables to set.

Every failure produces a specific, actionable message. A digest mismatch shows the expected and actual hashes. A cache-privacy failure names the root and tells you to restrict it and re-run. A readiness timeout shows the deadline. Errors are classified as retryable or permanent, so transient faults are retried and permanent faults are reported as-is.

---

## Speech-to-Text User Guide

The PromptForge gateway has a built-in speech-to-text runtime. It gives you two things at once: a live voice channel that streams interim transcripts while you speak, and an OpenAI-compatible file transcription endpoint that your existing client code can call without changes. Models are pinned by digest, provisioned automatically, and loaded only for the profile you select. This guide shows you how to configure the runtime, transcribe audio files, and run live voice sessions. When you finish, you will have a transcription service you can configure, call, and observe.

### What This Is

The gateway owns and operates the speech-to-text runtime. You do not run a separate service. You select an STT profile in the gateway configuration, and the gateway provisions, verifies, and loads the models for that profile.

You interact with the runtime in two ways. You stream microphone audio over a WebSocket for live results. Or you upload a WAV file over HTTP and receive the transcript in the response.

### Endpoints and Transports

The gateway exposes three endpoints:

- `GET /voice` - a WebSocket endpoint for live, streaming speech-to-text. Workshop clients use this persistent socket for voice interaction. Any WebSocket client can use it.
- `POST /v1/audio/transcriptions` - an OpenAI-compatible multipart endpoint for file transcription. Existing OpenAI client tooling works against it without modification.
- `GET /voice/capability` - a JSON probe that reports whether GPU-accelerated transcription is available.

Use `/voice` when you speak to the gateway in real time. Use `/v1/audio/transcriptions` when you have an audio file.

### Configuration and Runtime Lifecycle

You configure the runtime through the shared gateway configuration. There is no separate STT config file. You declare model catalog entries with `[[stt_model]]` tables, and you attach models to a profile with `[[profile]]`.

A minimal configuration declares one interim model and selects it in a profile:

````toml
[[stt_model]]
name = "speech"
role = "interim"
source = "model.bin"
vram_gb = 1.0

[[profile]]
name = "voice"
models = ["speech"]
````

The `role` field is `interim` or `final`. An interim model produces live partial results during streaming. A final model is optional and produces higher-quality committed text. With only an interim model, the streaming endpoint keeps working with a degraded stop fallback.

When the runtime starts, the gateway provisions only the models the selected profile declares. Unused models are not downloaded. Downloaded artifacts are verified before use. Switching profiles loads the new engine on demand and unloads the previous engine automatically.

A fuller configuration adds a final model, pins an artifact by digest, and tunes capture behavior:

````toml
[[stt_model]]
name = "speech"
role = "interim"
source = "model.bin"
vram_gb = 1.0

[[stt_model]]
name = "speech-final"
role = "final"
source = "model-final.bin"
sha256 = "<64-hex-digit digest>"
vram_gb = 2.0

[workshop.stt]
window_seconds = 8
interval_ms = 400

[[profile]]
name = "voice"
models = ["speech", "speech-final"]
````

- Set `sha256` on a model to pin it to an exact digest. The gateway rejects a tampered or wrong artifact at provisioning time.
- Omit `sha256` and point `source` at a local file such as `model.bin` to use an unpinned model directly.
- Tune capture through `[workshop.stt]`: `vocabulary` biases recognition toward your terms, `window_seconds` sets the analysis window, and `interval_ms` sets the pass interval.

Three profile shapes are valid:

- Interim plus final: full quality pipeline.
- Interim only: one model loads; streaming still works.
- No STT models: the gateway starts cleanly with no STT.

Two configurations are rejected. A profile with a final model but no interim model fails validation with an error that names the profile and the fix: add one interim STT model or remove the final model. A headless gateway refuses an active profile that selects STT models.

### GPU Acceleration

GPU-accelerated transcription through CUDA is a build-time feature. The `promptforge-workshop` desktop build enables it by default. A gateway build opts in with the `workshop-cuda` feature, which turns on the `promptforge-stt` crate's `cuda` feature.

Before you start a voice session, query the capability endpoint to check GPU availability:

````bash
curl "$GATEWAY/voice/capability"
````

The response reports GPU availability and whether an STT engine is provisioned and loaded in the active profile:

````json
{"gpu": true, "engine": true}
````

### File Transcription API

You transcribe a file with one multipart POST. Authenticate every request with the gateway bearer token. The simplest request uploads a WAV file and names a loaded model:

````bash
curl -X POST "$GATEWAY/v1/audio/transcriptions" \
  -H "Authorization: Bearer $TOKEN" \
  -F file=@meeting.wav \
  -F model=speech
````

The default response is compact JSON containing only the transcript:

````json
{"text": "hello"}
````

The `model` field selects which loaded model handles the request: the interim-role model or the final-role model. If the name matches no loaded model, the gateway returns HTTP 404 with a message naming the unknown model. A malformed request returns HTTP 400.

Uploaded audio must be 16 kHz mono WAV, at most 25 MiB. Integer WAV of any bit depth and 32-bit float WAV are both accepted; integer PCM is normalized to floating point automatically. Oversized files are rejected before any decoding work happens.

A maximal request chooses the verbose response shape, hints the language, and requests timestamp granularity:

````bash
curl -X POST "$GATEWAY/v1/audio/transcriptions" \
  -H "Authorization: Bearer $TOKEN" \
  -F file=@meeting.wav \
  -F model=speech-final \
  -F response_format=verbose_json \
  -F language=en \
  -F "timestamp_granularities[]=segment" \
  -F prompt="quarterly planning meeting" \
  -F temperature=0.0
````

- `response_format` is `json` (default) or `verbose_json`.
- `language` is a hint. It defaults to `en` and is echoed back in the verbose response.
- `timestamp_granularities[]` accepts `segment` (the default) and `word`. Word-level timestamps can be requested, but the `words` array is currently empty because the engine has no word alignment.
- `prompt` and `temperature` are accepted without errors for OpenAI compatibility, but the current transcription workers ignore them. `temperature` must be a finite number greater than or equal to 0.0.

The verbose response adds duration, language, task name, and segment timestamps:

````json
{
  "task": "transcribe",
  "language": "en",
  "duration": 12.0,
  "text": "hello world",
  "segments": [
    {"id": 0, "start": 0.0, "end": 12.0, "text": "hello world"}
  ],
  "words": []
}
````

Errors are distinguishable by cause. Each error message names the cause. Two examples:

````text
audio file exceeds the 25 MiB limit
````

````text
audio must be 16 kHz mono, got 44100 Hz and 2 channels
````

Missing fields, invalid field values, unsupported response formats, bad WAV data, and inference failures each produce a distinct, identifiable error.

### Voice Session Basics

A live voice session runs over the `/voice` WebSocket. You open one connection, then run one or more push-to-talk takes on it.

The flow for one take:

1. Open a WebSocket connection to `/voice`.
2. Send the text message `start` to begin a take.
3. Receive a `stream` announcement frame. It carries a generation number that identifies the take.
4. Send audio as binary WebSocket frames: 16 kHz mono little-endian 32-bit float PCM, 4 bytes per sample.
5. Receive `interim` frames while you speak. Each frame splits the transcript into a stable `committed` prefix and a still-changing `tentative` suffix.
6. Send the text message `stop` to end the take.
7. Receive a `final` frame with the complete transcript.

Control messages are bare words, not JSON. Committed text is append-only: once words appear in `committed` they are never revised, so you can render them permanently.

You can run multiple takes on one connection without reconnecting. State resets between takes. Send `start` again mid-connection to restart with an incremented generation. Generation counters are per-connection: a new connection starts numbering at 1.

### Voice Wire Protocol

The wire contract, by example. You send bare control words and binary audio:

````text
start
<binary PCM frames>
stop
````

The server answers with JSON frames. The `stream` announcement arrives immediately after `start`, before any other frame:

````json
{"type": "stream", "generation": 1}
````

While audio streams, `interim` frames carry the live partial result:

````json
{"type": "interim", "committed": "we hold these truths", "tentative": "to be self", "generation": 1}
````

On `stop`, the `final` frame carries the full transcript, the count of complete audio samples received, and the take generation:

````json
{"type": "final", "text": "we hold these truths to be self evident", "frames": 192, "generation": 1}
````

Rules to rely on:

- Every frame carries the same `generation` counter. Correlate any frame with its take and discard stale frames.
- A partial trailing sample in a binary frame is dropped. Only complete 4-byte samples are counted in `frames`.
- Unknown text messages are ignored. They do not break the take or disturb transcription.
- Standard WebSocket ping/pong keepalive is handled transparently.

### Transcription Quality Pipeline

During a take, the runtime refines the transcript in the background. You observe this through the frames.

When a final-role model is configured, `committed` text grows segment by segment as the final-pass model finishes closed speech segments. If the final pass fails, or no final model is configured, the runtime falls back to the interim model automatically. The interim-only fallback decodes the entire take on stop, so no speech is lost.

The final transcript is the committed prefix plus a freshly transcribed tail, joined by a single space. If you stop exactly at a segment boundary, the final transcript is just the committed prefix, with no redundant tail transcription.

Three suppression behaviors keep the stream clean:

- Silent or too-short audio windows are never sent to the transcription engine. Silent audio produces no interim frames and an empty final transcript.
- Interim frames are sent only when the transcript changed. Duplicates are suppressed.
- A slow-reading client always sees the freshest interim result. Stale interims are dropped in favor of the newest.

If the engine is swapped mid-stream, for example by a profile switch, the current take resets cleanly instead of corrupting the transcript.

### Session Observability and Security

During a take, the gateway pushes live status updates to the workshop activity feed: "Listening...", "Transcribing...", and "Finalizing transcript...". Transcription failures produce visible failure notifications ("Transcription failed") in the feed rather than silent drops.

The `/voice` endpoint accepts connections only from native clients (no `Origin` header) or allowed loopback origins. Cross-site requests are rejected. A foreign origin is refused with HTTP 403 at upgrade time, before any session starts.

---

## PromptForge Gateway User Guide

PromptForge Gateway is a local model-serving gateway. It serves your prompts and models through an OpenAI-compatible HTTP API, so your existing OpenAI clients and tools work against it without modification. You describe every model once in a single `gateway.toml` file. You group models into named profiles. You then switch the served model set mid-run without downtime, and you keep every vendor key in one file on one host. This guide teaches you to start the gateway, configure models and profiles, run local models on your own GPU, and operate the admin surface with confidence.

### What the Gateway Is

The gateway is one binary, `promptforge-gateway`. It routes OpenAI-shaped chat completion requests to a backend. You run it in the foreground and it serves until you stop it.

Every client addresses a model by the public `name` you gave it in `gateway.toml`. The gateway resolves that name to a configured endpoint. It rewrites the name to the endpoint's upstream model alias before it calls the provider. Your clients never learn the upstream alias.

The gateway is the only process that holds a vendor key. Clients authenticate to the gateway with a single shared bearer key. The gateway authenticates to the provider with the vendor key. The caller's bearer key never leaks upstream.

The gateway can also run local models with no external server. It downloads model files, verifies their checksums, and manages the `llama-server` child processes for you. Local and remote models merge into one catalog. Clients address both by name in the same way.

Named profiles select which models the gateway serves. You switch the active profile mid-run. In-flight requests drain before the switch completes.

### Running the Gateway

Start the gateway with one command:

````bash
promptforge-gateway serve gateway.toml --profile main
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

Run `promptforge-gateway -h` to print usage and exit. Unknown flags, unknown subcommands, and a missing `serve` subcommand are usage errors printed to stderr with the full usage text.

### Inference Endpoints

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

### Model Routing and Concurrency

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

### Profiles and Profile Switching

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

### Local Models and GPU Inference

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

### Artifact Cache and Files

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

### Web Search and Speech

The gateway includes a built-in, Brave-powered web search endpoint. It is compiled in by default and active when `[tools.web_search]` is configured. POST a query and receive results:

````bash
curl -X POST http://127.0.0.1:8080/v1/tools/web_search \
  -H "Authorization: Bearer my-secret-key" \
  -H "Content-Type: application/json" \
  -d '{"query": "promptforge gateway"}'
````

Each result carries a title, URL, site name, and extra snippets. Results are capped at 2 per host by default so no single site dominates. An empty or whitespace-only query is a 400. Calling the endpoint when web search is not configured is a 404.

With the `workshop` feature, the gateway accepts OpenAI-compatible multipart audio transcription at `POST /v1/audio/transcriptions`. Audio uploads are capped at 25 MiB; larger bodies get a 413 `file_too_large`.

### Tool-Call Dialects

Models that declare no dialect default to standard OpenAI tool calling. For models without native tool support, such as Gemma 3, set one key on the model entry:

````toml
tool_dialect = "gemma3_tool_code"
````

You then send standard OpenAI `tools` and `tool_choice` fields. The gateway converts your tool definitions into a plain-language system guide, strips the unsupported fields before the upstream call, and converts the model's `tool_code` fence replies back into standard OpenAI `tool_calls` objects with `finish_reason: "tool_calls"`. Each call gets a unique synthetic id.

The model writes Python-style calls, `name(key=<json>)`, one per line, inside a `tool_code` fence. Full JSON values round-trip as arguments. A recognized-but-malformed fence never corrupts the turn: the reply content is emptied, a `gateway_warning` field explains why, and the recovery is logged. Ordinary prose passes through untouched.

### Administration and Configuration

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

### Workshop UI

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

### Progress and Observability

During long-running operations, such as startup provisioning, model downloads, and applies, the gateway draws live progress bars in the terminal. When stderr is not a terminal, progress appears as plain log lines: `started` on first sight, a percentage line at each 5-percent advance, and `done` at completion.

Subscribe to the same events over HTTP:

````bash
curl http://127.0.0.1:8080/admin/progress \
  -H "Authorization: Bearer my-secret-key"
````

`GET /admin/progress` streams every progress event as a bearer-authenticated server-sent event stream. A fresh subscriber first receives the currently live operations replayed. Heartbeat lines every 15 seconds keep the connection alive through NAT and firewall timeouts. The apply route reports its reload stages through this stream while its own response stays plain JSON.

For routine state checks, poll `GET /admin/status`. It reports the active profile, loaded models, the model allowlist, local child count, and a queue note, along with a process-lifetime `config_generation` identifier the config UI uses to detect a restart.

---

## Writing Prompts in PromptForge

A PromptForge prompt is one markdown file that runs as a program. You write YAML frontmatter, a title, and sections that mix ordinary prose with Lua code. PromptForge parses the file, validates it, and executes it. Your prose goes to a model as instructions. Your Lua steers the run. Tools, fanout, and a virtual file store give one file the reach of a script, a prompt template, and an orchestration loop combined. After one read of this guide, you can write a prompt that calls models, calls tools, runs work in parallel, and saves its results.

### The Prompt Document

You write a prompt as a single markdown file. The file has three parts: YAML frontmatter at the top, one H1 title, and H2 sections.

Here is the smallest complete prompt:

````markdown
---
name: greeter
description: says hi
promptforge: 1
---

# Greeter

## Say hi

Say hello.
````

Each part does one job:

- The frontmatter sits between `---` markers and carries the prompt's metadata. The `promptforge: 1` key declares the file as a PromptForge prompt. The runtime validates this version before anything executes. A file without the key is declined. A file with an unsupported major version, such as `promptforge: 2`, is refused.
- The H1 heading is the prompt's title. It is required. A file without an H1 is invalid.
- The H2 headings divide the body into named sections.
- Prose under a section is an instruction. The runtime sends it to a model. The model's answer becomes the section's reply.

When a file fails to parse, you get a structured error with a machine-readable kind and the byte span of the broken region. Broken frontmatter YAML keeps the original YAML parser diagnostic.

### The Execution Model

The run walks the top-level H2 sections in file order. Each section runs once, in its own isolated Lua VM, with a fresh conversation.

Here is a minimal two-section run:

````markdown
---
name: two-step
description: ask, then report
promptforge: 1
---

# Two Step

## Ask

Say something.

## Report

```lua
return reply
```
````

The `## Ask` section has prose only. The prose goes to the model as a user message, and the answer is bound to the `reply` global. The section has no Lua block that returns, so control falls through to `## Report`. The Lua block in `## Report` reads `reply` and returns it. That string is the run's result.

The fall-through rules:

- A section whose Lua block returns nothing hands control to the next section in document order.
- A `return "..."` in a section's Lua block produces output directly from Lua.
- If no Lua block in the prompt returns anything, the run's result is the last model reply. If there was no reply, the result is the string "done".
- A prompt with no H2 sections is valid. Its H1-level Lua and prose run once, first, and a return value or prose reply from the H1 content becomes the run's result.

### Frontmatter and Run Configuration

A typical frontmatter declares `name`, `description`, and `promptforge: 1`. You can add `max_tool_iterations` to cap a section's tool loop:

````markdown
---
name: researcher
description: research a topic with tools
promptforge: 1
max_tool_iterations: 5
---
````

The keys you can set:

- `promptforge: 1` - required. Declares the format version. Only major version 1 is supported.
- `name` - the prompt's name.
- `description` - what the prompt does.
- `max_tool_iterations` - caps how many model-tool round trips one section's tool loop may take. The built-in default is 24. A model that never stops requesting tools fails the run with "tool-call loop did not converge" instead of hanging.

### Document Structure

Within the H1 content and within each section, content alternates between exact `lua` code fences and prose blocks. Those are the only two block kinds. A fence with any other language tag is not a PromptForge Lua block.

The classic section shape is Lua, then prose, then Lua:

````markdown
## Summarize

```lua
local limit = 100
```

Summarize the input in under 100 words.

```lua
return reply
```
````

Lua before a section's first prose is a prologue. It runs before the model call. Lua after the prose is an epilogue. It runs after the model's reply. A prologue that returns a value ends the section early: the prose and the epilogue are skipped.

Sections nest by heading depth: H3 inside H2, H4 inside H3, and so on down through H6. The top-level walk never descends into a section's children. Children run only when you address them by heading, which the control flow section covers.

You can place content directly under the H1, before the first H2 section. This H1-level content runs once, first, before any section runs. One special fence lives here: a prompt may include at most one `lua shared` block. It defines Lua code shared across the whole prompt, and it replays as the first chunk of every section's VM. Use it for shared helper functions and for the prompt-wide declarations that later sections introduce.

A `---` thematic break changes what a section does:

- Placed as a section's first content, with only whitespace before it, `---` marks the section off-walk. The normal walk skips it. It runs only when you address it by name.
- Placed anywhere else, `---` starts a reader-only comment region. Everything below it until the next heading is excluded from execution: no Lua compiles, no prose reaches the model, no list items parse from it.

### Lua Blocks and Host Globals

Each section's Lua runs in a fresh, sandboxed VM. You have the `string`, `table`, and `math` standard libraries plus the safe base functions. There is no `io`, `os`, or `debug`.

Inside a block you can read and write a set of host globals:

````markdown
## Inspect

```lua
log('section ' .. sys.section_name)
var.seen = args
return var.seen
```
````

- `args` - the run's raw input string.
- `reply` - the previous section's model reply. Assign a string to steer what the next section sees and what the run reports. Assign `reply = nil` to clear it. The value must be a string or nil; anything else fails at section end. `reply` is nil in the first section of a prompt.
- `var` - the walk's clipboard. Writes in one section are visible in later sections, across fall-through and jumps. `var` holds JSON data only; assigning a function into it fails the run and names the field and type. Mutate its fields (`var.count = 1`); never reassign the `var` global itself.
- `sys` - runtime metadata: `sys.id` (a run-global section counter; the H1 pass is id 0), `sys.section_name`, `sys.execution`, `sys.section_count`, and `sys.reply_finish_reason` (only after prose has run).
- `log('message')` - emits a log checkpoint, callable even at shared-block load time.

Host calls that fail raise ordinary Lua errors at the call site, so `pcall` can catch them. Later sections add model, tool, control-flow, and store functions to this environment.

### Prose and Substitution

Prose in a section is sent to the model as a user message. Before the send, the runtime resolves `{{ }}` placeholders in the prose. Lua source is never substituted.

````markdown
## Greet

```lua
var.name = args
```

Say hello to {{ var.name }}.
````

When the run input is "Acme Corp", the model receives "Say hello to Acme Corp.".

The placeholders you can write:

- `{{ args }}` - the run's input string.
- `{{ reply }}` - the previous section's reply. Using it before any reply exists is a hard error.
- `{{ var.key }}` - a value your Lua wrote. Dotted paths drill into nested tables: `{{ var.row.a }}`. A whole table or array renders as compact JSON.
- `{{ sys.key }}` - runtime metadata, such as `{{ sys.model }}` for the resolved model id.
- `{{ name }}` - a section-local Lua global assigned without `local`, such as `answer = 42`.

The rules:

- `{{ var }}` and `{{ sys }}` require a `.key` suffix.
- Unknown namespaces, missing keys, null values, empty path segments, and unclosed `{{` are all hard errors with the byte offset reported.
- Substitution is a single non-recursive pass. A substituted value that contains `{{ ... }}` is emitted verbatim. Substitution does no arithmetic: compute in Lua and reference the result.
- Escape literal braces with `\{{` and `\}}`. Escape a literal backslash with `\\`.
- Within one section, prose blocks build up a multi-turn conversation.

### Models and Inference

A section with non-empty prose must have a bound model, or the run fails with "model binding required for section X". You bind models in the `lua shared` block. The simplest form declares a prompt-wide default:

````markdown
---
name: greeter
description: says hi
promptforge: 1
---

# Greeter

```lua shared
models.default('writer', 'A general model for tests')
```

## Say hi

Say hello.
````

`models.default(alias, description)` binds the alias and makes it the default, so sections that name no model still have one. The description is natural language. The runtime matches it against the model catalog at run time and freezes the invocation parameters for the run.

The full set of declarations:

- `models.bind(alias, description, options)` declares a named binding without making it the default. The options table can set `thinking`, `temperature`, `max_tokens`, and `context` (context window size): `models.bind('analyst', 'careful analysis', { thinking = false, temperature = 0, max_tokens = 256 })`.
- `models.default('writer')` with a single argument makes a previously bound alias the default.
- `models.use('analyst')` in a section's prologue selects which bound model that section runs under. Call it at most once per section, before the prose.
- `models.get('analyst')` returns a handle exposing `name` and `model_id` without changing the section's active model.

The constraints: call `models.default` at most once per prompt, and only from the H1 block. Bind an alias before you use it. Duplicate aliases are rejected. A requested `context` size beyond what the model supports fails the bind.

You can also call a model directly from Lua, without prose:

````markdown
```lua
local tag = models.infer('Classify: ' .. args)
```
````

- `models.infer(prompt)` runs a one-shot, tool-free inference on a fresh single-message conversation, using the section's current model. It returns the reply as a plain string. It does not set `reply` or touch `sys`.
- `handle:infer(prompt)` does the same against a specific bound model, regardless of the section's active model: `models.get('analyst'):infer('ping')`. A bound alias also works directly as a global: `writer:infer('say hello')`.

Use direct inference for cheap auxiliary work: classification, extraction, rewriting.

### Tools

You declare tools by capability in the shared block:

````markdown
```lua shared
tools.bind('search', 'web search')
```
````

`tools.bind(alias, capability)` matches a natural-language capability description against the tool catalog at run time. Web search is a built-in capability.

A bound tool is withheld from the model until you scope it:

- `tools.always('search')` in the shared block exposes it in every section.
- `tools.add('search')` in a section's own Lua block exposes it in that section only.
- `tools.add('search', 'Find current facts on the web')` overrides the model-facing description at the point of use.

Here is a complete prompt with a scoped tool:

````markdown
---
name: researcher
description: answer with web search
promptforge: 1
---

# Researcher

```lua shared
models.default('writer', 'A general model for tests')
tools.bind('search', 'web search')
```

## Answer

```lua
tools.add('search')
```

Answer the question: {{ args }}.

```lua
assert(tools.calls['search'] > 0, 'search was never called')
return reply
```
````

The model calls the tool by the alias you bound. The section runs a multi-round tool-call loop: the model calls tools, then produces a final text answer. The `max_tool_iterations` frontmatter key caps the loop. Only the last prose block of a section runs the full tool loop; earlier prose blocks are single-shot. The epilogue reads `tools.calls['search']` to assert the model actually called the tool.

You can also define a Lua-backed local tool inside a section:

````markdown
```lua
tools.add_local('grab', 'Grab a value', { value = 'string' }, function(args)
  return 'got ' .. args.value
end)
```
````

`tools.add_local(name, description, schema, handler)` declares the tool in place. The schema is a Lua table mapping argument names to JSON types. The handler receives the model's arguments as an `args` table and returns a string that reaches the model verbatim. A local tool alias must not collide with a `tools.bind` alias or with another local tool in the same section.

### Control Flow

You already know the basic exit rule: a section whose Lua returns nothing falls through. The full rules:

- A scalar return from a prologue (early) Lua block ends just that section.
- A scalar return from an epilogue (late) Lua block ends the entire run.
- A return from the H1 block ends the run before any section runs.
- Running off the last section ends the run: the result is the last model reply, else "done".

Three functions move control between sections.

`jump('## Heading')` transfers control to another section by heading:

````markdown
```lua
jump('## Help')
```
````

The jump ends the current block immediately and skips the section's remaining blocks. The conversation is cleared, but the current `reply` and `var` carry across. Jumping to a child heading such as `### X` starts a child-level walk over that section's children; the parent walk resumes after the jumper when the child level exhausts.

`execute('## Heading')` runs a contained sub-chain and returns its final reply:

````markdown
```lua
local findings = execute('## Research')
```
````

The chain runs with a fresh VM and a fresh conversation. An optional second argument passes an input string that overrides the child's `args`: `execute('## Research', 'chain-args')`. A `return` inside the chain ends only the chain, not the run, and the outer walk resumes at the section after the caller. The chain gets a clone of the caller's `var`; the child's `var` writes are discarded. Chains nest to a cap of 8.

Off-walk sections act as shared subroutines. The walk skips them, but `execute` and `jump` run them on demand.

`list_from_section('### Items')` reads a list section's bullet or numbered items into Lua as an array of strings, with markers stripped:

````markdown
## Gather

```lua
local items = list_from_section('### Items')
return table.concat(items, ',')
```

### Items

---

- alpha
- beta
````

The off-walk marker keeps the list out of the walk, so it serves as a reusable item source.

Addressing rules apply to all three functions. Headings must match level and name exactly: `'### Items'`, not `'Items'`. A section's visible set is its siblings minus itself, plus its direct children. A section cannot address itself, its nieces and nephews, or (from inside a child) top-level sections. Not-found errors list the available visible sections. Two visible sections sharing a level and name produce an ambiguity error. Called from the H1 block, `execute`, `jump`, and `list_from_section` fail with "only available in sections". A local tool handler cannot call `jump`.

### Fanout

`fanout(worker, collection)` maps a worker section over a collection, running the worker once per member, concurrently:

````markdown
---
name: batch
description: reply about each item
promptforge: 1
---

# Batch

```lua shared
models.default('writer', 'A general model for tests')
```

## Run

```lua
local r = fanout('### Worker', list_from_section('### Items'))
return table.concat(r, ',')
```

### Items

---

- alpha
- beta

### Worker

---

Reply about {{ item }}.
````

The walk never visits the off-walk `### Items` or `### Worker` sections. The fanout runs the worker once per item and returns the results.

What each arm sees and what you get back:

- Each array member arrives inside the arm as the `item` global, in its native Lua type. A hash member arrives as a pair table with `item.key` and `item.value`.
- `{{ item }}` in worker prose interpolates the arm's member. Non-string items render as compact JSON.
- `sys.index` gives the arm's 1-based position within its fanout. `sys.id` continues the run-global sequence.
- `fanout` returns a Lua array of per-arm results in collection order, not finish order. Each result has `text`, `ok`, `item`, and `exhausted` fields. Results stringify to their text, so `table.concat(r, ',')` works.

The constraints:

- Up to 8 arms run at once by default.
- The collection must be a Lua table. An empty collection is an immediate error.
- A list section cannot be a fanout worker.
- Called from the H1 block, `fanout` fails with "only available in sections".
- An arm whose tool loop exhausts soft-degrades into a failure result (`ok == false`, `exhausted == true`) instead of killing the fanout. A fatal error in one arm aborts its queued and in-flight siblings.

### The Store

The store is a run-scoped virtual filesystem. You read and write virtual files addressed by logical string paths. One store is shared across all sections of a run, and it survives the context-clearing transitions that wipe each section's conversation.

````markdown
## Writer

```lua
store.write('note.txt', 'carried across')
```

## Reader

```lua
return store.read('note.txt')
```
````

The operations, one per line:

- `store.write(path, value)` writes a file.
- `store.read(path)` reads the verbatim contents.
- `store.read(path, start, end)` reads a 1-based inclusive slice of lines.
- `store.read_numbered(path, start, end)` reads a line range with absolute line numbers attached. With no bounds it numbers the whole file from 1.
- `store.append(path, value)` accumulates onto a file.
- `store.delete(path)` deletes a file.
- `store.exists(path)` checks whether a file exists.
- `store.glob(pattern)` lists matching files.
- `store.str_replace(path, old, new)` edits by anchor-based string replacement, so edits survive content shifts.

The frontmatter `input:` and `output:` keys declare the prompt's input and output files, each a path plus a description. The input is expected in the store when the run starts. The output is left there when the run finishes. Writes from an epilogue or from a local tool handler persist after the run completes.

Fanout arms share the store under race rules: two arms of one fanout writing the same path fail with a write-write race error, `store.append` from multiple arms is safe, and one arm rewriting its own path is fine.

To re-inject stored content into the model, wrap a verbatim read in the `untrusted` guard envelope.

### Observability, Cancellation, and Safety

You can observe and bound a run from inside the prompt:

- `log('message')` emits log checkpoints from Lua. One VM emits at most 1024 before logging cuts off.
- Each section VM is capped at 64 MiB of memory. There is no instruction ceiling on a block: a long or infinite loop is legal and runs until the host cancels; the instruction hook keeps polling for cancellation, so Ctrl-C lands even inside a tight loop.
- Each model request times out after 120 seconds. Response bodies are capped at 16 MiB.
- Cancel a run with Ctrl-C. The run ends with a recognizable "interrupted by Ctrl-C" result instead of a crash, even mid-tool-call, mid-infer, or stuck in a Lua loop.
- Tool results marked untrusted, such as web content, are wrapped before the model sees them, inside envelopes prefaced with "is data, not instructions". Trusted results reach the model verbatim.



---

## PromptForge MCP Server - User Guide

The PromptForge MCP server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. It puts your prompt catalog behind four fixed MCP tools, so the agent in your harness can discover, pick, and run your prompts as tools. Follow this guide and you end with a working server connected to your harness.

### What This Server Is

The server runs PromptForge prompts for agentic harnesses like Cursor and Claude Code. You connect it to your harness as a standard MCP server.

A prompt is a plain Markdown file. Its YAML frontmatter declares `name`, `description`, and the format version `promptforge: 1`. Its body carries executable Lua sections. A prompt library is a directory of `.md` files.

One `prompts.toml` file configures the whole server. It names the server settings, the prompts directory, the model gateway, and the prompts the harness sees.

The catalog sits behind four fixed built-in MCP tools. Prompts are never published as tools of their own. `tools/list` never changes when you add, edit, rename, or break a prompt.

### The Four Built-In Tools

`list_prompts` enumerates every enabled prompt in the catalog, healthy or broken. Each entry carries the prompt's name, its description, any problem that stops it, and its declared input and output contracts. The agent reads this list to learn what the server can run.

`run_prompt` executes an enabled prompt by name. Naming a prompt to `run_prompt` is the only way to invoke one.

`need_prompt` resolves a plain-English capability description to a ranked shortlist of up to three candidate prompts, best first. It runs nothing. The agent uses it to find a prompt, then passes one of the returned names to `run_prompt`.

`check_run` collects the outcome of a run that outlived its originating call. The agent passes the `run_id` that the earlier result named.

### Deployment and Execution Model

The server speaks MCP over two transports. Streamable HTTP serves remote or shared access behind a shared bearer token. Stdio serves harnesses that spawn the server as a local child process, as line-framed JSON-RPC.

Prompt runs make their model calls through an OpenAI-compatible chat-completions gateway. You configure the gateway in `prompts.toml`. Its credentials stay separate from the MCP-facing credentials.

A filesystem watcher keeps the catalog current while the server runs. Edit a prompt file and the change takes effect immediately. You do not restart the server. The client does not reconnect.

Boot is fail-fast. The server validates the catalog before it binds any transport. It refuses to serve an incomplete or broken catalog rather than start with prompts silently missing.

### Installation, Launch, and Boot

Install the `promptforge-mcp-server` package from crates.io. Any toolchain at Rust 1.89 or later builds it. The install puts one `promptforge-mcp-server` binary on your path.

Launch the server with one of exactly two command-line shapes. Use the first for HTTP. Use the second for stdio.

````console
promptforge-mcp-server serve prompts.toml
promptforge-mcp-server serve --stdio prompts.toml
````

Any other command line prints a usage error and exits nonzero.

A healthy boot logs each boot step - catalog resolve, retrieval index, tool build - and ends serving:

````text
promptforge-mcp-server serving on http://127.0.0.1:9310/mcp
````

A refused boot prints every catalog fault in one pass. Each fault names its prompt and its file. The process then exits nonzero, so you fix all faults in one pass rather than one restart each. A missing config file error reports the exact path and distinguishes a missing file from a permission failure.

Script around the binary with conventional exit codes: zero on a clean serve, nonzero on any boot or argument failure. Stop the server with Ctrl-C. Both transports drain and close.

### Server Configuration and Secrets

Start with a minimal `prompts.toml`. This is a complete config for HTTP:

````toml
[server]
api_key = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"
````

Every setting you omit takes a default. A full config shows what you can tune:

````toml
[server]
bind = "127.0.0.1:9310"
api_key = "shared-bearer"
allowed_hosts = ["localhost", "127.0.0.1", "::1"]
max_concurrent_runs = 4
admission_timeout = "30s"
reply_deadline = "240s"
retain_completed = "1h"
watch = true
watch_debounce = "500ms"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"

[paths]
prompts = "prompts"

[tools]
web_fetch = true
web_search = true
````

Under `[server]`:

- `bind` sets the HTTP bind address. Default `127.0.0.1:9310`.
- `api_key` sets the shared bearer token every `/mcp` request must present. Omit it for a local stdio install; `serve --stdio` never reads it.
- `allowed_hosts` lists the host authorities the server accepts.
- `max_concurrent_runs` sets how many prompts run at once. Default 4.
- `admission_timeout` sets how long a call waits for a run slot. Default `30s`.
- `reply_deadline` sets how long a call waits for its run. Default `240s`, kept under Cursor's ~300-second call ceiling.
- `retain_completed` sets how long a finished run stays collectable. Default `1h`.
- `watch` toggles hot reload. Default on.
- `watch_debounce` sets how long the watcher waits for filesystem events to settle. Default `500ms`.

Write durations in human-readable form: `30s`, `4m`, `500ms`, `1h`.

Under `[gateway]`, set `url` and `api_key`. Both are required. The URL must be a real http or https URL with a host. All prompt-run model traffic goes to this gateway.

Under `[tools]`, opt into `web_fetch` and `web_search` to grant prompts live web access. Both default to disabled. A prompt with no `[tools]` section runs in a true sandbox with no network access.

Keep secrets and machine-specific values out of `prompts.toml`. Write `${VAR}` references in any TOML string value:

````toml
[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "${GATEWAY_KEY}"
````

A name-matched `prompts.env` file beside `prompts.toml` can supply the values. Real environment variables always win over the file. File values never enter the process environment. A missing or malformed `prompts.env` never fails the load. Interpolation works in nested arrays and sub-tables, not just top-level strings.

An unset variable aborts the load and names the exact field. The one exception is `[server].api_key`: an unset variable there leaves the key absent, so stdio installs stay unblocked. Write `$$` for a literal dollar sign. A bare `$` not followed by `$` or `{` passes through literally. An unclosed `${...` is a load error.

The server refuses blank or whitespace-only secrets at load. Secrets redact as `Secret(redacted)` in all debug and display output and never serialize. Unknown or misspelled config keys fail the load and name the offending key. A config file over 4 MiB is refused.

### Catalog Configuration and Resolution

Point the server at your prompts directory with `[paths].prompts`. The default is `prompts/` relative to the working directory. Relative and absolute paths both work.

Select which prompt files enter the catalog with glob patterns:

````toml
[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["_*.md", "drafts/**"]
````

`*` matches within one path segment. `**` crosses separators. Matching is case-sensitive. A recursive pattern like `governance/**/*.md` reaches nested directories while `*.md` matches only the top level. Exclusions always win over inclusions. Exclude patterns match root-relative paths, so `drafts/**` means what it reads as.

Override glob results per prompt with a `[prompts.NAME]` block:

````toml
[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
````

Set `enabled = false` to drop a prompt the globs caught. Set `file` to publish a file no glob matches. The path is relative to the prompts directory. Absolute paths and any `..` component are rejected at config load. A leading `./` is accepted, and Windows backslash paths parse.

Keep ordinary non-prompt Markdown files in the prompts directory. A glob-matched file with no `promptforge:` frontmatter marker is skipped without comment. Notes and drafts never leak into the tool surface.

Prompt names must match `^[a-z][a-z0-9_]{0,47}$`: a lowercase ASCII start, then lowercase letters, digits, and underscores, 48 characters maximum. The four built-in tool names are reserved in every build. Two healthy prompts declaring the same name is a fault that lists every file that declared it. An empty resolved catalog is a hard fault; the server never boots serving nothing. A block whose `file` declares a different frontmatter name than the block key is a hard fault naming both names. A block with no `file` that matches no globbed prompt is a stale-override fault naming the dead key.

A prompt file over 2 MiB is refused as a broken entry. Every served file is confined to the prompts directory: a symlink or reparse point under the root that points outside it is resolved and dropped.

Boot and reload treat a broken prompt differently. Boot rejects it and refuses to serve. Reload retains it as a broken entry: still listed under a placeholder name suffixed `(broken)`, sorted after healthy entries, exposing no source text. Calling it returns the validation failure rather than silently running a stale copy. The catalog listing is always ordered by prompt name.

### Prompt Authoring

A prompt is a Markdown file with YAML frontmatter and Lua code blocks. This is a complete prompt:

````markdown
---
name: echo
description: Returns its argument
promptforge: 1
---

# Echo

## Main

```lua
return args
```
````

The frontmatter declares `name`, `description`, and `promptforge: 1`. Each `##` section carries a Lua code block.

A Lua prologue runs before any model call. Return a value from it to short-circuit the whole run. In a multi-section prompt, each section either falls through to the next or returns a final value to end the run. A Lua-visible `var` store shares state across section boundaries.

Declare input and output file contracts in the frontmatter. Give each a path and a description:

````markdown
---
name: reader
description: Reads its input and writes it to its output
promptforge: 1
input:
  path: paper.md
  description: The input file
output:
  path: report.md
  description: The output file
---

# Reader

## Main

```lua
local content = store.read("paper.md")
store.write("report.md", content)
return "done"
```
````

The prompt reads and writes the declared files at run time through `store.read(path)` and `store.write(path, content)`.

Bind external capabilities to tool names with `tools.bind(name, capability)`, and activate them per section with `tools.add(name)`. Declare capabilities in natural language; the server resolves them to enabled tools at run time. This scopes which tools a model may use in each section.

Bind a model to a named role with `models.default(role, description)` or `models.bind(alias, description, opts)`, and pick a role per section with `models.use(alias)`. The prompt chooses which gateway model serves each prose section. Role bindings resolve live against the gateway model catalog at boot.

Try the shipped example catalog to see working prompts: analyst_example, echo, greet, hello, research_person. It loads and serves exactly as shipped, out of the box.

### Running and Collecting

The simplest call names a prompt and nothing else:

````json
{ "prompt": "echo" }
````

Pass the prompt's input as one raw string with `args`. Omitting it passes the empty string.

````json
{ "prompt": "echo", "args": "hello" }
````

Seed a declared input with `input_file` (a filesystem path) or `input_text` (text placed directly in the prompt's store). The two are mutually exclusive; specify one, not both. Write a declared output to disk with `output_file`. Omit it to receive the output inline as the result value.

Every run outcome comes back in `structuredContent`, a flat JSON object:

````json
{ "run_id": "0123456789abcdef0123456789abcdef", "prompt": "echo", "status": "completed", "value": "hello", "turns": 0, "elapsed_ms": 4, "error": null }
````

A plain text block mirrors it: the value on completion, the error on failure, a collection instruction while running. `status` serializes as `running`, `completed`, or `failed`. A `completed` result always carries a `value` and a null `error`. A `failed` result always carries an `error` and a null `value`. `turns` counts model round trips; a Lua-only prompt reports zero. `elapsed_ms` measures only the run itself, never the queue wait.

A run can outlive its call. Past `reply_deadline`, the call returns a `running` result naming a `run_id` instead of failing. The run keeps executing in the background under a supervisor. Collect the finished record later with `check_run`:

````json
{ "run_id": "0123456789abcdef0123456789abcdef" }
````

Run ids are 128 random bits rendered as 32 hex digits. A finished run stays collectable for `retain_completed` and is then evicted. A still-running run is never evicted and reports its live elapsed time. Polling an unknown or evicted `run_id` returns a tool error whose message names the retention window. A run started in one HTTP session is collectable by `check_run` from a different session.

Stop a run by abandoning the awaiting call. When the client cancels the request or disconnects mid-wait, the run is signalled to cancel and its concurrency slot frees for a fresh run.

Recover from a mistyped prompt name by reading the error result. It lists every enabled prompt name ordered nearest-first, and nothing is run on a miss. Name resolution folds letter case and treats `-` and `_` as the same character, so `Research-Person` reaches `research_person`.

Admission is bounded. A call waits up to `admission_timeout` for one of `max_concurrent_runs` slots, then is refused with a retryable answer naming the exact wait spent.

### Discovery, Retrieval, and Tool Surface

Call `list_prompts` with no arguments to read the first page of the catalog:

````json
{}
````

Page through a large catalog with the optional `cursor` parameter. A page carries at most 100 entries. `next_cursor` in the response fetches the next page. A cursor the server never issued is a `-32602` invalid-params error.

Call `need_prompt` with a `capability` string to find a prompt without reading the whole catalog:

````json
{ "capability": "Build a stakeholder position report for one entity." }
````

Phrase the capability in author register: a short imperative phrase naming the operation and what it acts on, with no entity names or conversational framing. Good: "Build a stakeholder position report for one entity." Bad: "I need to know what Herb Sutter has said about ABI stability." Casual phrasings still return candidates. A capability over 4096 bytes is rejected with a message telling you to restate it as one short imperative.

The shortlist holds up to three candidates, best first. Each candidate carries a `name` you can pass to `run_prompt` and its verbatim `description`. An empty candidate list is a complete answer - "no prompt is close to this" - not an error. Broken prompts are never recommended. If the retrieval index is unavailable, `need_prompt` says so and redirects you to `list_prompts`.

The tool list is fixed for the life of the process: `list_prompts`, `run_prompt`, `need_prompt`, `check_run`, in that order. All four input schemas set `additionalProperties: false`, so a misspelled or obsolete argument is refused, not silently dropped. A prompt name is never dispatchable as a tool: calling `echo` directly returns METHOD_NOT_FOUND. A build without the `picker` feature publishes three tools instead of four, dropping `need_prompt`.

### Progress, Logging, and Error Surface

Attach a `progressToken` to a `tools/call` to receive live `notifications/progress` updates in Cursor or Claude Code while a multi-minute run is in flight. Frame 0 is captioned with the prompt's H1 title the moment the run starts. Each later frame is captioned with a section's H2 heading. Values latch monotonically from 0 and `total` is never sent, so the client shows a changing caption rather than a filling bar. Progress is strictly best-effort: a client that stops accepting notifications silently ends the stream without failing the call. Omit the token and the run is silent with an identical result.

Watch operations through structured logs. Every run boundary is an `info` event. Within-run chatter stays at `debug`. Failed tool calls and failed model turns surface at `warn`. Logs go to stdout normally and to stderr in stdio mode, so log capture never collides with the MCP wire protocol. Terminal run records carry run_id, prompt, status, turns, and elapsed_ms. Prompt content never reaches the log. Boot progress appears as log records for catalog resolve, retrieval index, and tool build, weighted by expected duration.

Read model-correctable failures as ordinary tool results with `isError` set: a broken prompt, an unresolvable capability, a refused admission, a failed run. The calling model reads the corrective detail and acts. Only malformed argument shapes are protocol errors. A missing required argument is a `-32602` error naming the key. An explicit `null` for an optional string is rejected as a client bug rather than coerced to empty.

### Transports and Security

On stdio, the server binds no network listener and reads no token. A config that sets `bind` or `api_key` anyway is logged as ignored. Each JSON-RPC message is one line. A line over 4 MiB, or a malformed line, is dropped and the session survives. EOF on stdin ends the session cleanly.

Over HTTP, the server serves MCP at `/mcp`. Every `/mcp` request must present the shared bearer token from `[server].api_key`. The check runs per request, not per session: a rotated-away token is refused on the very next request, even on an initialized session. The `Bearer` scheme is matched case-insensitively. Refusals are 401 with a `WWW-Authenticate: Bearer` header. HTTP refuses to bind without `[server].api_key`, before the socket is bound.

`allowed_hosts` validates the request `Host` header as a DNS-rebinding defence. Empty on a loopback bind defaults to `localhost`, `127.0.0.1`, `::1`. Empty on a non-loopback bind refuses to start with an error naming the bind address and the required setting; enumerate the public authorities instead, for example `["example.com", "example.com:8080"]`. A disallowed Host is rejected with 403 even with a valid token.

`/healthz` is public, outside the bearer layer, and returns `{"status": "serving"}`. A 15-second SSE keep-alive keeps long-running tool calls alive through proxies. The server speaks MCP protocol revision 2025-06-18 and does not advertise tool-list change notifications, because the tool list never moves.

### Hot Reload and the Watcher

The watcher is on by default. Add, edit, rename, or delete prompt files while the server runs. The change is live on the very next tool call on the same already-open MCP session, with no reconnect and no client notification. `watch_debounce` (default `500ms`) lets filesystem events settle before a reload, so one save costs one reload.

Edit `prompts.toml` itself and the save triggers the same reload path. Catalog-shaping changes apply on the next reload. These settings stay pinned to boot values and are logged by name as requiring a restart: `[server].bind`, `[server].api_key`, `[server].max_concurrent_runs`, `[server].admission_timeout`, `[server].reply_deadline`, `[server].retain_completed`, `[server].watch`, `[server].watch_debounce`, `[server].allowed_hosts`, `[paths].prompts`, `[gateway].url`, and `[gateway].api_key`.

Tolerate a prompt broken by a bad save. It stays listed as a broken entry carrying its error, and the rest of the catalog keeps serving. A reload that cannot re-resolve - an unparsable `prompts.toml`, a stale override, duplicate names, an empty result - keeps the previous catalog and logs the reason. A typo in one file never takes the running service down. Each reload logs its outcome: how many prompts loaded, how many are broken, whether ranking changed, and whether the retrieval index is current or stale.

Set `watch = false` to serve exactly the boot-resolved catalog for the life of the process.

---

## Semantic Tool Binding in PromptForge

PromptForge decides which tools a prompt can use. You describe each tool in plain prose. PromptForge matches the intent of a prompt against those descriptions with a small embedding model that runs on your machine. There is no LLM call. There is no network access at runtime. There is no keyword matching. The right tools reach the model. The wrong ones stay out. This guide shows you how to declare tools, tune the match, and read the results.

### What Tool Binding Does

You write a need in plain prose, for example "read a file from disk". PromptForge resolves that need to the tool that performs it. The match uses sentence embeddings. The whole embedding model is compiled into the library. You configure no model path. You ship no weights file. You make no runtime network call.

You describe your tools in a catalog. Each entry in the catalog is a tool descriptor. A descriptor pairs a tool identity with a natural-language description and a JSON Schema for its arguments. The identity has two parts: a server name and a tool name. The pair identifies a tool without ambiguity. Two tools with the same name on different servers never collide. Delimiter characters inside either part stay unambiguous.

You build a picker over the catalog. You then ask the picker which tool a given need refers to. You build the picker once. You ask it about as many needs as you like.

Every query returns one of four outcomes. The picker can bind one tool. It can report a group of duplicate tools published by one server. It can return a shortlist of candidates it could not separate. It can abstain when nothing fits.

### The Four Outcomes of a Match

Each need you resolve ends in exactly one outcome. You handle each case in your own code.

**Bound.** One tool cleared the similarity floor (the minimum score a candidate must reach) and left the runner-up behind by at least the configured margin. You call the chosen tool immediately.

**Duplicate.** One server publishes two tools that are near-verbatim copies of each other. This is a catalog fault. The picker fails loudly and names the pair. The group always holds at least two members, in ranked order. You fix the catalog.

**Ambiguous.** Two or more tools sit within the decision margin and the picker cannot separate them. This happens most often when one tool is republished across two servers. The group always holds at least two members, in ranked order. You choose for yourself, or you sharpen the descriptions.

**Absent.** Nothing in the catalog matched the need well enough to offer. An abstention is a successful answer, not an error. You can tell an abstention apart from an engine failure. An abstention means the policy answered. An error means the engine could not run.

One rule sits between binding and abstention. The solo floor is a second, lower score bar. A lone candidate that scores at or above the solo floor, but below the similarity floor, still binds when no runner-up reaches the solo floor. There is nothing to confuse it with. Two such candidates cause an abstention instead. Section "Setting Match Thresholds" gives the defaults for both floors.

The same tool republished on one server is a duplicate. The same tool republished across two servers is ambiguous. The distinction is the server name in the identity.

### Declaring Tools for Matching

Tools enter the catalog as descriptors. You write each descriptor. A descriptor carries four things: the server name, the tool name, your description, and a JSON Schema for the arguments. A descriptor carries nothing that could invoke the tool. Mapping a resolved descriptor onto something callable is your job.

The engine matches against three parts of each tool: the tool name with underscores removed, the description, and the parameter names in sorted order. Your wording directly steers the match. Parameter names in the schema affect semantic matching.

A minimal descriptor in JSON looks like this:

````json
{
  "server": "files",
  "name": "read_file",
  "description": "Read a file from disk",
  "inputSchema": {
    "properties": {
      "path": { "type": "string" }
    }
  }
}
````

The schema field accepts both `input_schema` and the MCP spelling `inputSchema`. Optional fields can be omitted. A missing schema becomes null on load. Missing annotations become the default, with every hint absent.

You can attach MCP behavioral hints to a descriptor: read-only, destructive, idempotent. Each hint is optional and absent by default. An absent hint never changes a ranking. Hints act only as a tie-break between candidates with identical scores. A positive read-only claim wins first, then a non-destructive claim, then an idempotent claim.

````json
{
  "server": "files",
  "name": "read_file",
  "description": "Read a file from disk",
  "inputSchema": {
    "properties": {
      "path": { "type": "string" }
    }
  },
  "annotations": { "readOnlyHint": true }
}
````

You assemble descriptors into a catalog. The catalog is the sole input contract of the picker. Order is preserved. Duplicate identities are accepted, not refused. Two tools claiming the same identity is a result the engine reports, not an input it rejects. You can look up the first descriptor that matches a given identity. You can ask the catalog its size. You can iterate its descriptors in the order you gave them.

A catalog serializes as a plain JSON array of descriptors. It round-trips losslessly. You can commit a catalog as data and load it back.

### Setting Match Thresholds

Five thresholds steer which of the four outcomes a need receives. The defaults are pre-calibrated. They were measured against a real catalog. You can resolve tools without tuning anything.

| Key | Default | Effect |
|---|---|---|
| `similarity_floor` | 0.825 | The minimum cosine similarity a candidate must reach to be considered at all. Raise it to bind less often. Lower it to consider weaker matches. |
| `margin` | 0.05 | The score gap the top candidate must clear the runner-up by before the engine binds. Raise it to demand a clearer winner. Set it to zero to let annotation hints choose between tied tools. |
| `duplicate_threshold` | 0.98 | The similarity at or above which two tools are treated as twins. The comparison uses the tools' own embeddings, not the query. |
| `solo_floor` | 0.5 | The minimum score at which a lone candidate still binds. Set it equal to `similarity_floor` to disable the solo rule. |
| `top_k` | 3 | How many candidates an ambiguous or duplicate outcome reports back. Must be nonzero. |

You tune the thresholds with checked setters that start from the defaults. Each threshold must be finite and within 0.0..=1.0. `top_k` must be nonzero. An out-of-domain value produces a configuration error that names the rejected field. Every stored configuration is always valid. There is no separate validation step to remember.

You can persist or transmit a configuration as JSON. Every field is optional. Absent fields are filled from the calibrated defaults.

````json
{
  "similarity_floor": 0.85,
  "top_k": 5
}
````

Invalid values in a configuration file are rejected, not silently accepted. A document with `similarity_floor` set to 2.0 fails. A document with `top_k` set to 0 fails.

Every threshold boundary is inclusive. A score exactly at the floor is considered. A gap exactly equal to the margin binds. A pair exactly at the duplicate threshold is a twin.

### Reading the Shortlist

You can ask for a shortlist instead of a decision. Use this when you would rather choose for yourself, for example when an end user picks the tool.

A shortlist is the best N tools for a need, ranked best first. You choose the cap. A shortlist lists candidates above the similarity floor without making a final decision. The solo-candidate exception is preserved: a lone leader between the solo floor and the strict floor is offered. A limit of zero returns an empty shortlist without paying for an embedding. A shortlist never drops either side of a tie. Even with `top_k` set to 1, a tie yields two entries.

Resolve and shortlist never contradict each other. If resolve abstains, shortlist returns nothing. If resolve binds a tool, shortlist offers exactly that tool.

You can inspect an ambiguous or duplicate group the same way. You can ask its length. You can take the first or second candidate. You can index into it. You can iterate it.

You can also detect near-duplicate pairs inside a chosen set of tool identities. The analysis compares the selected tools' own embeddings against the duplicate threshold. It runs independent of any query. Every requested identity is validated before any pair is compared. A missing identity fails the whole analysis and names the first absent one. Repeated identities collapse to set membership. Each detected pair exposes the two tools and their exact cosine similarity score. Pairs come back in deterministic catalog order, regardless of the order you requested.

Use these views to act. Tune descriptions. Split overloaded tools. Delete copy-pasted duplicates. Resolve ambiguity before it reaches the model.

### Writing Better Tool Descriptions

The engine reads the de-underscored tool name, the description, and the sorted parameter names. Write all three for the match.

- State the action in the description. A need that restates one tool's capability binds that tool. "Read a file from disk" binds a need phrased as "read the contents of a file from disk".
- Name parameters with meaningful words. Parameter names are part of the matched text. A schema full of `arg1` and `arg2` tells the engine nothing.
- Keep sibling tools distinct. Two tools that differ only by a copy-pasted name will surface as duplicates. Two tools that genuinely cover the same ground will surface as ambiguous. Both outcomes tell you to sharpen the wording.
- Attach behavioral hints to otherwise identical tools. On an exact score tie, a positive read-only claim wins first, then a non-destructive claim, then an idempotent claim. Catalog position decides when hints are absent or equal.
- Treat abstention as a wording signal. If a need you expect to match comes back absent, the description does not cover that phrasing. Broaden the description or lower `similarity_floor`.
- Treat ambiguity as an overlap signal. If two tools tie, their descriptions claim the same capability. Differentiate the descriptions, or attach hints so the tie breaks your way.

### Building and Rebuilding the Index

You build a picker from a catalog in a single call. The build loads the compiled-in model and indexes every tool. A build error is reported if the model cannot load or the catalog cannot be indexed. An empty catalog builds without error and reports every need as absent.

Loading the model is the expensive step. It happens once. You keep the returned handle and reuse it. Cloning a loaded handle is cheap. Several pickers share the same weights instead of reloading them. You can serve several catalogs from one loaded model. Each picker resolves only its own catalog's tools.

You can replace a picker's catalog with a new one. The rebuild preserves the model and the configuration. The original picker is left unchanged and still answers from its own catalog.

You can observe a picker. You can ask how many tools it indexes. You can ask whether it is empty. You can iterate its tools in catalog order, including reverse. You can look up a tool by identity. You can read back the configuration it was built with. Debug output shows the index size and shape. It never dumps raw embedding vectors.

Results borrow the picker's descriptors. No schema or descriptor is deep-cloned. To keep a resolved tool identity beyond the picker's lifetime, clone just the identity.

While the model loads, you can watch byte-level progress through an optional progress handle. While indexing, the handle advances one step per embedded tool. It completes even for an empty catalog.

### The Embedding Model Asset

The library embeds the BAAI/bge-small-en-v1.5 model, 384 dimensions, compiled into the binary. It runs locally on CPU. The finished binary needs no runtime download.

The first build needs network access. It downloads about 130 MB from the Hugging Face Hub. Later builds reuse the cache. Every downloaded file is pinned to one immutable commit and checked against a hardcoded SHA-256 digest before use. If a checksum fails, the build error names the expected and actual digests and the cache path. You delete the corrupt or tampered cached copy and rebuild.

To build offline or behind a proxy, point `HF_HUB_CACHE` or `HF_HOME` at a warm Hugging Face cache, or set `HF_ENDPOINT` to a reachable mirror.

The build downcasts the model weights from fp32 to fp16 before embedding them. The shipped binary is smaller. Repeated rebuilds skip the download and conversion work. A stamp file records the pinned revision, the conversion version, and the digests of the generated outputs. Corrupted, truncated, or replaced outputs are detected and regenerated. All generated artifacts land under the build output directory inside `target/`. Nothing is written into the source tree. At compile time you can inspect provenance: the pinned revision and the source repository are recorded alongside the embedded assets.

If the Hugging Face Hub is unreachable, the build error states which file could not be obtained, why network access is needed, and the full cause chain.

### Errors and What They Mean

Each fallible operation reports its own narrow failure category. There is no single catch-all error.

- **Model-load failure.** The compiled-in weights, tokenizer, or configuration could not be turned into a usable encoder. Every cause is a build defect. There is nothing to fix at the call site. The message names the category: configuration, dimension mismatch, provenance, tokenizer, truncation, weights, or architecture.
- **Index failure.** The catalog could not be indexed. Model-load and index failures flow into the single build error when you use the one-call build.
- **Query failure.** The need itself could not be embedded. The failure classifies into a stable category: tokenization, inference, or invalid embedding. A query failure is not an abstention. An abstention is a successful policy answer.
- **Selection failure.** A near-duplicate analysis referenced a tool not in the catalog. The error names the first missing tool identity. Selection analysis is validation, so an absent identity fails loudly rather than being silently dropped.
- **Configuration failure.** A threshold or `top_k` fell outside the supported domain. The error names the rejected field.

You can walk the underlying dependency cause of any failure through the standard error source chain. Error messages are compact lowercase noun phrases. They display transparently without wrapper noise.

### Guarantees at a Glance

- Determinism: the same model bytes, dependency versions, target, environment, catalog, configuration, and need always produce the same outcome. Cross-platform byte-identical vectors at floating-point boundaries are not promised.
- Thread safety: the model handle and the picker move or share across threads. They work in static and async contexts.
- Shared weights: two pickers over one model produce byte-identical embeddings for identical text.
- Zero-copy results: query results, shortlists, and duplicate pairs borrow the picker's descriptors. No schema or descriptor is deep-cloned.
- Model reuse: load once, clone cheaply, serve many catalogs.
- Stable text embedding: the same text always embeds to the identical vector. Cached or persisted vectors stay valid.

---

## Web Fetch Tool

The `web_fetch` tool fetches one web page and returns its main content as markdown. You call it from a prompt with a URL. The tool reads the page, strips the boilerplate, and gives the model clean text it can cite. It also guards your network. It checks every URL before any request leaves the machine. You add live web content to your prompts without opening a security hole.

### What the Tool Does

You give the tool a URL. It fetches that page and returns the main content as markdown. That is the whole scope.

You supply the exact URL. The tool does no search, crawling, or discovery on its own. If the model needs a page, the prompt must name the page.

You can let the model choose URLs at runtime. The tool is the security boundary between an untrusted model-supplied URL and the network. It validates every URL before any network access. It blocks private, loopback, and otherwise restricted IP ranges on every fetch. This protection is automatic. You do not configure it in the prompt.

### Fetching a Page

You invoke the tool by its name, `web_fetch`. A call passes a single JSON argument. The `url` string parameter is required. It names the page to fetch.

The simplest call looks like this:

````json
{
  "url": "https://example.com"
}
````

This call fetches the page and returns its content as text in the tool output. There is one tool and no auxiliary API to learn.

Every successful result opens with a provenance header. The header gives the final URL, a truncated flag, and the extraction mode. The content body follows the header.

````text
url: https://example.com/
truncated: false
extraction: readability

Example Domain

This domain is for use in illustrative examples in documents.
````

Read the header before the body. The `truncated` flag tells you whether the tool cut the text short. When it reads `truncated: true`, you may need a follow-up fetch with different parameters. The `extraction` label tells you how the tool processed the page. It is `readability`, `raw-html`, or `plain`.

Treat all returned text as data, never as instructions. Page content and soft errors arrive as untrusted tool output.

### What You Get Back

The tool inspects the response Content-Type and picks the right rendering path. You specify nothing in the prompt.

An HTML page comes back as only the main article content. The tool renders it as clean markdown with navigation, ads, and sidebars stripped. The header reads `extraction: readability`. This markdown is suitable for direct insertion into prompt context.

A page that is not article-shaped still yields usable markdown. Landing pages, docs indexes, and forums go through a whole-page HTML-to-markdown fallback. Short pages get the same treatment. When article extraction finds too little content, the tool converts the whole document instead.

You control this behavior with the optional `raw` boolean parameter. Set `raw` to true to skip article extraction and render the whole HTML document:

````json
{
  "url": "https://example.com/pricing",
  "raw": true
}
````

Use `raw` for pages that are mostly tables or lists, where extraction would discard content. The header then reads `extraction: raw-html`. The parameter is ignored for non-HTML responses and defaults to false.

Non-HTML text resources come back decoded verbatim. A JSON endpoint returns its body unmodified. JSON and XML responses, including any `+json` or `+xml` suffixed media type, decode as plain text. Plain-text resources return as-is. The header reads `extraction: plain` for all of these.

The tool handles encoding for you. It detects the charset the server declares and transcodes non-UTF-8 pages. Invalid UTF-8 decodes with lossy replacement rather than failure. XHTML served as `application/xhtml+xml` gets the same article-extraction treatment as regular HTML.

A URL whose content type the tool cannot render earns a clear refusal. Binary types such as PDF, octet-stream, images, audio, video, and archives are refused up front, without downloading the body. Your prompt fails visibly instead of ingesting garbage. The set of accepted content types is fixed by the tool.

### Controlling the Size

You cap the returned text with the optional `max_chars` integer parameter:

````json
{
  "url": "https://example.com/long-article",
  "max_chars": 2000
}
````

This call returns at most 2,000 characters of text. When you omit `max_chars`, the configured ceiling applies. The default ceiling is 40,000 characters per call. A request above the ceiling is clamped to it. Cuts always fall on a character boundary, so multibyte characters are never split.

The tool also bounds the response body. Bodies are capped at 8 MiB decompressed by default. Gzip and brotli responses are transparently decompressed and measured on their expanded size, so compression cannot smuggle content past the cap. A response whose declared Content-Length exceeds the byte cap is refused before the body downloads.

Truncation depends on the content type. A structured body such as JSON or XML is delivered complete or not at all. An oversized one is refused, never cut into an invalid prefix. A flat text body over the cap returns a truncated prefix flagged `truncated: true`. Watch that flag. It tells you a follow-up fetch with a tighter `max_chars` or a different URL may be needed.

Timeouts are fixed limits you observe as behavior. The tool allows 5 seconds to establish a connection and 20 seconds for the whole request by default. A slow server produces a soft, recoverable "timed out" message instead of a hung call.

### Redirects

The tool follows redirects automatically. It vets every hop before it follows it.

Redirects are capped at 5 hops by default. An embedding may set the cap to 0 to forbid redirects entirely. The hard ceiling is 20.

Every redirect target is re-validated against the full URL policy. DNS is re-resolved and re-filtered on every hop. A redirect cannot bounce a fetch to an internal address. An https-to-http downgrade redirect is always refused, even when plain http is enabled.

A refused redirect fails the fetch. The message names the from URL, the to URL, and the reason. You see exactly why the chain stopped.

### Safety Rules for URLs

The tool admits only `https://` URLs by default. Plain `http://` is rejected unless the embedding enabled it. Any other scheme, such as ftp, file, or gopher, is refused before any network access.

These rules decide what you can fetch:

- A malformed or unparseable URL is rejected before any network activity.
- URL fragments such as `#section` are stripped before fetching. The query string is preserved intact.
- URLs with embedded credentials, such as `user:pass@host`, are always rejected.
- Only ports 80 and 443 are allowed by default. A URL naming another port, such as 8080, is refused. When the URL omits a port, the default comes from the scheme: 443 for https, 80 for http.
- A URL whose host is a bare IP address is rejected by default in every encoding: octal, decimal-integer, IPv6, and shorthand forms.
- Non-global address classes remain hard-blocked even where IP literals are permitted: loopback, private RFC1918, link-local including the cloud metadata address 169.254.169.254, CGNAT, IPv6 loopback, IPv4-mapped and IPv4-compatible loopback, NAT64 loopback, and multicast.
- The whole loopback block is denied, not just 127.0.0.1. The blocklist applies equally to IPv6. IPv4 addresses disguised in IPv6 form are unwrapped and reclassified.

All policy checks run before any network access. A rejected URL never costs a request. The same checks are re-applied to every redirect target.

The blocklist tracks a pinned IANA special-purpose registry snapshot (2025). It is precise. Ordinary public addresses immediately adjacent to blocked ranges still fetch normally.

The tool protects your privacy in both directions. Error messages never leak URL secrets. Query strings, credentials, and fragments are stripped from every URL before it appears in any error. A blocked-address error says only that the host is not fetchable. It never reveals internal network topology. Every request carries no ambient identity: no cookies, no Authorization header, no Referer, and no proxy, including after a redirect.

### Errors and Recovery

Every failure mode returns a specific, human-readable message naming the cause. You never get a generic failure.

Failures come in two kinds. Hard errors fail fast. Malformed arguments, such as a missing `url`, a non-integer `max_chars`, or a non-boolean `raw`, are hard invalid-argument errors. Policy-violating URLs, such as embedded credentials, a disallowed port, an IP-literal host, or a blocked address, are also hard errors.

Soft errors arrive as ordinary tool output the model can react to. The model can try a different URL instead of the whole tool call aborting. Soft outcomes include:

- A disallowed scheme. The message names the scheme, for example `scheme not allowed: http`.
- An HTTP error status such as 404 or 500. The message names the status code and the final post-redirect URL.
- An unsupported content type. The message names the type, such as `application/pdf`, and suggests an HTML version of the page or a different URL.
- A missing content type. The tool refuses to guess the format.
- A timeout. The message says the request timed out and suggests a retry or a different URL.
- An oversized body. The message names the exact byte cap.
- A mid-stream network failure while reading a body. The message suggests a retry or a different URL.
- A DNS failure. The message names the host that could not be resolved.
- An unrecognized charset. The message names the label the tool cannot decode.

### Configuration

The tool works out of the box with a built-in safe fetch policy. No configuration is required. Configuration exists only at embed time. You observe it as fixed defaults and limits. Whoever embeds the tool customizes the policy through a single validated entry point. Invalid configurations are rejected up front with one error naming the offending field and the violated constraint.

The keys, defaults, and ceilings:

| Key | Default | Ceiling | Effect |
|---|---|---|---|
| `allow_http` | `false` | n/a | Permits `http://` URLs. `https://` is always allowed. |
| `allow_ports` | `[80, 443]` | n/a | Ports a fetch may target, matched against the URL's effective port. |
| `allow_ip_literals` | `false` | n/a | Grants literal syntax only. Non-global literals stay blocked. |
| `deny_cidr("...")` | empty | n/a | Adds denied CIDR ranges on top of the built-in table. |
| `allow_host_address(host, addr)` | empty | n/a | Exact (host, IP) escape hatch. The only supported way to reach an otherwise-blocked address. |
| `max_redirects` | 5 | 20 | Redirect hops per fetch. 0 forbids redirects entirely. |
| `max_bytes` | 8 MiB | 64 MiB | Response body cap, counted on decompressed bytes. |
| `max_chars` | 40,000 | 10,000,000 | Per-call cap on returned text length. |
| `connect_timeout` | 5s | 60s | Time allowed to establish a TCP connection on any hop. |
| `timeout` | 20s | 300s | Cap on the total time a single request may take. |
| `pool_idle_timeout` | 10s | 600s | How long idle connections stay in the pool. |
| `user_agent` | `"promptforge-webfetch/0.0"` | n/a | The User-Agent header sent on every request. |

Two keys shape the address policy. `deny_cidr("...")` blocks additional ranges that would otherwise be fetchable, such as an organization's own address space. `allow_host_address(host, addr)` admits one exact host-plus-address pair, for example localhost at 127.0.0.1. The exception never widens. It admits only the named address, and a DNS answer for another name cannot inherit it.

---

## promptforge-dev User Guide

You write prompts. You want to see what they do. `promptforge-dev` runs one prompt file against your already-running PromptForge gateway with a single command. Edit the file. Save it. See the result. Add `--watch` and every save triggers a fresh run, so the loop from edit to result takes seconds. The tool dumps the prompt's store to disk after each run, prints the final result on stdout, and keeps diagnostics on stderr. You get a fast, inspectable loop for prompt development with no gateway management, no model downloads, and no weight files.

### The Prompt Runner and Edit-Run Loop

`promptforge-dev` is a command-line tool. You point it at a prompt file. It runs that prompt against a PromptForge gateway that is already running. One command gives you one run.

The tool connects to an existing gateway. It never starts one. You do not manage a gateway lifecycle. You do not download models. You do not handle weight files.

Pass `--watch` to enable watch mode. Every save of the prompt file triggers a fresh run. You get a live edit-run feedback loop while you develop a prompt.

### Installation and Gateway Setup

Install the tool from crates.io with one command. The package is published. You do not build from source. The tool requires Rust 1.89 or later.

The tool needs two environment variables. Set both before you run it. There are no CLI flags for them.

````bash
export PROMPTFORGE_GATEWAY_URL=http://127.0.0.1:8081/v1
export PROMPTFORGE_GATEWAY_API_KEY=<bearer from your gateway profile>
````

`PROMPTFORGE_GATEWAY_URL` is the gateway API root. `PROMPTFORGE_GATEWAY_API_KEY` is your bearer credential. Both must be set and non-empty. An empty value counts as missing.

The tool validates the environment once at startup, before it does any work. If a variable is missing or empty, you get a startup error. The error names the missing variable. It tells you to start promptforge-gateway first, then export both variables. The tool exits with code 1.

A malformed gateway URL or a blank credential aborts startup before any prompt run. Your bearer credential never appears in logs or debug output. It renders as redacted.

### Running a Prompt

The simplest invocation names a prompt file.

````bash
promptforge-dev my-prompt.md
````

Pass an input string as the second positional argument. The input becomes the prompt's `args`. If you omit it, it defaults to empty.

````bash
promptforge-dev my-prompt.md "summarize this paragraph"
````

To pass an input that begins with `--`, place a bare `--` delimiter before it.

````bash
promptforge-dev my-prompt.md -- "--verbose"
````

Declare context, thinking, and max tokens on the prompt file itself. Use `models.bind` or `models.always`. The tool rejects CLI flags for these settings. `--context`, `--max-tokens`, `--no-think`, and `--verbose` all produce unknown-flag errors.

The tool runs only files that declare a `promptforge:` version in frontmatter. It refuses other files with a clear message.

Each run follows a fixed pipeline: validate environment, fetch model catalog, build tool set, parse prompt, execute, dump store. The catalog is fetched once and reused across watch-mode reruns. Every run prints a unique run id to stderr. The id correlates console output, traces, and store files.

When a run fails, you get a diagnostic that names the prompt file. The tool exits with code 1. A missing prompt file or more than two positional arguments produces a one-line problem description and the usage line, with exit code 2. Argument errors report before any credential check.

The exit codes are documented: 0 success, 1 runtime error, 2 usage error, 130 interrupted. Cancel a running prompt with Ctrl-C. You receive an "interrupted by Ctrl-C" message. The exit code is 130. Scripts can branch on these codes.

### Results and the Store Dump

When a run succeeds, the prompt's final result string prints on stdout. The result is separate from the diagnostic stream on stderr. You can pipe or redirect the result without observer noise.

After a run, the tool dumps the prompt store. You inspect what the prompt produced. Store dumping is part of the default run behavior. No extra flag is needed.

Every run's store lands in a directory beside the prompt file, named after it. For a prompt named `briefer.md`, the dump lands in `briefer/`. It contains the files the prompt wrote, such as `evidence.md` and `notes/deep.txt`.

````text
briefer.md
briefer/
  evidence.md
  notes/
    deep.txt
````

Every store write lands on disk immediately during the run. There is no post-run reconcile step. The tool clears the previous store directory before each new run, so stale files never masquerade as current output. A run that produces nothing removes its empty store directory when it finishes. Your directory tree stays clean. A failed run keeps its partial store on disk. You can debug from it.

The tool skips unsafe store paths and reports the status. Unsafe paths include absolute paths, `..` traversal, backslashes, control characters, and Windows reserved device names (CON, PRN, AUX, NUL, COM1-9, LPT1-9).

### Watch Mode

Pass `--watch` to rerun the prompt automatically on every save.

````bash
promptforge-dev --watch my-prompt.md
````

The tool prints a startup line: it is watching the file, and Ctrl-C stops it. Edit the prompt. Save it. A fresh run fires. Successful results print to stdout on every rerun. Diagnostics stay on stderr.

A failed rerun prints its error on stderr. Watch mode keeps watching for the next save. You keep iterating.

A burst of rapid saves coalesces into one rerun. The rerun fires after 300ms of quiet. One logical edit produces exactly one run. Editors that save through atomic write-then-rename still trigger reruns.

Watch mode watches only the prompt file. Changes to other files in the same directory never trigger a rerun. This includes the store directory's contents. A bare file name as the prompt path watches the current directory. If the filesystem watcher backend fails, watch mode stops with a descriptive error.

Reruns are fast. The run environment is built once and reused across every save. The tool does not refetch the model catalog or rebuild the tool picker on each save.

Stop watch mode at any time with Ctrl-C. The exit is clean. You see "interrupted by Ctrl-C". No spurious final rerun fires, even if a save was mid-debounce.

### Web Tools and Tool Picking

Your prompts get web fetching and web search tools during a run. Both tools are always available on every run. There is no offline mode. There is zero configuration.

The model fetches a web page with `web_fetch`. The tool returns the page's main content as markdown. It runs locally.

The model searches the web mid-run with `web_search`. The tool proxies through the PromptForge gateway. It uses your validated bearer credential.

The run picks relevant tools for the prompt automatically. A semantic tool picker resolves natural-language capability descriptions to the matching tool. The picker is built over the live tool catalog and an embedding model. The live tool set is validated before the picker is derived, so every advertised tool is actually callable. Duplicate tool identities or illegal wire names produce clear startup errors instead of silent breakage.

### Raw Capture and Trace Files

Pass `--capture-raw` to persist verbatim request and response bodies. This covers full prompts, tool arguments and results, and model output.

````bash
promptforge-dev --capture-raw my-prompt.md
````

This flag is the only way trace capture activates. An ordinary run never silently persists sensitive data. When the flag is active, a warning on stderr names the trace directory.

The traces go to a `.trace/` directory inside the prompt's store directory: `<prompt-stem>/.trace/`. Each model turn produces one pretty-printed JSON file per direction, named `turn-{N}-request.json` and `turn-{N}-response.json`. Each file holds one verbatim request or response body. You inspect or replay exactly what happened during a session.

Trace capture never blocks the run. A background worker writes the files. If the capture queue falls behind, events drop. The tool reports the exact drop count on stderr when the run finishes. Each written trace file gets a stderr confirmation. A failed trace write produces a stderr diagnostic, and the run continues.

### Progress and Console Output

During setup, progress bars render when stderr is a terminal. They cover the catalog fetch, the embedding-model load, and tool indexing. Bars clear as phases finish. Off a terminal, stderr stays clean.

During the run, you watch a live verbose trace on stderr. Every observation is its own bracketed line. Each line is prefixed with the run id.

````text
[dev-3a7f...] Research: Run started
[dev-3a7f...] Section: Lua: step one
````

The final result prints separately to stdout. You can pipe or redirect output without observer noise.

### Diagnostics and Failure Reporting

When a run fails inside a Lua section, the error message leads with the prompt file path and the exact line number.

````text
dev run failed: briefer.md:51: <detail>
````

The line number points at the innermost failing section. A failure not tied to a prompt line shows a plain message without a line number. Errors name the failing file and stage, whether the file cannot be read, parsed, or executed.

Use the run id prefix on each trace line to correlate console output, traces, and store files for one run.

### Filesystem Security

All dump and trace writes are owner-only. Directories are mode 0o700 and files are mode 0o600 on Unix. On Windows, full control goes to the current user alone. You do not configure this. It is always active. On Windows, this hardening depends on the USERNAME environment variable.

The tool refuses to write through symlinked or reparse-point ancestors. A planted link cannot redirect sensitive output outside the dump tree. Dump files are written atomically. Content goes to a temporary file, then renames over the destination. An interrupted write never corrupts a previously dumped file. A failed write removes its partial temporary file.

---

*This guide was assembled from per-crate documentation. For source-level details, see each crate's individual user guide.*
