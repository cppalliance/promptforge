# `promptforge-gateway` public API redesign

## Purpose and stance

`promptforge-gateway` is a lib+bin application: an OpenAI-compatible gateway server that routes chat completions to remote endpoints and to a gateway-owned `llama-server` subprocess. It has no downstream library consumers in the workspace. The only real consumers of the library target are the crate's own binary (`src/main.rs`) and the integration-test binary (`tests/it/main.rs`).

That fact is the design lever. Today `src/lib.rs` declares all ten implementation modules `pub`, so the entire config schema, error internals, routing internals, queue scheduler, upstream adapters, Axum handlers, subprocess helpers, and post-processing helpers are a semver surface. None of it needs to be. The redesign collapses the public surface to a deliberate application facade and privatizes everything else to `pub(crate)` or private, which simultaneously resolves the large family of "internal detail is public / dependency type leaks / passive struct has public invariant-bearing fields" findings.

Privatization here is safe and is the primary tool. Where the binary or tests genuinely need a seam, the redesign gives them one narrow, typed, owned seam rather than the current grab-bag of loose constructors.

## Current public API (authoritative surface)

Grounded with `cargo public-api -p promptforge-gateway` on the reviewed tree (the crate builds and the surface extracts cleanly; the extraction also emits three rustdoc warnings for private intra-doc links and one redundant link in `local/mod.rs`, which fail a `-D warnings` doc gate). Everything below is externally reachable because every module is `pub`:

- root: `AppState` (public, non-`#[non_exhaustive]`) with `AppState::new` and the six-argument `AppState::from_parts`; `build_router(AppState) -> axum::Router`.
- `config`: `Secret`; `Protocol` (missing `#[non_exhaustive]`); `Config`, `ServerConfig`, `EndpointConfig`, `ModelConfig`, `DeviceConfig`, `LaneConfig`, `LocalConfig`, `LocalModelConfig`, `ToolsConfig`, `WebSearchConfig` (all `#[non_exhaustive]` but with public invariant-bearing fields and public `Deserialize`); enums `DeviceKind`, `ThinkingMode`, `SearchProvider`; `Config::{load, from_toml_str, validate, endpoint_concurrency, local_model_concurrency}`.
- `error`: `GatewayError` and `ConfigError` (both leak: `GatewayError::UpstreamTransport(Box<dyn Error>)`, `GatewayError::upstream_transport(reqwest::Error)`, string buckets `SwitchFailed`/`Parse`/`Validation`); `GatewayError: IntoResponse`.
- `local`: `LocalError` (leaks `reqwest::Error`, `io::Error`; 14 data variants without per-variant `#[non_exhaustive]`; `Server(String)` catch-all); `LocalRuntime::{empty, start, models, child_count}`; six fixture constants `DEV_MODEL_*` / `SCENARIO_MODEL_*`.
- `profile`: `MAX_INCLUDE_DEPTH`, `default_profiles_dir`, `list_profiles`, `load_named`, `load_path`.
- `queue`: `QueueConfig`, `AdmitError`, `Permit`, `EndpointLane::{unlimited, new, admit}`.
- `routing`: `Endpoint`, `Model` (public fields, no `#[non_exhaustive]`, leak `Arc<dyn Upstream>`, `EndpointLane`, stringly `tool_dialect`/`tools_mode`), `Routing::{new, from_config, merge, model, models}`.
- `tools`: `WebSearchSettings`, `WebSearchState`, `WebSearchRequest`, `WebSearchResponse`, `SearchResult`, the `web_search` Axum handler, plus nine re-exports from `web_search_process`.
- `upstream`: unsealed `Upstream` trait (`async fn send` over wire DTOs), `OpenAiUpstream::new`.
- `web_search_process`: three `*_MAX_CHARS` constants and seven processing fns.
- `wire`: `ChatRequest`, `ChatResponse`, `ModelsResponse`, `ModelInfo` (public fields leak `serde_json::Value`/`Map`, stringly dialect/mode).

Binary consumes: `config::Config`, `local::LocalRuntime`, `profile::{self, default_profiles_dir}`, `routing::Routing`, `AppState`, `build_router`, plus `config.server.{bind,key}` and `config.tools`. Integration tests consume: `Config`, `Routing`, `AppState`, `build_router`, `LocalRuntime::{empty,start}`, `SCENARIO_MODEL_URL`, `SCENARIO_MODEL_SHA256`, `Secret`, `profile::load_named`.

## Proposed public API (smallest coherent surface)

The crate root becomes a facade of `pub use` re-exports and one application entry point. Every implementation module is declared private (`mod config;`), and the root re-exports only the items below. All new/retained public structs and enums are `#[non_exhaustive]`; all fallible functions document `# Errors`; every retained public item gets a doctest.

### Application entry point (binary path)

```rust
/// Options for running the gateway. Built by the binary from parsed args.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ServeOptions {
    pub profiles_dir: PathBuf,
    /// Mutually exclusive with `config_path`; enforced at construction.
    pub source: ConfigSource,
}

/// Where startup configuration comes from. Replaces the "profile and/or path,
/// but actually not both" contract that `main.rs` advertised then rejected.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ConfigSource {
    Profile(ProfileName),
    Path(PathBuf),
}

/// Load config, provision local children, bind, and serve until Ctrl-C.
/// Owns the tokio runtime; the binary stays a thin arg-parsing shell.
pub fn run(options: ServeOptions) -> Result<(), StartupError>;
```

### In-process assembly (test path, and used internally by `run`)

```rust
/// A fully assembled, owning gateway. Holds the live routing table, the server
/// key, the web-search capability, and - crucially - the `LocalRuntime`, so
/// dropping a `Gateway` terminates every managed `llama-server` child.
pub struct Gateway { /* opaque */ }

impl Gateway {
    /// Assemble from a validated config. Provisions and starts local models.
    pub fn from_config(config: &Config, profiles: ProfilesContext)
        -> Result<Gateway, StartupError>;

    /// The Axum router for this gateway. This is the crate's one deliberate,
    /// documented Axum integration point; the crate is an application, not a
    /// general library, so exposing `axum::Router` here is intentional.
    #[must_use]
    pub fn router(&self) -> axum::Router;

    /// Serve on a caller-owned listener until `shutdown` completes. Tests pass
    /// an ephemeral `TcpListener` they bound themselves (no port race), read
    /// back `local_addr`, and drive a rendezvous instead of sleeping.
    pub async fn serve(
        self,
        listener: tokio::net::TcpListener,
        shutdown: impl Future<Output = ()> + Send + 'static,
    ) -> Result<(), ServeError>;
}

/// Optional admin-profile directory plus the active profile name.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct ProfilesContext {
    pub dir: Option<PathBuf>,
    pub active: Option<ProfileName>,
}
```

`AppState`, `AppState::new`, `AppState::from_parts`, and the free `build_router` are removed from the public API (folded into `Gateway` and made `pub(crate)`).

Two constructors and two trait impls are deliberately retained on the facade and are part of the intended surface:

- `ServeOptions::new(PathBuf, ConfigSource) -> ServeOptions` and `ProfilesContext::new(Option<PathBuf>, Option<ProfileName>) -> ProfilesContext`. Both structs are `#[non_exhaustive]`, so the separate `promptforge-gateway` binary and integration-test crates cannot use struct-literal construction; the constructors are the only supported build path for those callers.
- `impl Debug for Gateway` (opaque, redaction-safe: the inner state's `Secret` fields redact) so operators can log an assembled gateway handle.
- `impl Display for ProfileName` renders the validated single-component name; it is used internally for path assembly, admin responses, and tracing, and is a natural rendering for a public name newtype.

`Config` retains `#[derive(Debug, Clone)]`; the `Clone` impl is part of the surface (a validated `Config` is cheap to clone when assembling multiple gateways or profile-switch candidates). `Secret` carries no public `Deserialize` or `From<String>` impl and no `is_empty`: it is constructed only crate-internally, so `expose` is its single public accessor.

### Validated configuration (re-exported at root)

```rust
/// Validated gateway configuration. Invariant-bearing fields are private;
/// deserialization goes through a private raw DTO and `TryFrom`, so a value of
/// this type cannot hold an invalid state (empty key, zero counts, unknown
/// endpoint refs, kind-incompatible devices, bad web-search bounds).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Config { /* private */ }

impl Config {
    pub fn load(path: &Path) -> Result<Config, ConfigError>;
    pub fn from_toml_str(raw: &str) -> Result<Config, ConfigError>;
    pub fn load_profile(dir: &Path, name: &ProfileName) -> Result<Config, ConfigError>;
    // read-only accessors as needed by tests; no public mutation
}

/// A redacting secret. No `Serialize`; `Debug`/`Display` redact; `expose` is
/// the single plaintext accessor. Server key is a validated non-empty secret.
#[derive(Clone)]
pub struct Secret(/* private */);

/// Exactly one normal path component with a non-empty UTF-8 stem. Rejects path
/// separators, `.`, `..`, and empty. This is the profile-switch confinement type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileName(/* private */);
impl ProfileName { pub fn parse(s: &str) -> Result<ProfileName, ProfileNameError>; }

#[must_use] pub fn default_profiles_dir() -> PathBuf;
```

### Public errors (opaque, source-preserving, classifiable)

```rust
/// Startup/serve failure. Opaque wrapper over a private representation that
/// preserves the underlying cause via `source()`. No dependency types leak.
#[non_exhaustive]
pub struct StartupError { /* private */ }
impl StartupError { pub fn kind(&self) -> StartupErrorKind; }

#[non_exhaustive]
pub enum StartupErrorKind { Config, Provisioning, Bind, Serve }

/// Config load/validate failure. Opaque; keeps `toml::de::Error`, `io::Error`,
/// and validation detail as private `#[source]`; paths kept as `PathBuf`.
#[non_exhaustive]
pub struct ConfigError { /* private */ }
impl ConfigError { pub fn kind(&self) -> ConfigErrorKind; }

#[non_exhaustive]
pub enum ConfigErrorKind {
    Read, Parse, Interpolation, UnresolvedVar,
    Validation, IncludeCycle, IncludeDepth,
}

#[non_exhaustive] pub struct ServeError { /* private */ }
#[non_exhaustive] pub struct ProfileNameError { /* private */ }
```

Everything else - `GatewayError` (HTTP adapter), `LocalError`, `LocalRuntime`, `Routing`, `Endpoint`, `Model`, `Upstream`, `OpenAiUpstream`, `EndpointLane`, `Permit`, `QueueConfig`, `AdmitError`, all `wire` DTOs, all `tools` DTOs, the `web_search` handler, `web_search_process` helpers, `profile::{list_profiles, load_path, MAX_INCLUDE_DEPTH}`, and the six fixture constants - becomes `pub(crate)` or private.

## Removals and privatizations

| Item(s) | Action | Rationale |
|---|---|---|
| `pub mod {config,error,local,profile,queue,routing,tools,upstream,web_search_process,wire}` | change to private `mod`; root `pub use` only the facade | PGW-LIB-001; crate root becomes a facade |
| `AppState`, `AppState::new`, `AppState::from_parts`, `build_router` | `pub(crate)`; replaced by `Gateway` | PGW-LIB-002/003/004/005 |
| `GatewayError`, `ConfigError` (old shape), `LocalError` | make `GatewayError`/`LocalError` `pub(crate)`; replace public `ConfigError` with opaque wrapper | ERR-001..007, LOCAL-ERROR-001..005 |
| `routing::{Endpoint, Model, Routing, Upstream, OpenAiUpstream}` | `pub(crate)`; public fields become private | ROUTING-003/004/005, UP-006, WIRE-004 |
| `wire::{ChatRequest, ChatResponse, ModelsResponse, ModelInfo}` | `pub(crate)`; add internal typed message/choice validation | WIRE-001..008 |
| `tools::*` (state, DTOs, `web_search` handler) | `pub(crate)` | TOOLS-001/005 |
| `web_search_process::*` + `tools` re-exports | private; keep only `post_process_results` `pub(crate)` | TOOLS-002/014, WSP-005 |
| `queue::{QueueConfig, AdmitError, Permit, EndpointLane}` | `pub(crate)` | Q cross-file; validated config carries queue settings |
| `local::{six fixture constants}` | remove from API; move fixtures into `tests/it` | MOD-006, ART-010 |
| `profile::{list_profiles, load_path, MAX_INCLUDE_DEPTH}` | `pub(crate)`; fold `load_named` into `Config::load_profile` | PROFILE-004 |
| `config::Protocol`, `ThinkingMode`, `SearchProvider`, `DeviceKind` and all `*Config` structs | `pub(crate)` behind the validated `Config` | CFG-002/008/009 |

## Responsibility moves and internal structure

Ownership and file layout changes that back the facade and satisfy the >500-line splits and the lifecycle/soundness findings:

- Startup orchestration moves from `main.rs` into a library `runner` (`run`, `Gateway`). `main.rs` keeps only `parse_args(args_os) -> Result<ServeOptions, _>` (typed, unit-tested, `args_os` for non-UTF-8 paths) and exit-code selection. (MAIN-001..006)
- `config.rs` (1,337 lines) splits into `config/` facade + `secret.rs`, `raw.rs` (deserialize DTOs), `interpolate.rs` (parse-TOML-first, interpolate only string values), `validate.rs` (`TryFrom<Raw> for Config`, `NonZeroUsize`/`NonZeroU8`, validated URL, closed enums). (CFG-002/003/004/006/007/012)
- `local/` ownership is corrected so `LocalRuntime` (owned by `Gateway`) holds the supervised child handles; routing exposes non-owning descriptors. `Drop` no longer both under-guarantees and escapes. Add an explicit bounded async `shutdown` invoked via `spawn_blocking`; `Drop` is best-effort non-blocking. One process-wide Ctrl-C watcher via `OnceLock`; a shared cancellation token threads into startup and respawn readiness so profile switch cancels in-flight recovery. (MOD-001/002/003, SERVER-001/002/004)
- `local/mod.rs` -> `local.rs` + `runtime.rs`, `dialect.rs`; `artifacts.rs` (1,569) and `server.rs` (1,244) split into download/auth, archive-extraction, cache-confinement/locking, digest, progress, identity, readiness, capture submodules. (MOD-005, ART-008, SERVER-007)
- Bounded I/O everywhere: a shared local-upstream/remote-upstream/web-search HTTP policy sets connect + whole-request timeouts and a byte-capped body reader for success and error paths, deserializing from bounded bytes; sidecar and readiness reads capped; timestamp subprocess replaced with `SystemTime` formatting. (UP-001/002/003, UPSTREAM-001/002, TOOLS-009/010, WSP-001/002, SERVER-003, SIDECAR-001/002/003)
- Digest-verified artifacts with path confinement: require a validated 64-hex `Sha256` for every remote source (or explicit HTTPS-only unpinned policy), attach the hub token only to an allowlisted HTTPS Hugging Face host, cache key from normalized source identity, atomic temp-file publication for sidecars/blobs, no-follow/handle-relative writes under the cache root. (ART-001..006, SIDECAR-004/005)
- Errors narrowed and source-preserving: `LocalError::Server(String)` -> operation variants with `#[source]`; `ConfigError`/`StartupError` opaque wrappers with `kind()` classification; upstream decode failures classified distinctly from transport so child-respawn triggers only on genuine transport death. (ERR-002/003/004, LOCAL-ERROR-003/004, UP-004, UPSTREAM-003)
- Determinism/lifecycle in tests: `Gateway::serve` on a test-owned ephemeral listener; a `pub(crate)` queue-admission rendezvous seam replaces sleeps; a `TestServer` fixture (in `tests/it`) owns addr + shutdown + join handle; per-phase `tokio::time::timeout`. (Q-004/005, IT-001..004)

## Invariants the new surface enforces

- A `Config` value cannot hold an invalid state: non-empty server `Secret`; positive `context`/counts (`NonZero*`); `default_count <= max_count`; validated HTTP(S) URLs; endpoint refs resolve; no duplicate endpoint ids within a model; device kind matches its payload; closed vocabularies (`Protocol`, freshness, safesearch, tool dialect/mode) are enums. (CFG-001..006, ROUTING-005, WIRE-005)
- `Secret` never serializes and redacts in `Debug`/`Display`; `expose` is the one accessor.
- `ProfileName` is one normal path component; profile switch and includes cannot escape the profiles root (documented include-boundary policy). (LIB-007, PROFILE-009/010)
- `Gateway` owns `LocalRuntime`; child lifetime is bounded by gateway lifetime; profile switch is serialized (generation guard or owner task) and either fully swaps state or leaves the prior state intact with a stable admin credential. (MOD-001, LIB-008/009)
- Every HTTP body (request-decode, success, error) is byte-bounded before allocation; every outbound HTTP call has a timeout. (UP-*, TOOLS-010, WSP-001)
- Public errors preserve `source()` and expose `kind()`; no dependency type appears in any public signature.

## Required tests and docs

- Doctests on every retained public item: `run`, `ServeOptions`/`ConfigSource`, `Gateway::{from_config, router, serve}`, `Config::{load, from_toml_str, load_profile}`, `Secret`, `ProfileName::parse`, `default_profiles_dir`, and the public error `kind()` methods.
- Table-driven config rejection tests: empty key, empty/zero/malformed fields, duplicate ids, kind-incompatible devices, bad web-search bounds, interpolation of quotes/backslashes/newlines/`$$`.
- Boundary tests: oversized and exactly-at-limit success and error bodies for remote/local upstream and web-search; stalled-upstream timeout releases the lane; malformed JSON classified as decode not transport.
- Lifecycle tests: drop `Gateway` terminates children even when a model was cloned into routing; profile switch cancels in-flight respawn; async caller stays responsive during teardown.
- Artifact tests: digest required/normalized, hub token only on allowlisted host, archive traversal/link rejection, atomic sidecar publication, interrupted-download cleanup.
- Deterministic integration tests: owned ephemeral listener, rendezvous-based queue-full 503, per-phase timeouts, fake backend that validates method/path/headers/model/messages and records them; error responses assert stable machine code and shape.
- Doc gate: fix the private intra-doc links so `RUSTDOCFLAGS="-D warnings" cargo doc` passes. (MOD-004, UPSTREAM-004)

## Disposition of every finding

API-related findings are resolved by the facade/typing changes above; internal-only findings are recorded here as required internal work that the sweep must include (they do not add public surface).

### Manifest and root

| Finding | Disposition |
|---|---|
| MANIFEST-001 | API-adjacent: set `version = "0.0.0"`, keep `publish = false`. |
| MANIFEST-002 | Non-API: unpublished app; record the metadata exemption in workspace policy (or add `readme`/`keywords`). |
| PGW-LIB-001 | Resolved: private modules + root `pub use` facade. |
| PGW-LIB-002 | Resolved: `from_parts` removed; replaced by `Gateway::from_config`. |
| PGW-LIB-003 | Resolved: `AppState` privatized; `Gateway` opaque. |
| PGW-LIB-004 | Resolved: single documented Axum point (`Gateway::router`); handlers/`GatewayError` privatized. |
| PGW-LIB-005 | Resolved: `Gateway::router` is `#[must_use]`. |
| PGW-LIB-006 | Resolved: doctests + route/auth docs on the facade. |
| PGW-LIB-007 | Resolved: `ProfileName` confinement type. |
| PGW-LIB-008 | Resolved: serialized switch (generation guard/owner task). |
| PGW-LIB-009 | Resolved: build+validate off-lock, atomic swap, stable credential. |
| PGW-LIB-010 | Internal: use a vetted constant-time compare on fixed-length digests. |
| PGW-LIB-011 | Resolved: bounded `ClientId` newtype at the queue boundary. |

### config

| Finding | Disposition |
|---|---|
| CFG-001 | Resolved: validated non-empty server `Secret`. |
| CFG-002 | Resolved: private raw DTO + `TryFrom`, private fields, `NonZero*`. |
| CFG-003 | Resolved: validate all required strings/URLs/counts. |
| CFG-004 | Resolved: model device kind payloads as distinct variants; require `Local` for local models. |
| CFG-005 | Resolved: reject duplicate endpoint ids within a model. |
| CFG-006 | Resolved: validate web-search bounds at load; drop downstream `.max(1)` repair. |
| CFG-007 | Resolved: parse TOML first, interpolate only string values. |
| CFG-008 | Resolved: `Protocol` privatized behind validated `Config` (moot as public). |
| CFG-009 | Resolved: `Deserialize` on private DTOs only; public `Config` has no public serde. |
| CFG-010 | Resolved: derive `Clone`/`PartialEq`/`Eq` on validated types where valid. |
| CFG-011 | Resolved: doctests on retained `Config` entry points. |
| CFG-012 | Resolved: `config/` module split. |
| CFG-013 | Resolved: structured `ConfigErrorKind`; accurate `# Errors`. |
| CFG-014 | Resolved: table-driven rejection tests. |

### error

| Finding | Disposition |
|---|---|
| ERR-001 | Resolved: opaque errors; no `reqwest::Error`/`Box<dyn Error>` in public signatures. |
| ERR-002 | Resolved: keep `toml::de::Error` as private `#[source]`. |
| ERR-003 | Resolved: narrow operation variants replace `SwitchFailed(String)`. |
| ERR-004 | Resolved: `kind()` classification on public errors. |
| ERR-005 | Resolved: narrow per-unit errors composed privately. |
| ERR-006 | Resolved: paths as `PathBuf`, include chain as `Vec<PathBuf>` internally. |
| ERR-007 | Internal: table-driven classification/`Display`/`source` unit tests. |

### local (artifacts, error, mod, server, sidecar, upstream)

| Finding | Disposition |
|---|---|
| ART-001 | Resolved (soundness): hub token only on allowlisted HTTPS HF hosts. |
| ART-002 | Resolved: require validated digest (or explicit HTTPS-only unpinned policy) + revalidation. |
| ART-003 | Resolved: connect/read timeouts + max artifact size. |
| ART-004 | Resolved: cache key from normalized source identity/digest. |
| ART-005 | Resolved: parse digest into fixed-size `Sha256` at boundary. |
| ART-006 | Resolved: handle-relative/no-follow atomic cache writes. |
| ART-007 | Internal: archive/traversal/cleanup/concurrency tests. |
| ART-008 | Resolved: split `artifacts.rs`. |
| ART-009 | Resolved: fail with a config error when no home dir. |
| ART-010 | Resolved: constants removed from API; fixtures move to tests. |
| LOCAL-ERROR-001 | Resolved: `LocalError` `pub(crate)`; dependency types private. |
| LOCAL-ERROR-002 | Resolved (moot once `pub(crate)`); add per-variant `#[non_exhaustive]` if any stays public. |
| LOCAL-ERROR-003 | Resolved: operation variants with `#[source]` replace `Server(String)`. |
| LOCAL-ERROR-004 | Resolved: `is_retryable`/category classification. |
| LOCAL-ERROR-005 | Internal: lowercase `Display`. |
| MOD-001 | Resolved: `LocalRuntime` owns children; routing non-owning. |
| MOD-002 | Resolved: single `OnceLock` Ctrl-C watcher / injected cancellation. |
| MOD-003 | Resolved: capability discovery returns `Result<Option<bool>, _>`. |
| MOD-004 | Resolved: fix private intra-doc links. |
| MOD-005 | Resolved: split `local/mod.rs` -> `local.rs` + submodules. |
| MOD-006 | Resolved: fixture constants leave the API. |
| MOD-007 | Resolved: narrow startup error, opaque sources. |
| MOD-008 | Resolved: `#[non_exhaustive]` + `Default` on `LocalRuntime`; doctests. |
| MOD-009 | Internal: extract pure evidence-merge fn and test it. |
| SERVER-001 | Resolved: bounded `shutdown` via `spawn_blocking`; non-blocking `Drop`. |
| SERVER-002 | Resolved: `try_wait`-first; propagate kill/wait failures from explicit ops. |
| SERVER-003 | Resolved: byte-capped readiness body into a narrow struct. |
| SERVER-004 | Resolved: shared cancellation token tracks recovery workers. |
| SERVER-005 | Internal: typed capture completion; preserve reader/panic failures. |
| SERVER-006 | Resolved (soundness): OS-crypto RNG for the loopback bearer token. |
| SERVER-007 | Resolved: split `server.rs`. |
| SIDECAR-001 | Resolved: remove `date` subprocess; format `SystemTime`. |
| SIDECAR-002/003 | Resolved: bounded tokenizer/sidecar reads. |
| SIDECAR-004 | Resolved: atomic temp-file publication; validate fast-path file. |
| SIDECAR-005 | Internal: versioned structured sidecar format + round-trip tests. |
| SIDECAR-006 | Internal: typed fetch error; caller downgrades deliberately. |
| UPSTREAM-001 | Resolved: local client timeouts from a named policy. |
| UPSTREAM-002 | Resolved: bounded error-body read. |
| UPSTREAM-003 | Resolved: decode vs transport classification; recovery on transport only. |
| UPSTREAM-004 | Resolved: rewrite `LocalRuntime` docs without private links. |
| UPSTREAM-005 | Internal: cooldown/respawn-failure/concurrency tests. |

### profile

| Finding | Disposition |
|---|---|
| PROFILE-001 | Resolved: return type error on non-array inherited collections. |
| PROFILE-002 | Resolved: choose and document one device-overlay contract. |
| PROFILE-003 | Resolved: `list_profiles` checks `is_file()`. |
| PROFILE-004 | Resolved: module private; `load_named` -> `Config::load_profile`; keep `default_profiles_dir`. |
| PROFILE-005 | Resolved: doctests on retained items. |
| PROFILE-006 | Resolved: accurate `# Errors` via `ConfigErrorKind`. |
| PROFILE-007 | Internal: single-read diagnostics; avoid false line numbers. |
| PROFILE-008 | Internal: negative merge/name tests. |
| PROFILE-009 | Resolved: document and enforce include boundary. |
| PROFILE-010 | Resolved: `ProfileName` grammar (one path component). |

### queue

| Finding | Disposition |
|---|---|
| Q-001 | Resolved: scheduling identity from a bounded `ClientId` (server-validated). |
| Q-002 | Resolved: `NonZeroUsize` in validated settings; no panicking public ctor. |
| Q-003 | Resolved: bounded client-id length caps per-entry memory. |
| Q-004 | Resolved: rendezvous seam replaces sleeps. |
| Q-005 | Internal: deterministic queued-cancellation tests. |
| Q-006 | Internal: distinguish closed-channel from `QueueFull`. |
| Q-007 | Internal: `Send + Sync` compile assertions. |
| Q-008 | Resolved (moot once `pub(crate)`); doctests only if any stays public. |

### routing

| Finding | Disposition |
|---|---|
| ROUTING-001 | Resolved: fallible construction rejects duplicate names; `new` `pub(crate)`. |
| ROUTING-002 | Resolved: `from_config` validates first; `pub(crate)`. |
| ROUTING-003/004 | Resolved: `Endpoint`/`Model` `pub(crate)`, private fields. |
| ROUTING-005 | Resolved: `tool_dialect`/`tools_mode` become enums. |
| ROUTING-006 | Resolved (moot once `pub(crate)`). |
| ROUTING-007 | Internal: routing invariant tests. |

### tools and web_search_process

| Finding | Disposition |
|---|---|
| TOOLS-001 | Resolved: `tools` module `pub(crate)`; no Axum handler in API. |
| TOOLS-002 | Resolved: drop broad re-export; import `post_process_results` privately. |
| TOOLS-003 | Resolved: private fields; `NonZeroU8` limits validated once. |
| TOOLS-004 | Resolved: closed knobs as enums/newtypes; Unicode-aware query trim + limits. |
| TOOLS-005 | Resolved (moot once `pub(crate)`). |
| TOOLS-006 | Internal: derive `Clone`/`PartialEq` as needed. |
| TOOLS-007 | Resolved: docs only for anything retained (none public). |
| TOOLS-008 | Internal: context wrapper preserves `source()`. |
| TOOLS-009 | Internal: handle body-read result explicitly. |
| TOOLS-010 | Resolved: byte-bounded success and error bodies. |
| TOOLS-011 | Internal: deterministic mock-server handler tests. |
| TOOLS-012 | Internal: `TryFrom`/`From` conversion. |
| TOOLS-013 | Resolved: split `tools.rs`. |
| TOOLS-014 | Resolved: real `url` parsing; drop ad hoc host extraction from API. |
| WSP-001 | Resolved: cap `age`/`extra_snippets` count and length. |
| WSP-002 | Internal: bounded single-pass sanitize. |
| WSP-003/004 | Resolved: standards-compliant URL parsing; no char-boundary URL truncation. |
| WSP-005 | Resolved: module private; only `post_process_results` `pub(crate)`. |
| WSP-006 | Resolved: canonicalize/validate domain filters at request parse. |
| WSP-007 | Resolved (moot once private). |

### wire

| Finding | Disposition |
|---|---|
| WIRE-001 | Resolved: typed, validated message shapes at the boundary. |
| WIRE-002 | Resolved: minimally typed choices; structural failure -> upstream-protocol error. |
| WIRE-003 | Resolved: private fields + checked constructors reject reserved keys in `rest`. |
| WIRE-004 | Resolved: `wire` module `pub(crate)`. |
| WIRE-005 | Resolved: dialect/mode/object as enums/fixed literals. |
| WIRE-006 | Internal: derive `Clone`/`PartialEq` as needed. |
| WIRE-007 | Internal: serde round-trip/collision unit tests. |
| WIRE-008 | Resolved (moot once `pub(crate)`). |

### main and integration tests

| Finding | Disposition |
|---|---|
| MAIN-001 | Resolved: startup moves to library `run`/`Gateway`. |
| MAIN-002 | Resolved: `ConfigSource` enum makes profile/path mutually exclusive at parse. |
| MAIN-003 | Internal: distinguish signal-handler failure from interrupt. |
| MAIN-004 | Resolved: source-preserving `StartupError`; print full chain. |
| MAIN-005 | Resolved: `args_os` + `PathBuf` operands. |
| MAIN-006 | Resolved: pure `parse_args` + table-driven tests. |
| IT-001 | Resolved: rendezvous-based queue-full test. |
| IT-002 | Resolved: arrival/release channels replace sleep polling. |
| IT-003 | Resolved: per-phase `tokio::time::timeout`. |
| IT-004 | Resolved: owned `TestServer` fixture (addr + shutdown + join). |
| IT-005/006/008 | Internal: fake backend validates+records request; assert error codes; tighten smoke assertion. |
| IT-007 | Internal: split `tests/it` into area modules. |

## Self-check

- Every API-related finding above has an explicit disposition; internal-only findings are recorded as required sweep work that adds no public surface.
- The proposed surface (`run`, `ServeOptions`/`ConfigSource`, `Gateway`, `ProfilesContext`, `Config`, `Secret`, `ProfileName`, `default_profiles_dir`, and opaque `StartupError`/`ConfigError`/`ServeError`/`ProfileNameError` with `kind()`) is sufficient for both current consumers: the binary calls `run`; integration tests assemble a `Gateway` from a `Config`, bind their own listener, and drive `serve`/`router`. Fixture constants move into the test binary.
- The change is implementable in one crate sweep: privatization plus type-narrowing is local to this crate, `main.rs` and `tests/it` are updated in the same pass, and no other workspace crate imports `promptforge-gateway`, so the clean `cargo build --workspace` does not regress.

*2026-08-10 15:11 - claude-opus-4.8*
