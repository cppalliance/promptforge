# Gateway Configuration

One file runs the whole gateway. `gateway.toml` holds your bind address, your credentials, your complete model catalog, and every deployment profile. Edit the file, and the gateway parses it, expands environment variables, and validates every rule before anything runs. A file that fails any check never starts a gateway. This guide teaches you the full schema, section by section, with working examples you can copy and edit. Operators normally use the Config UI to edit configuration. This guide is for advanced users who edit `gateway.toml` directly. The canonical commented example is `gateway.local.example.toml` at the workspace root. It is guaranteed to parse and validate against the current schema.

## One File, One Version

You pin the on-disk schema by writing `config-version = 2` at the top of the file. Version 2 is the profile-and-STT layout. Any other value, or a missing version, fails with migration guidance.

One file owns everything. Global settings live in `[server]`, `[workshop]`, `[local]`, and `[tools]`. Remote providers live in `[[endpoint]]`. Compute pools live in `[[dominion]]`. The model catalog lives in `[[model]]`, `[[local_model]]`, and `[[stt_model]]`. Deployment variants live in `[[profile]]`. Keep sections in this canonical order. The Config UI writes this order, so equivalent edits stay merge-friendly.

The active profile is state, not config. It lives in a sibling file named `<config-stem>.state.toml` with a single key, `active_profile = "name"`. You switch profiles without editing `gateway.toml`, and the gateway remembers the selection across restarts.

## The Core Sections

`[server]` sets the socket address the gateway binds to and the shared bearer key every `/v1/*` request must present. `[workshop]` runs an embedded workshop UI on its own bind address. `[local]` chooses the cache directory for downloaded models and the pinned llama.cpp install. `[tools]` holds optional built-in tools such as web search.

The smallest valid config needs only the version, the bind address, and the API key:

````toml
config-version = 2

[server]
bind = "127.0.0.1:8080"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
````

Everything else has defaults or is optional. The API key must not be empty. The gateway never starts without authentication.

## Validation at Load

Validation is upfront and total. Load reads the file, expands variables, and checks every rule before any profile runs. Every profile is validated, not only the active one. This includes VRAM budgets and STT role pairing. A broken inactive profile cannot slip through.

Misspelled or unknown keys are rejected everywhere, so typos fail fast at load. Removed v1 constructs are hard-break errors that name the file, the key, and the exact line. A configuration that fails any check never produces a running gateway.

## Server, Secrets, and Interpolation

You set the bind address and API key in `[server]`:

````toml
[server]
bind = "127.0.0.1:8080"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"
````

You reference environment variables in any string value with `${VAR}` syntax, including strings nested inside arrays and tables. Keep secrets and host-specific settings out of the file this way. Write `$$` for a literal dollar sign. An unset variable is a startup error that names the variable. An unclosed `${...` is a distinct malformed-interpolation error. Text in comments and keys is never expanded.

Secrets are safe by construction. API keys render as `redacted` in logs and debug output. They serialize as `"***"` in exported JSON. Error messages may name fields, model names, and sources, but never render a secret. When you edit a config that shows `"***"`, leave the marker in place. The real secret carries over on save. Secrets match entries by name or id, so reordering models or endpoints does not lose them.

If you bind `0.0.0.0` or `::`, same-host clients such as the workshop use the matching loopback URL automatically. You configure no separate client address.

## Endpoints and Remote Models

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

## Local Models and Companions

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

## Speech-to-Text Models

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

## Dominions and VRAM Budgeting

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

## Profiles and Startup Selection

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

## Loading, JSON, and Pending Edits

Loading reads the single file, expands variables, validates everything, and activates the selected profile in one step. Tooling can validate a TOML document in memory without touching disk. Tooling can switch the active profile on an already-loaded configuration without re-reading anything.

You export the resolved configuration as JSON for inspection or piping into other tooling. Every secret field redacts to `"***"`. The JSON uses the same key names as the TOML.

Admin edits are staged as pending shadow files: `gateway.toml.next` and `gateway.state.toml.next`. No save touches the live config until the staged edit is explicitly promoted. Promotion is atomic, with automatic backup-and-restore if the replacement fails midway. A staged edit is fully validated against the real schema and profile definitions before it reaches disk. A bad edit leaves no shadow behind.

You preview a pending edit exactly as it would run. Command-line and environment profile overrides still win. A pending-changes report lists which files have staged edits and which top-level sections will change, including a pending profile switch. A single admin document can carry both the global config and an `active_profile` choice. The profile choice is split out into the pending state file automatically, preserving the config/state boundary. A profile still recorded as active cannot be deleted in a pending edit.

## Built-In Tools: Web Search and Workshop

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

[workshop.tape]
path = "tape.jsonl"
````

The workshop binds `127.0.0.1:7910` by default. Set `open_browser = true` to open the system browser once the UI is serving. The workshop derives its gateway connection from `[server]`: same address, same API key. No credential is duplicated and none can drift.

`[workshop.stt]` tunes live capture. `window_seconds` sets the seconds of trailing audio per interim pass (default 15). `interval_ms` sets the milliseconds between passes (default 500). `vocabulary` lists domain terms the transcriber is biased toward; an empty list disables biasing. `[workshop.tape]` enables session recording to a JSONL tape file (default `tape.jsonl`). Relative paths resolve from the config file's directory, never the process's current directory.

## Errors and Migration

Every load failure classifies into one of seven stable categories: the file could not be read, the TOML did not parse, an interpolation was malformed, an environment variable was unset, a semantic check failed, a removed layout feature was found, or a shadow file could not be written. Tooling reacts to the category without parsing error text.

Removed v1 constructs are hard-break errors, not silent ignores. Each diagnostic names the file, the removed key, the exact line, and the replacement layout:

- `include` key: use one `gateway.toml` with `[[profile]]` checklist entries.
- A sibling `profiles/` directory: move every profile into this file as a `[[profile]]` checklist.
- A top-level `models` allowlist: move the checklist into a `[[profile]]` `models` key.
- `[workshop.voice]` model keys: use `[workshop.stt]` tuning and a global `[[stt_model]]` entry.
- A missing or wrong `config-version`: set `config-version = 2`.

Legacy `[queue]` and `[[device]]` sections, and per-endpoint or per-model device keys, fail as parse errors. Queue and device settings moved into `[[dominion]]`.
