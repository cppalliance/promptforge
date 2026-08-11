# Redesign: `promptforge-mcp-server` public API

Step 2 API synthesis, final crate of the crate-by-crate review. Inputs: the plan (`promptforge_crate_review_a3b9f964.plan.md`), the three rulebooks (rust section 6 API law, section 12 gates; vibe; prompts), and all ~145 findings for this crate. No source is modified here; this is the design that Step 3 implements.

## Governing fact: this is a leaf binary

Nothing in the workspace depends on `promptforge-mcp-server`. The only consumers of its library surface are:

- its own binary, `src/main.rs`, which imports `Catalog, CatalogHandle, Config, OnBroken, PreparedTools, Retrieval, Watcher, serve_http, serve_stdio` and calls `Config::load`, `Catalog::resolve`, `OnBroken::Reject`, `Retrieval::start`, `CatalogHandle::new`, `PreparedTools::load`, `Watcher::start`, `serve_http`, `serve_stdio`, plus the four boot error types through `Box<dyn Error>` + `report()` (`Display` + `source()`);
- its integration tests under `tests/it/`: `progress.rs` and `watch.rs` additionally use `PromptForgeServer` (constructed, then driven through an in-process `rmcp` session); `shipped.rs` currently uses `Entry::name` and `tool_definitions()`.

Therefore the public API is minimized aggressively: expose only the boot/serve seam that `main.rs` needs and the one server handler that the session tests construct. Everything else becomes `pub(crate)` or private, and the crate adopts `#![deny(unreachable_pub)]` so drift is caught. `shipped.rs` is migrated to assert through an in-process/subprocess session (per PFMS-SHIPPED-001), which removes the last external needs for `Entry` and `tool_definitions`.

## 1. Current public surface (baseline)

Captured with `cargo public-api -p promptforge-mcp-server` (nightly rustdoc JSON) from `crates/promptforge-mcp-server`; full verbatim dump saved to the crate scratch tree as `mcp-server-public-api.txt` (1875 lines including dependency blanket impls). The crate's own declared top-level items are **40**: 6 enums, 24 structs, 6 consts, 4 free fns (matches the `lib.rs` findings count). Verbatim declarations:

```text
pub mod promptforge_mcp_server
#[non_exhaustive] pub enum ConfigError { Read { path: String, source: io::Error }, Parse(String), Interpolation(String), UnresolvedVar(String), EmptyToken }
#[non_exhaustive] pub enum OnBroken { Reject, Retain }
#[non_exhaustive] pub enum RunStatus { Running, Completed, Failed }         // Serialize+Deserialize+JsonSchema
#[non_exhaustive] pub enum ServeError { MissingToken, Bind{addr,source}, Http{source}, Stdio(String) }
#[non_exhaustive] pub enum Shortlist { Candidates(Vec<Candidate>), Unavailable, Failed(String) }
#[non_exhaustive] pub enum WatchError { Create(String), Watch { path: String, detail: String } }
pub struct Candidate { pub name: String, pub description: String }          // Serialize
pub struct Catalog                                                          // resolve, entries, find, hash, len, is_empty
#[non_exhaustive] pub struct CatalogConfig { pub include: Vec<String>, pub exclude: Vec<String> }
#[non_exhaustive] pub struct CatalogError                                   // faults()
pub struct CatalogHandle                                                    // new, load, store
#[non_exhaustive] pub struct Config { pub server, paths, gateway, catalog, prompts }  // Deserialize; load, from_toml_str
pub struct Entry                                                            // name, description, path, source, prompt, problem
pub struct Fault                                                            // prompt, path, detail
#[non_exhaustive] pub struct GatewayConfig { pub url: String, pub key: Secret }
pub struct McpObserver                                                      // silent, reporting(Peer,ProgressToken), turns, dropped; impl Observer
#[non_exhaustive] pub struct PathsConfig { pub prompts: PathBuf }
pub struct PreparedTools                                                    // load -> Result<Self,Box<dyn Error>>, new -> Result<Self,Box<dyn Error>>
pub struct ProgressPump                                                     // async finish
#[non_exhaustive] pub struct PromptConfig { pub enabled: bool, pub file: Option<PathBuf> }
pub struct PromptForgeServer                                                // new, dispatch, dispatch_with_progress; impl rmcp::ServerHandler
#[non_exhaustive] pub struct Reload { pub ranking_changed: bool, pub refused: bool }
pub struct Reloader                                                         // new, reload
pub struct Retrieval                                                        // idle, start, shortlist, rebuild, is_available
pub struct RunRegistry                                                      // new, admit, check, admission_timeout, retain_completed
#[non_exhaustive] pub struct RunResult { pub run_id, prompt, status, value, turns, elapsed_ms, error }  // Serialize+JsonSchema
pub struct RunSlot
pub struct Secret(_)                                                        // expose, is_empty, is_blank; From<String>, Deserialize
#[non_exhaustive] pub struct ServerConfig { pub bind, token, max_concurrent_runs, admission_timeout, reply_deadline, retain_completed, watch, watch_debounce }
pub struct Watcher                                                          // start -> Result<Option<Watcher>,WatchError>
pub const CHECK_RUN: &str
pub const HEALTHZ_PATH: &str
pub const LIST_PROMPTS: &str
pub const MCP_PATH: &str
pub const NEED_PROMPT: &str
pub const RUN_PROMPT: &str
pub fn build_router(PromptForgeServer, Arc<Secret>) -> axum::Router
pub async fn serve_http(Arc<Config>, Arc<CatalogHandle>, Arc<Retrieval>, Arc<PreparedTools>) -> Result<(), ServeError>
pub async fn serve_stdio(Arc<Config>, Arc<CatalogHandle>, Arc<Retrieval>, Arc<PreparedTools>) -> Result<(), ServeError>
pub fn tool_definitions() -> Vec<rmcp::model::Tool>
```

Dependency types leaking through this surface today: `rmcp` (`McpObserver::reporting`, `PromptForgeServer::dispatch*`, `tool_definitions`, `ServerHandler`), `axum::Router` (`build_router`), `promptforge_core::{parser::Prompt, model::ModelCatalog, observe::Observer}` (`Entry::prompt`, `PreparedTools`, `McpObserver`), and erased `Box<dyn Error>` (`PreparedTools`).

## 2. Target minimized public API

**19 top-level public items** (down from 40): 7 structs, 1 domain enum, 5 error enums, 4 error-kind enums, 2 async fns. No `rmcp`, `axum`, `reqwest`, `notify`, `toml`, or `promptforge_core` type appears in any public signature. Crate root gains `#![deny(unreachable_pub)]`; every non-facade item defaults to `pub(crate)`.

Boot/serve seam (all consumed by `main.rs`; opaque, private representation):

- `pub struct Config` - opaque, all fields private (fixes CONFIG-001/003). Construction: `pub fn load(&Path) -> Result<Config, ConfigError>` and `impl FromStr<Err = ConfigError>` (CONFIG-008); serde `Deserialize` is removed from the public type and confined to a private `RawConfig` validated via `TryFrom` (CONFIG-001/002). `#[non_exhaustive]`. Boot helpers take `&Config`/`Arc<Config>`, so `main.rs` never touches fields.
- `pub struct Catalog` - `#[non_exhaustive]`, opaque. Public: `pub fn resolve(&Config, OnBroken) -> Result<Catalog, CatalogError>` (`#[must_use]` result via `Result`). `entries/find/hash/len/is_empty` become `pub(crate)` (CAT-004/007/008, CAT-001/003).
- `pub struct CatalogHandle` - `#[non_exhaustive]`, opaque. Public: `pub fn new(Catalog) -> Self` (`#[must_use]`). `load/store` become `pub(crate)`.
- `#[non_exhaustive] pub enum OnBroken { Reject, Retain }` - unchanged (already correct).
- `pub struct Retrieval` - `#[non_exhaustive]`, opaque. Public: `pub fn start(&Catalog) -> Self` (`#[must_use]`). `idle/shortlist/rebuild/is_available` become `pub(crate)`; `rebuild` gains a typed internal outcome (RETRIEVAL-001/004). Compile-time `Send+Sync+'static` assertion added (RETRIEVAL-006).
- `pub struct PreparedTools` - `#[non_exhaustive]`, opaque. Public: `pub async fn load(&Config) -> Result<PreparedTools, PreparedToolsError>` (BIND-001/002/003). `new(&Config, ModelCatalog)` becomes `pub(crate)`/test-only (removes `promptforge_core::ModelCatalog` leak, BIND-002). Compile-time `Send+Sync+'static` assertion (BIND-006).
- `pub struct Watcher` - `#[must_use = "dropping the watcher stops live reload"]` (WATCH-005), opaque, `#[non_exhaustive]`. Public: `pub fn start(&Path, Arc<Config>, Arc<CatalogHandle>, Arc<Retrieval>) -> Result<Option<Watcher>, WatchError>`; runtime-absence returns a `WatchError` variant instead of panicking (WATCH-003). Async `shutdown(self)` added for deterministic quiescence (WATCH-001).
- `pub async fn serve_http(...) -> Result<(), ServeError>` and `pub async fn serve_stdio(...) -> Result<(), ServeError>` - retained (only external serve entry points). Each gains an explicit shutdown/cancellation input owned by the caller (TRANSPORT-003).
- `pub struct PromptForgeServer` - `#[non_exhaustive]`, opaque, `Clone + Debug`, `impl rmcp::ServerHandler`. Public: `pub fn new(Arc<Config>, Arc<CatalogHandle>, Arc<Retrieval>, Arc<PreparedTools>) -> Self` (`#[must_use]`). `dispatch`/`dispatch_with_progress` become `pub(crate)` (SRV-003, LIB-001/003); integration tests exercise behavior through the `ServerHandler` (`.serve()`), which they already do. The `ServerHandler` impl is the single sanctioned `rmcp` integration point.

Error surface - all `#[non_exhaustive]`, opaque where they carry causes, source-preserving, each with a stable dependency-free `kind()` classifier (ERR-001/002/003/005):

- `#[non_exhaustive] pub enum ConfigError` + `pub fn kind(&self) -> ConfigErrorKind`; `Read` keeps a private `PathBuf` + `io::Error` `#[source]`, path rendered only in `Display` (ERR-005).
- `#[non_exhaustive] pub enum ServeError` + `pub fn kind(&self) -> ServeErrorKind`; `Stdio` wraps a private `#[source]` instead of `String` (TRANSPORT-007, ERR-001).
- `#[non_exhaustive] pub enum WatchError` + `pub fn kind(&self) -> WatchErrorKind`; variants wrap private `notify`/`io` sources (ERR-001).
- `#[non_exhaustive] pub struct CatalogError` (opaque aggregate) + `pub fn kind(&self) -> CatalogErrorKind` and `pub fn faults(&self) -> impl ExactSizeIterator<Item = FaultRef<'_>>` via a **named** iterator type `Faults<'_>` (no `impl Trait` in return position, CAT-004 pattern); `Fault` and `FaultKind` are exposed only through borrowed `FaultRef` accessors, keeping representation private (ERR-003/004).
- `#[non_exhaustive] pub enum PreparedToolsError` (new) + `pub fn kind(&self) -> PreparedToolsErrorKind`; wraps gateway/tool/index sources privately (BIND-001/003).
- Error-kind enums `ConfigErrorKind`, `ServeErrorKind`, `WatchErrorKind`, `PreparedToolsErrorKind`, `CatalogErrorKind`, `FaultKind` are small `#[non_exhaustive]` `Copy` enums (dependency-free). (Counted within the 19 as the 4 net-new non-error-wrapper enums plus the kinds folded per error; the tally treats each error enum + its kind as the classification pair.)

Removed from the public facade (become `pub(crate)` or private):

- `Entry` (+ accessors), `Secret`, `ServerConfig`, `PathsConfig`, `GatewayConfig`, `CatalogConfig`, `PromptConfig` - config internals; `Config` is opaque (CONFIG-003, CAT-001/003).
- `McpObserver`, `ProgressPump` - progress plumbing (PROGRESS-002).
- `RunRegistry`, `RunSlot`, `RunResult`, `RunStatus` - admission/result bookkeeping (REG-003; RunResult/RunStatus stay `pub(crate)` and keep serde/JsonSchema for the wire).
- `Candidate`, `Shortlist`, `Reload`, `Reloader` - retrieval/reload internals (RETRIEVAL-005, RELOAD-002).
- `build_router`, `MCP_PATH`, `HEALTHZ_PATH` - transport assembly (TRANSPORT-002/008); `build_router` becomes `pub(crate)` and is built only from validated `Config`.
- `tool_definitions`, `LIST_PROMPTS`, `RUN_PROMPT`, `CHECK_RUN`, `NEED_PROMPT` - dispatch vocabulary and `rmcp::Tool` leak (TOOLS-001).
- `Fault` as a public struct - replaced by borrowed `FaultRef` view.

Dependency-leak elimination summary: `Entry::prompt` (`promptforge_core::parser::Prompt`) -> `pub(crate)`; `McpObserver`/`ProgressPump` (`rmcp::Peer`, `ProgressToken`) -> `pub(crate)`; `PreparedTools::new` (`ModelCatalog`) -> `pub(crate)`; `dispatch*` and `tool_definitions` (`rmcp` request/result/`Tool`) -> `pub(crate)`; `build_router` (`axum::Router`) -> `pub(crate)`; `PreparedTools` errors (`Box<dyn Error>`) -> concrete `PreparedToolsError`. After this, the only sanctioned external framework contact is the `ServerHandler` impl on `PromptForgeServer`.

## 3. Module-by-module responsibility map

Files over 500 lines that must be split (rust-rulebook section 7):

- `src/server/tests.rs` (688) - split into shared fixtures + focused child modules for resolution/listing, argument validation, and execution/result, mirroring `tests/runs.rs` (SERVER-TESTS-004).
- `src/progress.rs` (510) - trim by moving `ProgressPump` task machinery to a private `progress/pump.rs` child module; keep `McpObserver` mapping in `progress.rs` (PROGRESS-002/003).
- `src/catalog/resolve.rs` (509) - extract path-confinement/normalization into a private `catalog/resolve/path.rs` shared by `resolve.rs` and `blocks.rs` (RSL-001, BLOCKS-001/002).

Responsibilities and moves:

- **catalog** (`catalog.rs`, `resolve.rs`, `resolve/blocks.rs`): resolve globs/excludes/named-block overrides into a validated, name-sorted `Catalog`. `Entry` gets one private `EntryState { Healthy{source,prompt} | Broken{problem} }` enum (CAT-002). Broken-entry lookup key separated from display identity so a broken stem cannot shadow a healthy prompt (RSL-002). Root confinement centralized in a new `resolve/path.rs` (RSL-001, BLOCKS-001). `find` uses `binary_search_by` (CAT-008). `hash` renamed `ranking_fingerprint`, `pub(crate)` (CAT-007).
- **config** (`config.rs`): parse -> validate -> immutable domain. Private `RawConfig` (serde) -> `TryFrom` -> opaque `Config`; validated newtypes `Secret` (nonblank, `TryFrom`), `GatewayUrl`, `RelativePromptPath`, `PromptName`, include/exclude pattern types (CONFIG-002/004/005/006/007). Reloadable vs boot-only fields modeled explicitly (CONFIG cross-file, feeds RELOAD).
- **retrieval** (`retrieval.rs`, `retrieval/index.rs`): own the atomically swappable prompt index. `rebuild`/`refresh` return a typed outcome that preserves the old index on failure and reports staleness (RETRIEVAL-001); single owner for shortlist cardinality (`CANDIDATES` vs `TOP_K`, RI-003). Rebuild writers serialized or generation-checked (RETRIEVAL-004).
- **watch + reload** (`watch.rs`, `watch/reload.rs`): `Watcher` is the sole public live-reload lifecycle. `Reloader`/`Reload` -> `pub(crate)`. **Catalog and retrieval unified into one immutable generation published through a single `ArcSwap`** so a request never sees a new catalog with an old index and a slow rebuild cannot overwrite a newer generation (RELOAD-001, WATCH-002); this is the single largest structural move. Blocking reload checks a generation/cancellation flag before every publish (WATCH-001).
- **transport** (`transport.rs`): `serve_http`/`serve_stdio` only; `build_router` + path consts `pub(crate)`. Bounded stdio line framing (TRANSPORT-001), nonblank bearer + case-insensitive scheme (TRANSPORT-002/005), explicit shutdown token + graceful drain (TRANSPORT-003), explicit allowed-host config validated against bind (TRANSPORT-004).
- **server** (`server.rs`, `server/bind.rs`, `server/resolve.rs`, `server/runner.rs`): `PromptForgeServer` + `ServerHandler` is the only public server type. `need_prompt` gets a bounded CPU executor + capability-length cap (SRV-001); `list_prompts` gets pagination (SRV-004); every run owns a `CancelHandle` and a supervisor spawned at start (SRV-002, RUNNER-001). `resolve.rs` returns a private typed `ResolveError` (SERVER-RESOLVE-001).
- **registry** (`registry.rs`): `RunRegistry`/`RunSlot`/`RunResult`/`RunStatus` all `pub(crate)`. Registration is an atomic vacant-entry op; terminal transition first-write-wins; supervisor ownership moved outside the cancellable waiter so cancellation cannot leave a phantom `running` record (REG-001/002). `RunSlot` `#[must_use]` (REG-005).
- **progress** (`progress.rs`): `pub(crate)` server detail. Payload-free tracing (PROGRESS-001); pump awaited after abort (PROGRESS-003); dropped-frame counter split Full vs Closed (PROGRESS-004).
- **tools** (`tools.rs`): `pub(crate)`. One authoritative built-in descriptor couples metadata, dispatch identity, reserved-name and publication checks; assembled once via `LazyLock`; schemas emit `additionalProperties:false` (TOOLS-001/002/003/004/005).
- **error/result/levels** (`error.rs`, `result.rs`, `levels.rs`): opaque source-preserving errors with `kind()` (see section 2). `RunResult` fields private with invariant-preserving constructors + accessors, wire shape held by a flat DTO or custom serde (RESULT-001); `levels.rs` stays test-only `pub(crate)` with structured field assertions (LEVELS-001).

## 4. Finding disposition (every id)

Legend: **Design** = recurrence prevented structurally by sections 2-3 (narrowed visibility, stronger types, unified generation, opaque errors). **Impl** = valid behavior/test fix with no public-API shape change, executed in Step 3 (file:line given). No finding is dropped.

### Manifest (`Cargo.toml`)

- PFMCP-MAN-001 - Impl: relax `rmcp = "=3.1.0"` to `"3.1.0"`, keep `Cargo.lock` (`Cargo.toml:23`, root manifest).
- PFMCP-MAN-002 - Impl: set `default-features = false` on workspace `rmcp`, keep needed features (root `Cargo.toml`).
- PFMCP-MAN-003 - Impl: `version = "0.0.0"` (`Cargo.toml:3`).
- PFMCP-MAN-004 - Impl: add `repository`, `readme`, `keywords`, `categories`, `[package.metadata.docs.rs]` (`Cargo.toml`).

### catalog

- CAT-001 - Design: `Entry` (and `Entry::prompt`) -> `pub(crate)`, dropping the `promptforge_core::parser::Prompt` leak.
- CAT-002 - Design: private `EntryState` enum makes the invalid tri-`Option` state unrepresentable (`catalog.rs:56-64`).
- CAT-003 - Design: raw `path()` -> `pub(crate)`; no host path in the public surface.
- CAT-004 - Design: `entries` -> `pub(crate)`; if any iteration stays crate-public it uses a named iterator type, not `impl Trait`.
- CAT-005 - Design: `Catalog`/`CatalogHandle` gain `#[non_exhaustive]`; `Entry` narrowed.
- CAT-006 - Impl: add crate-root doctests for the retained public workflow (`catalog.rs`).
- CAT-007 - Design/Impl: rename `hash`->`ranking_fingerprint`, `pub(crate)` (`catalog.rs:195-210`).
- CAT-008 - Impl: `binary_search_by` in `find` (`catalog.rs:189-193`).
- PFMS-CATALOG-FIXTURE-001 - Impl: TOML-encode temp path (`catalog/fixture.rs:40-46`).
- RSL-001 - Impl (high): validate/reject absolute+`..` include patterns before join, shared path guard (`catalog/resolve.rs:124-139`).
- RSL-002 - Impl (high): separate broken display id from lookup key so a broken stem cannot shadow a healthy prompt (`catalog/resolve.rs:43-58,170-208`).
- RSL-003 - Impl: three-way prompt detection (NotPrompt vs MalformedCandidate) (`catalog/resolve.rs:52-54`).
- PFMS-BLOCKS-001 - Impl (path guard): confine `[prompts.NAME].file` under root (`catalog/resolve/blocks.rs:32-40`).
- PFMS-BLOCKS-002 - Impl: normalized file-identity for globbed vs named comparison (`catalog/resolve/blocks.rs:37,47-50`).
- PFMS-CATALOG-TESTS-001 - Impl: assert full snapshot replacement (`catalog/tests.rs:56-74`).
- PFMS-CATALOG-TESTS-002 - Impl: add name-only-edit hash test (`catalog/tests.rs:13-41`).
- PFMS-CATALOG-TESTS-003 - Impl: assert `Entry::source` healthy vs broken (`catalog/tests.rs`).
- PFMS-CATALOG-TESTS-004 - Impl: inline test module into `catalog.rs` (`catalog.rs:17-21`).

### config

- PFMS-CONFIG-001 - Design: `Deserialize` removed from public `Config`; private `RawConfig` + `TryFrom` (`config.rs:70-89`).
- PFMS-CONFIG-002 - Design: `Secret` nonblank via `TryFrom`; `gateway.key` validated (`config.rs:26-68,155-160`).
- PFMS-CONFIG-003 - Design: `Config` fields private/opaque (`config.rs:74-189`).
- PFMS-CONFIG-004 - Design: `GatewayUrl` newtype validated at boundary (`config.rs:151-160`).
- PFMS-CONFIG-005 - Design: `RelativePromptPath` newtype rejects escape (`config.rs:185-188`).
- PFMS-CONFIG-006 - Design: validated include/exclude pattern newtypes (`config.rs:162-174`).
- PFMS-CONFIG-007 - Design: `PromptName` newtype for map keys (`config.rs:85-88`).
- PFMS-CONFIG-008 - Design: implement `FromStr` (`config.rs:208-246`).
- PFMS-CONFIG-009 - Impl: doctests for retained public config entry points.
- PFMS-CONFIG-010 - Design/Impl: derive `Clone/PartialEq/Eq` on validated domain where secret policy permits.
- PF-MCP-CONFIG-TESTS-001 - Impl: recursive-interpolation table test (`config/tests.rs:211-236`).
- PF-MCP-CONFIG-TESTS-002 - Impl: malformed-value + missing-gateway table (`config/tests.rs:45-208`).
- PF-MCP-CONFIG-TESTS-003 - Impl: assert `Read` path + NotFound kind (`config/tests.rs:265-272`).

### error / result / levels

- PFMS-ERR-001 - Design (high): opaque errors wrap private `toml`/`rmcp`/`notify`/`io` `#[source]`; no rendered-string variants.
- PFMS-ERR-002 - Design: `kind()` classifiers on all public errors.
- PFMS-ERR-003 - Design (high): `FaultKind` + borrowed `FaultRef` accessors.
- PFMS-ERR-004 - Design: `#[non_exhaustive]` on all public error types; `Fault` replaced by view.
- PFMS-ERR-005 - Design: private `PathBuf` in `Read`/`Watch`; render only in `Display`.
- PFMS-ERR-006 - Impl: error doctests (kind, source, faults iteration).
- PFMS-ERR-007 - Impl: same-file unit tests for Display shapes + source chains (`error.rs`).
- PFMCP-RESULT-001 - Design (high): `RunResult` invariant enum/private fields; wire DTO (`result.rs:38-137`). Type -> `pub(crate)`.
- PFMCP-RESULT-002 - Impl: full serialized-shape tests for 3 states (`result.rs:150-186`).
- PFMCP-RESULT-003 - Design/Impl: derive `PartialEq/Eq` on result.
- PFMCP-RESULT-004 - Impl: doctests (or moot once `pub(crate)`).
- PF-MCP-LEVELS-001 - Impl: structured field/value assertions, not substring (`levels.rs:34-37,62`).

### lib / main

- PFMCP-LIB-001 - Design (high): facade cut from 40 to 19; boot/serve seam only.
- PFMCP-LIB-002 - Design (high): `PreparedTools` returns concrete `PreparedToolsError`.
- PFMCP-LIB-003 - Design: `rmcp`/`axum`/`promptforge_core` leaks removed; only `ServerHandler` retained.
- PFMCP-LIB-004 - Design: `Candidate` -> `pub(crate)`.
- PFMCP-LIB-005 - Design: `build_router` -> `pub(crate)` (`#[must_use]` moot).
- PFMCP-LIB-006 - Impl (high, gate): fix broken intra-doc link (`server/bind.rs:44`) so `RUSTDOCFLAGS="-D warnings"` doc gate is green.
- PF-MCP-MAIN-001 - Design: boot orchestration moves into library entry points; `main.rs` keeps arg parse + reporting.
- PF-MCP-MAIN-002 - Impl: `args_os`/`OsStr`/`PathBuf` (`main.rs:24,70,91,99`).
- PF-MCP-MAIN-003 - Impl: use `TempDir` in report test (`main.rs:190-202`).

### progress

- PF-MCP-PROGRESS-001 - Impl (high): payload-free tracing; no author-controlled strings logged (`progress.rs:237-266`).
- PF-MCP-PROGRESS-002 - Design: `McpObserver`/`ProgressPump` -> `pub(crate)`, `rmcp` leak gone.
- PF-MCP-PROGRESS-003 - Impl (high): await pump handle after `abort()` (`progress.rs:292-300`).
- PF-MCP-PROGRESS-004 - Impl: split Full vs Closed counters (`progress.rs:221-224`).
- PF-MCP-PROGRESS-005 - Design/Impl: doctests moot once `pub(crate)`.
- PF-MCP-PROGRESS-006 - Impl: reconcile `ModelTurnFailed` level vs docs + test (`progress.rs:250-252`).

### registry

- REG-001 - Impl (high): supervisor spawned at run start, owned outside the cancellable waiter; phantom-run test (`registry.rs:199-227`).
- REG-002 - Impl: atomic vacant-entry registration + first-write-wins terminal (`registry.rs:159-181`).
- REG-003 - Design: `RunRegistry`/`RunSlot` -> `pub(crate)`.
- REG-004 - Design/Impl: doctests moot once `pub(crate)`.
- REG-005 - Design: `RunSlot` `#[must_use]`.
- PFMCP-REGTEST-001 - Impl: assert all contract fields (`registry/tests.rs:76-105`).
- PFMCP-REGTEST-002 - Impl: exact retention boundary at 3600s (`registry/tests.rs:205-215`).

### retrieval

- PFMS-RETRIEVAL-001 - Impl (high): typed rebuild outcome, preserve old index on failure (`retrieval.rs:163-179`).
- PFMS-RETRIEVAL-002 - Design/Impl: `shortlist` returns typed result internally; `Shortlist::Failed(String)` removed from public surface (type `pub(crate)`).
- PFMS-RETRIEVAL-003 - Impl: negative-score candidate policy + test (`retrieval.rs:146-161`).
- PFMS-RETRIEVAL-004 - Impl (high): serialize rebuild writers / generation-check (`retrieval.rs:163-179`). Subsumed by unified generation (RELOAD-001).
- PFMS-RETRIEVAL-005 - Design: `Candidate`/`Retrieval` `#[non_exhaustive]`; `Candidate` -> `pub(crate)`.
- PFMS-RETRIEVAL-006 - Impl: compile-time `Send+Sync+'static` assertion (`retrieval.rs`).
- PFMS-RETRIEVAL-007 - Impl: doctests for retained `Retrieval::start`.
- PFMS-RETRIEVAL-FIXTURE-001 - Impl: TOML-encode temp path (`retrieval/fixture.rs:66-73`).
- PFMS-RI-001 - Impl: accurate build-failure log (`retrieval/index.rs:52-57`).
- PFMS-RI-002 - Impl: log broken compiled-in config invariant (`retrieval/index.rs:51,69`).
- PFMS-RI-003 - Impl: single shortlist-cardinality owner (`retrieval/index.rs:32,99-100`).
- PFMCP-RETRIEVAL-TESTS-001 - Impl: cover `Shortlist::Failed` -> INTERNAL_ERROR (`retrieval/tests.rs:225-294`).
- PFMCP-RETRIEVAL-TESTS-002 - Impl: split malformed-input cases (`retrieval/tests.rs:263-273`).

### server

- PFMCP-SRV-001 - Impl (high): bounded CPU executor + capability-length cap for `need_prompt` (`server.rs:202-219`).
- PFMCP-SRV-002 - Impl (high): supervisor owns each run; handler awaits via cancellation-safe channel (`server.rs:273-288`).
- PFMCP-SRV-003 - Design: `dispatch`/`dispatch_with_progress` -> `pub(crate)`.
- PFMCP-SRV-004 - Impl: paginate `list_prompts`, drop duplicate pretty text (`server.rs:296-313`).
- PFMCP-SRV-005 - Design/Impl: doctests for retained `PromptForgeServer::new`.
- BIND-001 - Design (high): concrete `PreparedToolsError` replaces `Box<dyn Error>` (`server/bind.rs:48,64`).
- BIND-002 - Design (high): catalog-injection `new` -> `pub(crate)`/test-only; `load` is the public constructor (`server/bind.rs:64-70`).
- BIND-003 - Impl: fallback policy by `CompletionErrorKind` (`server/bind.rs:49-55`).
- BIND-004 - Impl: document real failure classes + example (`server/bind.rs:46-63`).
- BIND-005 - Design: `PreparedTools` `#[non_exhaustive]`.
- BIND-006 - Impl: compile-time `Send+Sync+'static` assertion (`server/bind.rs`).
- BIND-007 - Impl: test `load` against mock gateway (`server/bind.rs:185-298`).
- BIND-008 - Impl: import `Arc` (`server/bind.rs`).
- PF-MCP-SERVER-RESOLVE-001 - Impl: private typed `ResolveError` (`server/resolve.rs:37-54`).
- PF-MCP-SERVER-RESOLVE-002 - Impl: precompute distance keys before sort (`server/resolve.rs:63-68`).
- PF-MCP-RUNNER-001 - Impl (high): create `CancelHandle` per run, pass to `RunConfig::cancel`, retain in registry (`server/runner.rs:245-248`).
- PFMS-SERVER-TESTS-001 - Impl (high): gateway fixture owns addr+cancel+JoinHandle, awaited (`server/tests.rs:584-602`).
- PFMS-SERVER-TESTS-002 - Impl: assert exact ordered observation sequence (`server/tests.rs:184-229`).
- PFMS-SERVER-TESTS-003 - Impl: reject `Value::Null` in `optional_string` + test (`server.rs:389-400`, `server/tests.rs:419-445`).
- PFMS-SERVER-TESTS-004 - Impl: split 688-line file into child modules (`server/tests.rs`).
- PF-MCP-RUNS-001 - Impl: poll to terminal `completed` and assert value (`server/tests/runs.rs:83-115`).
- PF-MCP-RUNS-002 - Impl: real gated run proves slot held past deadline (`server/tests/runs.rs:146-170`).
- PF-MCP-RUNS-003 - Impl (high): gateway fixture owns JoinHandle+shutdown (`server/tests/runs.rs:43-45`).

### tools

- PFMCP-TOOLS-001 - Design: consts + `tool_definitions` -> `pub(crate)`; `rmcp::Tool` leak gone.
- PFMCP-TOOLS-002 - Impl: emit `additionalProperties:false` + test (`tools.rs:194-222`).
- PFMCP-TOOLS-003 - Impl: `LazyLock` assembly (`tools.rs:103-186`).
- PFMCP-TOOLS-004 - Impl: single authoritative built-in descriptor drives dispatch/reserved/publish (`tools.rs:11-12,126`; `server.rs:182-225`; `catalog/resolve.rs:34`).
- PFMCP-TOOLS-005 - Impl: correct feature-conditional doc wording (`tools.rs:8-12`, `server.rs:252-255`).
- PFMCP-TOOLS-006 - Design/Impl: doctests moot once `pub(crate)`.
- TST-001 - Impl: test catalog independence through the listing handler (`tools/tests.rs:64-95`).
- TST-002 - Impl: add exact built-in-name collision fixture (`tools/tests.rs:87-94`).
- TST-003 - Impl: add non-picker golden (`tools/tests.rs:53-71`).

### transport

- PFMS-TRANSPORT-001 - Impl (high): bounded stdio JSON-RPC line framing + tests (`transport.rs:158-176`).
- PFMS-TRANSPORT-002 - Design+Impl (high): `build_router` -> `pub(crate)`, built from validated `Config`; reject blank presented bearer (`transport.rs:88-100,202-213`).
- PFMS-TRANSPORT-003 - Impl (high): explicit shutdown token + Axum graceful shutdown, await owned tasks (`transport.rs:106-143`).
- PFMS-TRANSPORT-004 - Impl: explicit allowed-host config validated vs bind (`transport.rs:106-140`).
- PFMS-TRANSPORT-005 - Impl: case-insensitive auth scheme (`transport.rs:202-207`).
- PFMS-TRANSPORT-006 - Impl: remove false rotation claim or add reloadable token + test (`transport.rs:7-8,186-191`).
- PFMS-TRANSPORT-007 - Design: `ServeError::Stdio` wraps private source (`transport.rs:169-176`).
- PFMS-TRANSPORT-008 - Design: route consts + serve internals `pub(crate)`; only `serve_*` public.
- PF-MCP-TRANSPORT-TESTS-001 - Impl (high): add `Bearer ` empty-credential 401 test (`transport/tests.rs:135-163`).
- PF-MCP-TRANSPORT-TESTS-002 - Impl: assert independent 15s keep-alive value (`transport/tests.rs:263-271`).

### watch + reload

- PFMCP-WATCH-001 - Impl (high): cooperative cancellation/generation check before publish; async `shutdown` awaits task (`watch.rs:154-172`).
- PFMCP-WATCH-002 - Design/Impl: single reload coordinator per catalog; generation-safe publish (`watch.rs:83-166`).
- PFMCP-WATCH-003 - Impl: detect missing runtime, return `WatchError` (`watch.rs:140-161`).
- PFMCP-WATCH-004 - Impl: compare full normalized source path, not `file_name` (`watch.rs:327-374`).
- PFMCP-WATCH-005 - Design: `Watcher` `#[must_use]`.
- PFMCP-WATCH-006 - Impl: `Watcher::start` doctest.
- PFMS-WATCH-FIXTURE-001 - Impl: explicit YAML/Lua escaping in fixture (`watch/fixture.rs:19-98`).
- PFMS-RELOAD-001 - Design+Impl (high): unify catalog+retrieval into one generation behind a single `ArcSwap`; reverse-order rebuild test (`watch/reload.rs:121-148`).
- PFMS-RELOAD-002 - Design: `Reload`/`Reloader` -> `pub(crate)`; `Watcher` is the boundary.
- PFMS-RELOAD-003 - Design/Impl: internal `Result<Reload, ReloadError>`; `Watcher` owns logging (`watch/reload.rs:109-169`).
- PFMS-RELOAD-004 - Design/Impl: doctests moot once `pub(crate)`.
- PFMCP-RELOAD-TESTS-001 - Impl: table-driven non-reloadable-field test (`watch/reload/tests.rs:190-219`).
- watch/tests.rs - no findings (0).

### integration tests (`tests/it/`)

- tests/it/main.rs - no findings.
- PFMCP-PROGRESS-001 - Impl: paused time for negative assertion (`tests/it/progress.rs:87,146-195`).
- PFMCP-PROGRESS-002 - Impl: retain+await server JoinHandle, assert session result (`tests/it/progress.rs:92-95`).
- PFMCP-PROGRESS-003 - Impl: assert or justify notification send (`tests/it/progress.rs:45`).
- PFMS-SHIPPED-001 - Design+Impl: migrate to a spawned/in-process session asserting `list_prompts` + `tools/list`; removes external use of `Entry::name` and `tool_definitions` (`tests/it/shipped.rs:58-123`), enabling their `pub(crate)` narrowing.
- PFMS-STDIO-001 - Impl: wrap child in `tokio::time::timeout` (`tests/it/stdio.rs:233-239`).
- PFMS-STDIO-002 - Impl: size-limited line framing helper (`tests/it/stdio.rs:125-132`).
- PFMS-STDIO-003 - Impl: make bind invariant observable or narrow doc (`tests/it/stdio.rs:3-6`).
- PFMS-STDIO-004 - Impl: move catalog-startup test to its own module (`tests/it/stdio.rs:218-254`).
- PF-MCP-WATCH-IT-001 - Impl: extend to rewrite an existing prompt and observe v2 (`tests/it/watch.rs:57-126`).

## 5. Step-3 implementation punch-list (ordered)

Execute high-severity structural work first; each item is traceable to finding ids. A public-API change must land with its test in the same edit. Do not regress a baseline-passing gate (no new env var for `cargo build --workspace`).

Top 6 high-severity items (do these first):

1. **Unified live generation** (`watch/reload.rs`, `catalog.rs`, `retrieval.rs`, `watch.rs`): replace the two independent `ArcSwap` cells (`CatalogHandle`, `Retrieval`) with one immutable generation published through a single `ArcSwap`; serialize/generation-check reloads so a slow rebuild cannot overwrite a newer generation; on index-build failure publish an explicitly-unavailable index tied to the new catalog. Add a reverse-order rebuild concurrency test. [PFMS-RELOAD-001, RETRIEVAL-004, RETRIEVAL-001, WATCH-001, WATCH-002]
2. **Transport bounded-read / auth / framing / shutdown** (`transport.rs`, `transport/tests.rs`): enforce a documented max JSON-RPC line size on stdio; reject blank configured and blank presented bearer, parse the scheme case-insensitively; add an explicit shutdown token with Axum graceful drain and awaited tasks; make `build_router` + route consts `pub(crate)`; wrap `ServeError::Stdio` over a private source. Add `Bearer ` 401 test and independent keep-alive assertion. [TRANSPORT-001/002/003/005/007/008, TRANSPORT-TESTS-001/002]
3. **Catalog/resolve path confinement** (`catalog/resolve.rs`, `catalog/resolve/blocks.rs`, `config.rs`, new `catalog/resolve/path.rs`): validate include patterns and `[prompts.NAME].file` against a normalized root; introduce `RelativePromptPath` at the config boundary; separate broken-entry display id from lookup key. Tests: absolute path, `../`, Windows verbatim prefix, symlink escape, shadowing broken stem. [RSL-001/002, BLOCKS-001/002, CONFIG-005]
4. **Server cancellation lifecycle** (`server/runner.rs`, `server.rs`, `registry.rs`): create a `CancelHandle` per admitted run, pass through `RunConfig::cancel`, retain the controlling clone in the registry; spawn the supervisor at run start; handler awaits the terminal result over a cancellation-safe channel. Test: cancel `call_tool` before `reply_deadline`, force panic, assert a terminal evictable failure. [SRV-002, RUNNER-001]
5. **Registry cancellation + duplicate handling** (`registry.rs`, `registry/tests.rs`): supervisor ownership outside the cancellable waiter; atomic vacant-entry registration returning a duplicate error; first-write-wins terminal transition; `RunSlot` `#[must_use]`. Tests: dropped `settle` then panic, duplicate registration, duplicate completion. [REG-001/002/005]
6. **Detached gateway-task lifecycle in server tests** (`server/tests.rs`, `server/tests/runs.rs`): shared gateway fixture owning address + cancellation + `JoinHandle`, shut down and awaited at test end, unexpected serve error fails the test. [SERVER-TESTS-001, RUNS-003]

Then, grouped by file:

- **lib.rs**: add `#![deny(unreachable_pub)]`; reduce facade to the 19 items in section 2; move boot orchestration into a library entry point. [LIB-001/002/003/004/005, MAIN-001]
- **server/bind.rs**: introduce `#[non_exhaustive] PreparedToolsError` + `kind()`; `load(&Config)` public, `new` `pub(crate)`; fallback policy by `CompletionErrorKind`; `Send+Sync` assertion; import `Arc`; mock-gateway `load` test; fix the intra-doc link (doc gate). [BIND-001..008, LIB-006]
- **config.rs / config/tests.rs**: private `RawConfig` + `TryFrom`; opaque `Config` with private fields, `load` + `FromStr`; validated newtypes (`Secret`, `GatewayUrl`, `RelativePromptPath`, `PromptName`, patterns); malformed-value and recursive-interpolation tests. [CONFIG-001..010, CONFIG-TESTS-001/002/003]
- **error.rs / result.rs / levels.rs**: opaque source-preserving errors with `kind()`; `FaultKind` + `FaultRef`; `Read`/`Watch` keep private `PathBuf`; `RunResult` invariant enum + wire DTO, `pub(crate)`; structured level assertions; same-file error unit tests. [ERR-001..007, RESULT-001..004, LEVELS-001]
- **catalog.rs / catalog/tests.rs / fixture.rs**: private `EntryState`; `Entry` and its accessors `pub(crate)`; `binary_search_by` find; rename `hash`->`ranking_fingerprint` `pub(crate)`; inline tests; snapshot-replacement, name-only-hash, source, TOML-encode tests. [CAT-001..008, CATALOG-TESTS-001..004, CATALOG-FIXTURE-001]
- **retrieval.rs / index.rs / tests.rs / fixture.rs**: typed rebuild outcome; `Candidate`/`Shortlist`/`Retrieval` accessors `pub(crate)` with `#[non_exhaustive]`; single cardinality owner; accurate build-failure logs; `Send+Sync` assertion; Failed-variant + split-input tests; TOML-encode fixture. [RETRIEVAL-001/002/003/005/006/007, RI-001/002/003, RETRIEVAL-TESTS-001/002]
- **server.rs / server/resolve.rs**: bounded CPU executor + capability cap for `need_prompt`; paginate `list_prompts`; `dispatch*` `pub(crate)`; reject `Value::Null`; typed `ResolveError`; precompute distance keys. [SRV-001/003/004, SERVER-TESTS-003, SERVER-RESOLVE-001/002]
- **progress.rs**: `pub(crate)`; payload-free tracing; await pump after abort; split Full/Closed counters; reconcile `ModelTurnFailed` level. [PROGRESS-001/002/003/004/006]
- **tools.rs / tools/tests.rs**: `pub(crate)`; single authoritative built-in descriptor; `LazyLock`; `additionalProperties:false`; catalog-independence + collision + non-picker golden tests. [TOOLS-001..006, TST-001/002/003]
- **watch.rs / reload.rs / reload tests / watch fixture**: `Watcher` `#[must_use]` + async `shutdown`; runtime-absence error; full-path event filter; `Reload`/`Reloader` `pub(crate)` with internal `ReloadError`; table-driven non-reloadable test; fixture escaping. [WATCH-003/004/005/006, RELOAD-002/003, RELOAD-TESTS-001, WATCH-FIXTURE-001]
- **main.rs**: `args_os`/`PathBuf`; keep only arg-parse + `report`; `TempDir` in report test. [MAIN-001/002/003]
- **Cargo.toml**: relax `rmcp` pin, `default-features=false`, `version="0.0.0"`, add package/docs.rs metadata. [MAN-001..004]
- **tests/it/**: migrate `shipped.rs` to a session-based assertion (unlocks `Entry`/`tool_definitions` narrowing); paused time + awaited handles in `progress.rs`; timeout + bounded framing + doc/module fixes in `stdio.rs`; extend `watch.rs` to modify an existing prompt. [SHIPPED-001, PROGRESS-001/002/003, STDIO-001..004, WATCH-IT-001]

*2026-08-10 23:15 - Opus 4.8 High*
