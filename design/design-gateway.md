# `promptforge-gateway`: a service that holds every backend credential and routes OpenAI-shaped requests onto one of them

## Executive summary

This crate is one always-on service process. It accepts OpenAI-shaped chat completion requests over HTTP, resolves the request's model name to a configured backend, holds the credential that backend needs, forwards the request, and relays the answer back. Nothing above it holds a vendor key, and nothing above it knows which machine answers: a prompt names `reasoning-large`, and whether that is an Anthropic key or a pod on the intranet is a line in one configuration file.

`POST /v1/chat/completions` is the only route that reaches a backend. `POST /v1/tools/web_search` is a built-in tool the gateway answers itself, for the same reason it holds the model credential. `GET /v1/models` is the catalog. Admin routes under `/admin/*` list and switch profiles. `GET /health` is a liveness probe and takes no token; every other route checks a bearer against one shared secret in constant time.

Routing is one exact string match with a 404 on a miss - no prefixes, no aliases, no default model - and the request body is passed through with only `model` substituted, so a sampling parameter the gateway has never heard of reaches the backend anyway and the caller's model name comes back rather than the vendor's. Configuration is one TOML file that rejects an unknown key outright and expands `${VAR}` from the environment before it parses, so a deployment that forgot to export a credential fails at startup rather than serving with a blank one.

What is not here is as load-bearing as what is: no streaming, no retries, no token budget, and no knowledge of prompts or runs. Per-endpoint concurrency limits and a fair waiting queue (`[queue]` / `[[endpoint]].concurrency`) are implemented; a full queue answers 503. Local generative models are served by a gateway-owned `llama-server` subprocess (not in-process FFI). The one process every LLM consumer depends on is deliberately the simplest one in the system.

## The key design choices

1. **Every backend credential lives in this process and nowhere above it.** A prompt names `reasoning-large`; whether that resolves to an Anthropic key or a pod on the intranet is a line in one configuration file that only this process reads. Nothing above the gateway holds a vendor key or knows which machine answers, so a key rotation touches one file on one host rather than every consumer. Reversing this means putting keys back into the executor and into Talktron, which is a change to two deployment stories and a widening of the surface a leak can come from.

2. **The request body is passed through with only `model` substituted.** `ChatRequest` and `ChatResponse` name the two fields the gateway routes on and carry everything else in a flattened map, so a sampling parameter the gateway has never heard of reaches the backend without a gateway release, and the caller's own model name comes back rather than the vendor's. The alternative was patching raw bytes by string surgery: cheaper, but it forecloses protocol translation and blurs the 400 boundary, since a malformed body would surface as an opaque upstream 4xx. Tension: a misspelled field passes through and produces the backend's error rather than ours.

3. **Model resolution is one exact string lookup, and a miss is a 404.** No prefix matching, no regex, no alias chain, and no default model, because every one of those turns a typo into a silent charge against the wrong backend. A model name is any string the deployment chooses, and naming it by capability rather than by vendor is the mechanism that lets one prompt stay fixed while the machine under it changes between development and production.

4. **A model resolves to exactly one endpoint, fixed for the active profile.** `from_config` takes the first id in the model's `endpoints` list and the rest are parsed and ignored. The table is held behind a `tokio::sync::RwLock` so `POST /admin/switch-profile` can replace routing and local children without restarting the listener. Selection among endpoints and multi-endpoint health are deliberately unbuilt. A dead local `llama-server` child is supervised on send (same-port respawn), which is separate from endpoint selection.

5. **The gateway answers one tool itself, for the same reason it holds the model credential.** `POST /v1/tools/web_search` is proxied to the search provider with the provider's key, and the executor never sees that key. `web_fetch` needs no credential and so stays in the executor with no route here. The rule is the credential and not the category of work, which means there is one place to look for a secret rather than two.

6. **The search request is closed where chat is open, and a result is four fields.** Chat is a passthrough of somebody else's evolving specification, while the search shape is the gateway's own, so an unknown key there is a caller's mistake and is rejected. The provider returns thumbnails, profiles, ranking metadata, and nested clusters, and every byte would land in a model's context window at the caller's expense, so title, URL, description, and age are what a model deciding what to read next actually needs. `count` is clamped rather than rejected, since breadth is a preference and not a contract. The tool's off switch is the absence of its configuration table, which answers 404 rather than 501: this deployment lacks a resource, it is not failing to implement a capability.

7. **Every configuration struct is `deny_unknown_fields`, so a misspelled key is a boot failure.** A setting silently ignored is worse than a process that refuses to start, because the first is discovered in production behaviour and the second on the deploy. Tension: adding a field is then a breaking change for anyone who set an unrecognised one early.

8. **`${VAR}` expands over the file's whole text at load time, and it is the only environment mechanism.** The pass runs before the TOML is parsed, so it applies to any string value and an unresolved variable is reported as a missing environment variable rather than as a type error somewhere further in. There is no implicit pickup of `ANTHROPIC_API_KEY` or `OPENAI_API_KEY`, because implicit pickup makes a deployment's effective credential invisible in its configuration and turns a leftover shell export into a live production key. Tension: a container already passing secrets as environment variables still has to write the one-line reference.

9. **`Secret` cannot be serialized and redacts through both `Debug` and `Display`.** With no `Serialize` it cannot reach a metrics label or a JSON error body by accident, and with both formatting traits redacting a `tracing` field or a `{:?}` on a configuration struct is safe by default rather than by discipline. `expose` is the single accessor, called when building an auth header, so every place a key becomes plaintext is one grep away.

10. **Every error body is the OpenAI error envelope, with `type` and `code` fixed per variant.** An unmodified SDK surfaces the gateway's refusals as its own error type rather than as an unparseable blob, and `UnknownModel` reports `model_not_found`, OpenAI's own code, so a client written against OpenAI already handles it. `type` collapses onto three values because it is the coarse class an SDK groups on rather than an identifier. Tension: `UpstreamStatus` is the only variant whose status is not fixed by the variant, so a client matching on status alone cannot tell the gateway refusing from the backend refusing.

11. **`Upstream` is the seam for per-vendor translation, and it has one method.** Remote endpoints use `OpenAiUpstream`; local `[[local_model]]` endpoints use `LocalUpstream` (OpenAI chat shape plus lazy `llama-server` respawn). Its shape is whole-response only rather than the `build`/`whole`/`stream` triple a fuller design would carry, because nothing streams yet and a three-method trait with two unimplemented methods would be a seam pretending to be a mechanism. `Protocol` has a single variant for the same reason: a second wire protocol is a new variant and a code change, not a configuration line somebody can set today and get a panic from.

12. **`GET /health` takes no token.** A service supervisor's liveness probe should not need a secret to learn whether the process is up, and what the route reveals is only that it is serving. The two `/v1` routes check `Authorization: Bearer` in constant time and answer a missing or wrong token with a 401 carrying no detail.

13. **That bearer token is the same shared secret the MCP server checks.** This is settled rather than assumed: the gateway sits behind the production firewall, so its check is defence in depth and not a separate trust boundary, and a second string to rotate would buy nothing while adding a way to be half-configured. Tension: one leaked string reaches both the prompt surface and the model credentials.

14. **Local generative inference is a managed `llama-server` subprocess, not in-process `llama-cpp-2`.** Linking llama.cpp into the gateway binary is deferred. When `[[local_model]]` is present, startup downloads a pinned b10082 `llama-server` (GPU builds: Vulkan on Windows/Linux, Metal on macOS), downloads each GGUF into `~/.promptforge` (or `[local].cache_dir`), spawns one child per local model, and registers each as a normal routed `Model` whose `base_url` is the child's `http://127.0.0.1:{port}/v1`. After readiness, `LocalUpstream` owns the child: on chat send, a transport failure against a dead child triggers one same-port / same-alias / same-api-key respawn and one request retry (with a short cooldown). There is no background watchdog. `GET /health` remains process liveness only. Operator experience matches the plan: one `gateway.toml`, no separate Ollama install. `unsafe_code` stays forbidden. Falsifier for revisiting in-process FFI: if subprocess IPC or lifecycle (startup races, clean shutdown, multi-model VRAM contention) proves inadequate for production workloads.

## Profiles (named configs with include and hot switch)

A profile is a TOML file describing the active model catalog, endpoints, optional devices/lanes, and local models. The gateway runs one profile at a time.

```
promptforge-gateway serve --profile analytical
promptforge-gateway serve --profiles-dir ./profiles --profile base
promptforge-gateway serve gateway.toml   # path still works; includes resolve relative to the file
```

Default profiles directory: `~/.promptforge/profiles/` (Windows: `%USERPROFILE%\.promptforge\profiles\`). Example profiles live in the repo under `profiles/` (`base.toml`, `analytical.toml`).

Inheritance:

```toml
include = ["base.toml"]
```

- Paths are relative to the including file
- Resolution is depth-first; max depth 16; cycles are `ConfigError`
- Arrays (`endpoint` / `model` / `local_model` / `device`) merge by append; same `id` or `name` is replaced by the later (child) definition
- A leaf file with only `[[device.lane]]` (no `[[device]]` in that file) parses as a table; those lanes attach to the parent device named by `lane.device`
- Merge type errors include `path:line` when the header can be located in the overlay file
- Scalars (`server.*`, `queue.*`, `local.cache_dir`) - later wins

Admin routes (same bearer token as `/v1`):

| Route | Behaviour |
|---|---|
| `GET /admin/profiles` | List `*.toml` stems in the profiles directory |
| `GET /admin/status` | Current profile name, model names, local child count, queue note |
| `POST /admin/switch-profile` `{"name":"threat"}` | Immediate switch: under the write lock replace local with empty and install remote-only routing from the new profile, drop the old `LocalRuntime` (kills previous llama-server processes), then `spawn_blocking` `LocalRuntime::start` and install the result. On start failure leave empty local models and the remote-only live state, return `SwitchFailed`. No queue drain. In-flight chat may fail. |

`AppState` holds routing, token, web-search, and `LocalRuntime` behind `tokio::sync::RwLock` so switch can rebuild without restarting the HTTP listener. Bind address does not change on switch (restart required).

Optional devices/lanes (Layer 3):

```toml
[[device]]
id = "anthropic"
type = "remote"
concurrency = 10

[[device]]
id = "local-gpu"
type = "local"

[[device.lane]]
device = "local-gpu"
id = "generative"
concurrency = 1

[[endpoint]]
id = "anthropic"
device = "anthropic"   # or keep endpoint-level concurrency =
# ...

[[local_model]]
name = "qwen-local"
device = "local-gpu"
lane = "generative"
# ...
```

Remote endpoints may still set `concurrency` directly (Layer 1). Local models default to concurrency 1 when device/lane are omitted. For a local model, that resolved lane concurrency is one knob: it is both the gateway admit limit and `llama-server --parallel`.

## Local generative models (`[[local_model]]`)

```toml
[local]
# optional; default ~/.promptforge
# cache_dir = "~/.promptforge"

[[local_model]]
name = "qwen-local"
description = "A careful analysis model suited to structured reasoning and long-context review"
source = "https://huggingface.co/.../model.gguf"
sha256 = "..."          # optional pin
context = 65536
thinking = "never"
gpu_layers = 99
flash_attention = true
cache_type_k = "q8_0"
cache_type_v = "q4_0"
n_predict = 8192
chat_template_file = "..."  # optional; passed as --chat-template-file when GGUF template lacks tools
```

On start (and after switch-profile), each local model becomes a catalog entry: `description` appears in `GET /v1/models` so live H1 semantic resolution works the same as for remote models. Each local endpoint uses `LocalUpstream`, which owns the child and respawns it on send after a post-ready death so catalog `upstream_name` and port stay stable. Dropping `LocalRuntime` (process exit or profile replace) drops those upstreams and kills every child.

After a local child is healthy, the gateway GETs llama `/props`, builds `DialectEvidence`, and resolves a `tool_dialect` through `promptforge-core`'s `ToolDialectRegistry` (hard-fail on none or tie). The catalog advertises `tool_dialect` and `tools_mode` (`native` or `emulated`). Remote OpenAI-compat models default to `openai` / `native`. When `/props` omits `chat_template`, a sibling `<stem>.md` beside the GGUF (written at HF provision with frontmatter plus a fenced Jinja `chat_template`) supplies fallback evidence; live props always win over a conflicting sidecar.

First-time GGUF (and other blob) downloads show an indicatif progress bar on an interactive stderr TTY - percent, bytes, rate, and ETA. When stderr is not a TTY, the same download emits periodic `tracing` progress lines instead.

See `gateway.local.example.toml` at the repository root for a full sample pinned to the same Qwen3.5-9B Q4_K_M digest as `promptforge-core-tests`.

## The boundary is one process wide, and everything above it is a client

The crate also answers one built-in tool route of its own. It is the box labelled `promptforge-gateway` in the system diagram, and it is the only box with an edge to an LLM backend.

It is the single process that talks to LLM backends. Tension: one point of failure for every LLM consumer at once.

What it does not do:

- Expose an MCP surface of any kind. Not a tool, not a prompt, not a resource. MCP exists to cross a process boundary and the gateway's work crosses none.
- Read a prompt, parse markdown, resolve a slot, evaluate Lua, or know what a prompt is. It receives an assembled message array.
- Touch storage. No database, no filesystem write. Logging is `tracing_subscriber`'s formatter on stdout; the rolling log file in the unbuilt design is not here.
- Know what a run is. Beyond `Authorization`, only `X-PromptForge-Client` is read (for fair queue scheduling; absent means `"default"`).
- Authenticate per client. Fair scheduling attributes waiting slots by the optional client header, not by a login identity.
- Retry a failed upstream request, or fail over mid-response.
- Inspect, cache, or log message content. It rewrites the `model` field and nothing else.
- Count tokens, enforce a token budget, or truncate a context.
- Decide which model a prompt uses. A slot resolves to a model name in `prompts.toml`; this crate only maps that name onto a backend.

What it does hold, against the boundary the earlier design drew: a non-LLM credential. `[tools.web_search]` carries the search provider's key, the gateway sends it to the provider, and the executor never sees it. That is the same argument the LLM credential is held on rather than an exception to it, and it means there is one place to look for a secret rather than two.

The test of the boundary: a process with no relation to PromptForge - Talktron, a shell script with curl, any unmodified OpenAI SDK - gets correct routing and credentials by changing one base URL, and never learns that prompts exist.

## The wire shape is defined twice on purpose, against a public specification

A binary crate with a thin library target, so integration tests and the fake backend link the same code the service runs. Types below are `promptforge_gateway::Type`.

The wire structs are defined here and defined again in the executor crate, `promptforge-core`, which owns `GatewayClient`. That duplication is deliberate. The authority for the schema is the OpenAI chat completions specification, not a Rust struct: Talktron reaches this service through the Python SDK and a shared crate would not help it. Two independent definitions against a public contract also mean a change on one side cannot silently pass a type check on the other. Tension: two definitions of one shape to keep in step, which is what the end-to-end test driving this service with the core's client exists to catch.

## Three routes, and only the liveness probe is unauthenticated

Three routes. `axum` on `hyper`.

| Route | Auth | Purpose |
|---|---|---|
| `POST /v1/chat/completions` | yes | The only route that reaches a backend |
| `POST /v1/tools/web_search` | yes | The built-in search tool, proxied to the provider |
| `GET /health` | no | Liveness for a service supervisor |

`/health` takes no token because a supervisor probe should not need a secret, and answers `{"status":"serving"}` whenever the process is serving. `/v1/*` checks `Authorization: Bearer` against `server.api_key` in constant time, and a missing or wrong key is 401 with no detail. That key is the same shared secret the MCP server checks, settled rather than assumed: the gateway's check is defence in depth behind the production firewall, not a separate trust boundary, so a second string to rotate would buy nothing and add a way to be half-configured. Tension: one leaked string reaches both the prompt surface and the model credentials.

### `POST /v1/chat/completions`

The gateway names only the two fields it routes on and carries everything else in a flattened map.

```rust
#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Value>,
    /// Every field the gateway does not name, preserved verbatim.
    #[serde(flatten)] pub rest: Map<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[non_exhaustive]
pub struct ChatResponse {
    /// Rewritten to the name the caller asked for, never the upstream name.
    pub model: String,
    pub choices: Vec<Value>,
    #[serde(flatten)] pub rest: Map<String, Value>,
}
```

The gateway names only the fields it needs and carries the rest in a flattened map, so a new upstream sampling parameter reaches the backend without a gateway release. The rejected alternative was forwarding raw bytes with `model` patched by string surgery: cheaper, but it forecloses protocol translation and makes the 400 boundary fuzzy, since a malformed body would then surface as an opaque upstream 4xx. Tension: a misspelled field passes through and produces the backend's error rather than ours.

Messages and choices are `Value` rather than typed structures, which carries the passthrough rule as far as it goes: nothing inside a message or a choice is named here, so nothing inside one can fail to deserialize.

`model` on the way out is the caller's name, so a client that echoes it back into its next request gets a string that still routes.

### `POST /v1/tools/web_search`

The gateway answers one tool itself, for the reason it holds the LLM credential: it is the process that holds keys, so a tool needing a provider key is a tool the gateway answers. The executor calls this route instead of calling a search provider, and no search key ever reaches it. `web_fetch`, which needs no credential, stays in the executor and has no route here.

The route is bearer-authed on the same token as `/v1/chat/completions`. Its request is closed rather than flattened, which is the opposite choice from the chat route and made for the opposite reason: chat is a passthrough of somebody else's evolving specification, while this shape is the gateway's own, so an unknown key is a caller's mistake and is rejected.

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebSearchRequest {
    pub query: String,
    /// Defaults to `settings.default_count`; clamped to `1..=settings.max_count`.
    #[serde(default)] pub count: Option<u8>,
    #[serde(default)] pub freshness: Option<String>,
    #[serde(default)] pub country: Option<String>,
    #[serde(default)] pub search_lang: Option<String>,
    #[serde(default)] pub safesearch: Option<String>,
    #[serde(default)] pub include_domains: Vec<String>,
    #[serde(default)] pub exclude_domains: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct WebSearchResponse {
    /// Trimmed request query, always present.
    pub query: String,
    pub results: Vec<SearchResult>,
}

#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub age: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub site_name: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")] pub extra_snippets: Vec<String>,
}
```

`query` is trimmed of ASCII whitespace; empty after trim is `MalformedRequest("web_search: empty query")` (HTTP 400) and Brave is not called. Success with zero hits is still 200 with `{"query":"...","results":[]}`.

The consumer of this JSON is a language model deciding what to read next. Each hit is trimmed to title, URL, description, optional age, optional `site_name` (hostname when parseable), and optional `extra_snippets`. Provider extras (thumbnails, ranking metadata) are dropped so they do not inflate context. `age` and empty `extra_snippets` are omitted rather than null.

`count` is clamped rather than rejected: floor 1, ceiling `max_count` (default 20), default `default_count` (default 10) when absent. Optional knobs (`freshness`, `country`, `search_lang`, `safesearch`) pass through to Brave when non-empty; config `default_freshness` / `default_safesearch` apply when the request omits them and those defaults are non-empty. `include_domains` / `exclude_domains` filter after Brave (empty or absent means no filter).

Post-process order is deterministic: map hits (including `extra_snippets`), sanitize title/description (control chars, whitespace collapse, a small HTML-entity set, length caps), optionally strip tracking query params (`utm_*`, `fbclid`, `gclid`, `mc_cid`, `mc_eid`), set `site_name`, include then exclude domain filters, then diversify by hostname (`max_per_host`, default 2) while walking Brave order until `count` is filled.

The provider is reached as `GET {base_url}/web/search` with `q`, over-fetched `count` (`min(max_count, requested.saturating_mul(3).max(requested))`), `extra_snippets=true`, optional knobs, and the credential in `X-Subscription-Token`. Only `web.results` is read; missing `web` yields an empty list. Upstream failures keep existing transport/status mapping, with messages prefixed `web_search: `.

Configuration is one optional table, and its absence is the tool's off switch:

| Key | Required | Default | Meaning |
|---|---|---|---|
| `provider` | yes | - | The search provider. `brave` is the only value the enum has. |
| `api_key` | yes | - | The provider credential, a `Secret`, usually `${BRAVE_API_KEY}`. |
| `base_url` | no | `https://api.search.brave.com/res/v1` | Where the provider lives. Overridden to point at a proxy or a fake. |
| `default_count` | no | `10` | Used when the request omits `count`. |
| `max_count` | no | `20` | Clamp and over-fetch ceiling. |
| `max_per_host` | no | `2` | Diversity cap per hostname group. |
| `default_freshness` | no | `""` | Applied when the request omits freshness and this is non-empty. |
| `default_safesearch` | no | `""` | Applied when the request omits safesearch and this is non-empty. |
| `strip_tracking` | no | `true` | Scrub tracking query params from result URLs. |

A request when no `[tools.web_search]` section was configured is `ToolNotConfigured`, which renders as 404. A 404 rather than a 501 is the deliberate reading: the route is not a capability the gateway is failing to implement, it is a resource this deployment does not have, and a deployment without a search key is an ordinary deployment rather than a broken one. Nothing else is affected by its absence.

## Routing is one exact string match, and a miss is a 404

Resolution is one exact string lookup. The request's `model` is matched against the `name` of a `[[model]]` entry; a miss is 404. There is no prefix matching, no regex, no alias chain, and no default model, because every one of those turns a typo into a silent charge against the wrong backend.

A model name is any string the deployment chooses. Naming it after a vendor model, as the shipped `gateway.toml` does with `claude-sonnet-4-6`, is right when both environments genuinely reach that vendor. Naming it by capability, `reasoning-large`, is what lets the same name resolve to an Anthropic key in development and a pair of RunPod pods in production, and that indirection is the mechanism that makes prompts deployment-agnostic. All endpoints of one model serve it under one `upstream` string; a provider that spells the model differently gets its own model entry.

```rust
pub struct Endpoint {
    pub id: String,
    pub upstream: Arc<dyn Upstream>,
}

pub struct Model {
    pub name: String,
    /// The string the backend knows this model by. Substituted into the outgoing body.
    pub upstream_name: String,
    /// The endpoint serving this model: the first of the configured list.
    pub endpoint: Arc<Endpoint>,
}

pub struct Routing {
    models: HashMap<String, Arc<Model>>,
}

impl Routing {
    pub fn new(models: HashMap<String, Arc<Model>>) -> Routing;
    pub fn from_config(config: &Config) -> Result<Routing, ConfigError>;
    pub fn model(&self, name: &str) -> Result<Arc<Model>, GatewayError>;
}
```

A model resolves to exactly one endpoint, fixed at load: `from_config` takes the first id in the model's `endpoints` list and the rest are parsed and ignored. Selection among endpoints, endpoint health, and the `EndpointId` newtype are unbuilt design. The table is built once and held behind an `Arc`; it does not change while the process runs.

### One wire protocol

An endpoint declares `protocol = "openai"`, which is the only variant the enum has. A second protocol is a new variant and a code change, not a configuration line.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol { Openai }

#[async_trait]
pub trait Upstream: Send + Sync {
    /// Forward `req`, substituting `upstream_model` for the caller's model name.
    async fn send(&self, req: ChatRequest, upstream_model: &str)
        -> Result<ChatResponse, GatewayError>;
}
```

`OpenAiUpstream` substitutes `upstream` for `model`, sets `Authorization: Bearer` unless the endpoint's key is empty, posts to `{base_url}/chat/completions`, and restores the caller's model name on the way back. An empty `api_key` sends no authorization header at all, which is what a loopback pod wants.

`LocalUpstream` is the second implementation: same OpenAI chat shape against a loopback `llama-server`, plus lazy child supervision (detect death, respawn once on the fixed identity, retry the request once). Multi-endpoint health selection remains unbuilt; this only keeps one local child reachable after a crash.

The trait is the seam where per-vendor translation will live. Its shape is whole-response only: one `send` rather than the `build`/`whole`/`stream` triple the unbuilt design specifies, because nothing streams yet and a three-method trait with two unimplemented methods would be a seam pretending to be a mechanism.

## One file configures the process, and an unknown key stops the boot

One file, `deny_unknown_fields` on every struct, so a misspelled limit is a boot failure rather than a setting silently ignored. Tension: adding a field is then a breaking change for anyone who set an unrecognised one early.

```rust
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct Config {
    pub server: ServerConfig,
    #[serde(rename = "endpoint")] pub endpoints: Vec<EndpointConfig>,
    #[serde(rename = "model")] pub models: Vec<ModelConfig>,
    /// Built-in tool configuration. Absent when no `[tools]` section is present.
    #[serde(default)] pub tools: Option<ToolsConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ServerConfig {
    /// No default. A network interface, never loopback: Cursor connects from a
    /// workstation and Talktron from its own process, so a guessed default would
    /// either bind loopback and break both or bind every interface silently.
    pub bind: SocketAddr,
    /// The same shared secret the MCP server checks, compared in constant time.
    pub token: Secret,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct EndpointConfig {
    /// An operator-chosen handle referenced by `[[model]]` entries. Distinct from
    /// a model's caller-facing `name`.
    pub id: String,
    pub protocol: Protocol,
    /// A trailing slash is trimmed.
    pub base_url: String,
    pub api_key: Secret,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[non_exhaustive]
pub struct ModelConfig {
    /// The name a slot in `prompts.toml` resolves to, matched by one exact string
    /// lookup. It is the string that stays fixed across deployments while everything
    /// below it changes, which is the whole mechanism of this file.
    pub name: String,
    /// The string the backend knows this model by, substituted into the outgoing body
    /// and never returned to the caller.
    pub upstream: String,
    /// Endpoint ids. Only the first is used; `validate` rejects an empty list and an
    /// id no `[[endpoint]]` defines.
    pub endpoints: Vec<String>,
    /// Parsed and not read. Nothing in the crate consults it.
    #[serde(default)] pub default_max_tokens: Option<u32>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError>;
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError>;
    pub fn validate(&self) -> Result<(), ConfigError>;
}
```

`[tools]` carries one optional `[tools.web_search]` table, itself `deny_unknown_fields`, whose keys are tabulated with the route above. Absent, the search route answers 404 and everything else is unaffected.

`validate` rejects a duplicate endpoint id; a duplicate model name; a model with an empty endpoint list; a model naming an endpoint that is not defined; `queue.max_depth` below 1; and an endpoint `concurrency` below 1 when present. `max_depth` is the number of *waiting* requests (not counting in-flight). An omitted `concurrency` means unlimited for that endpoint (queue is a pass-through). Every other check the unbuilt design describes - a model on an `anthropic` endpoint without `default_max_tokens`, an unknown pack name - is about configuration this crate does not parse. An endpoint id that no model references is not an error.

The `${VAR}` interpolation is a load-time pass over the file's whole text rather than a per-field decoder, so it applies to any string value, and an unresolved variable fails the load before the TOML is parsed. That ordering is why a missing key is reported as a missing environment variable rather than as a type error somewhere further in.

The shipped `gateway.toml` at the repository root is one endpoint (Anthropic's OpenAI-compatible endpoint), one model, and the search tool, with all three secrets interpolated from the environment.

## Every secret enters through one file and redacts everywhere else

Keys come from `gateway.toml` string fields, and `${VAR}` in any string value is expanded from the process environment at load time. `$$` escapes a literal dollar sign. An unresolved variable fails the load, so a deployment that forgot to export a key never starts serving with a blank credential.

Interpolation is the only environment mechanism. There is no implicit pickup of `ANTHROPIC_API_KEY` or `OPENAI_API_KEY` from the ambient environment, because implicit pickup makes a deployment's effective credential invisible in its configuration and turns a leftover shell export into a live production key. Tension: a container that already passes secrets as environment variables still has to write the one-line reference.

```rust
#[derive(Clone, Deserialize)]
#[serde(from = "String")]
pub struct Secret(String);

impl Secret {
    /// The only accessor. Called when building an auth header.
    pub fn expose(&self) -> &str;
    pub fn is_empty(&self) -> bool;
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}

impl fmt::Display for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result;
}
```

`Secret` has no `Serialize`, so it cannot reach a metrics label or a JSON error body by accident. `Debug` and `Display` both redact, so a `tracing` field or a `{:?}` on `EndpointConfig` is safe. An upstream error body is truncated when it is captured, and it is never relayed to the caller, because a misconfigured backend can echo a request header; the truncation is 2,000 characters rather than 2 KiB, which is the same intent measured in a different unit.

## Every failure is the OpenAI error envelope, so an unmodified SDK reads it

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayError {
    #[error("unauthorized")]
    Unauthorized,                                       // 401
    #[error("unknown model {0}")]
    UnknownModel(String),                               // 404
    #[error("tool not configured: {0}")]
    ToolNotConfigured(&'static str),                    // 404
    #[error("malformed request: {0}")]
    MalformedRequest(String),                           // 400
    #[error("upstream transport error")]
    UpstreamTransport(#[source] Box<dyn std::error::Error + Send + Sync>), // 502
    #[error("upstream returned {status}")]
    UpstreamStatus { status: u16, body: String },       // see below
}
```

Every error body is the OpenAI error envelope, so an unmodified SDK surfaces it as its own error type rather than as an unparseable blob:

```json
{ "error": { "message": "unknown model reasoning-large",
             "type": "invalid_request_error", "code": "model_not_found" } }
```

`message` is the variant's `Display`. `type` and `code` are fixed per variant:

| Variant | Status | `type` | `code` |
|---|---|---|---|
| `Unauthorized` | 401 | `authentication_error` | `unauthorized` |
| `UnknownModel` | 404 | `invalid_request_error` | `model_not_found` |
| `ToolNotConfigured` | 404 | `invalid_request_error` | `not_found` |
| `MalformedRequest` | 400 | `invalid_request_error` | `malformed_request` |
| `UpstreamTransport` | 502 | `server_error` | `upstream_transport` |
| `UpstreamStatus`, upstream 4xx | the upstream's | `invalid_request_error` | `upstream_client_error` |
| `UpstreamStatus`, upstream 5xx | 502 | `server_error` | `upstream_error` |

`UnknownModel` reports `model_not_found`, OpenAI's own code for an unresolvable model and therefore the string a client written against OpenAI already handles. `type` collapses the rows onto three values because it is the coarse class an SDK groups on rather than an identifier. Tension: `UpstreamStatus` is the only variant whose status is not fixed by the variant, so a client matching on status alone cannot tell the gateway refusing from the backend refusing.

Two departures from the rule the unbuilt design states, that `code` is the variant name in snake case. `ToolNotConfigured` reports `not_found`, which is not its variant name and which a future 404 could reuse. And a 429 from the backend is a client error like any other, so it passes through as `upstream_client_error` with the upstream's status and no special handling of its `Retry-After`.

`MalformedRequest` is declared and classified and never constructed: a body that will not deserialize is rejected by `axum`'s own JSON extractor and never reaches a handler, so its rejection shape is `axum`'s rather than the envelope above.

`UpstreamTransport` boxes the transport error rather than naming `reqwest::Error` in the public API, so a change of HTTP client is not a breaking change for a caller matching on the enum.

## The runtime is built by hand so the service and foreground paths cannot drift

`promptforge-gateway serve <gateway.toml>`. The path is explicit and positional and has no default; anything else prints the usage line and exits with failure.

No `#[tokio::main]` anywhere in the crate. A Windows service entry point is called by the SCM on a thread the SCM owns, after `main` has already handed control to `service_dispatcher`, so the runtime has to be constructed inside the service handler. An attribute macro on `main` builds it in the wrong place and at the wrong time. The foreground path builds a runtime the same way, so when the service path is added there is exactly one construction site and the two cannot drift.

Startup order is load profile (with includes), start `LocalRuntime` (provision + spawn children when `[[local_model]]` is set), build the routing table (remote then local), build handler state (including profiles dir for admin routes), bind, serve. A configuration that will not load or validate is a startup failure with the error on stderr, so a broken file never reaches a listening socket. Process exit or profile switch drops the previous `LocalRuntime` and kills its children.

*2026-08-08 - cursor-grok-4.5*
