# promptforge-core public API redesign

This document reconstructs the effective current public API of `promptforge-core`, specifies the smallest coherent replacement, and maps every downstream migration. It is a runtime redesign, not a facade over the old modules: the old broad surface is removed in the same sweep that installs the new one. No design element requires a new environment variable or manual provisioning for an ordinary `cargo build --workspace`; runtime env reads (`GatewayClient::from_env`) stay runtime-only, and no build script is touched.

## 1. Reconstructed current public API

`src/lib.rs` exposes 14 public modules (`cancel`, `client`, `debug`, `dialects`, `execute`, `lua`, `model`, `normalize`, `observe`, `parser`, `store`, `subst`, `tools`, `untrusted`) plus root re-exports `CancelHandle`, `Error`, `NearDuplicateDiagnostic`, `Result`, `promptforge_version`. `error`, `lua_models`, `resolve` are private; `fanout` is `pub(crate)`.

The effective reachable surface (condensed from the per-file findings):

- **cancel**: `CancelHandle::{new, cancel, is_cancelled, cancelled}`; ambient free fns `scope`, `wait_cancelled`, `is_cancelled`.
- **client**: `Message` (all fields public, raw `serde_json::Value` tool calls), `ToolSchema`, `ToolCall` (raw `Value` args), `CompletionResult`, `Completion`, `GatewayClient::{new, disabled, with_normalizer, from_env, complete}`; `Debug` leaks the bearer key.
- **debug**: `DebugCapture`, `DebugEvent` (raw `Value` payloads).
- **dialects**: `OpenAiDialect`, `Gemma3ToolCodeDialect`, `ToolDialectId` (+`tools_mode`), `ToolsMode`, `DialectEvidence` (+`new`, 4 public fields), `DetectScore(pub u8)`, `DialectRequest`, `ToolDialect` trait, `ToolDialectRegistry::{builtin, get, resolve}`.
- **execute**: `RunOptions<'a>` (public borrowed fields), `ResolutionContext<'a>` (leaks `ToolPicker`), `run`.
- **lua**: `LuaProgram`, `SectionVm`, `ToolResolver`, `ToolBinding`, `LuaToolHandle`, `LuaSectionHandle`, `LuaFanoutResult`, `ToolBindings`, `ToolCallCounts`, `ToolScope`, `ClosedScopes`, `LuaOutcome`, `run_chunk`, plus re-exported `LuaModelHandle`, `ModelInferHook` (leak `mlua`).
- **model**: `ModelId`, `ThinkingMode`, `ModelDescriptor`, `ModelNeedOpts`, `ModelInvocation`, `ModelBinding`, `CompletionOptions`, `ModelBindings`, `ModelCatalog` (+`to_picker_catalog`), `ModelRegistry`, `ModelResolver`, `ResolvedModel`, `fetch_model_catalog`, `model_picker_from`, `PickerModelResolver`, `pinned_qwen_dev_catalog`.
- **normalize**: `NormalizedTurn`, `CompletionNormalizer`, `OpenAiChatNormalizer`.
- **observe**: `Observer`, `NullObserver`, `detail` module with 64 `&'static str` constants; `observe(&self, &str, &str, &str)`.
- **parser**: `Frontmatter`, `Block`, `Section` (+`prologue/prose/epilog/is_list_only`), `Prompt` (all fields public) (+`parse`, `entry`), `promptforge_version`.
- **store**: `StoreError`, `Store`, `MemStore`, `StoreRef`.
- **subst**: `substitute(...)` (6 params, leaks `Value`).
- **tools**: `web_search` module, `WebSearch`, `ToolId`, `Tool` (`untrusted_output` defaults `false`), `ToolRegistry`.
- **untrusted**: `wrap`, `nonce`.
- **root error**: one 37-variant `Error` enum spanning every domain, `Http(Box<dyn Error>)`, `Result<T>`.

Effective downstream consumption (from the 35 call sites): the product workflow is `promptforge_version` -> `Prompt::parse` -> `execute::run` with `ResolutionContext`, `RunOptions`, `&[Arc<dyn Tool>]`, `StoreRef`. Genuine external extension points actually used are `Tool` (webfetch, core WebSearch), `Store` + `DebugCapture` (dev dump), `Observer` (mcp, dev, core-tests), the dialect *detection* triple (`DialectEvidence`/`ToolDialectRegistry`/`ToolDialectId`, used by gateway `local/mod.rs`), `GatewayClient` + wire types (`Message`/`CompletionResult`/`CompletionOptions`, used by gateway integration tests), `ModelCatalog`/`ModelDescriptor`/`ModelId`/`ThinkingMode`/`fetch_model_catalog`, and `LuaProgram::source` (core-tests parser assertions). Everything else in `lua`, `normalize`, `subst`, `untrusted`, and the dialect *dispatch* trait has no external consumer.

## 2. Proposed public API

The facade shrinks from 14 modules to 8, each owning one coherent responsibility. `lua`, `normalize`, `subst`, `untrusted`, `fanout`, `resolve`, `lua_models`, `error`, and the dialect dispatch machinery become `pub(crate)`. All public structs gain `#[non_exhaustive]`; invariant-bearing fields go private behind accessors; every public error gains a stable classifier; `serde` derives that are part of the wire contract stay on (this crate is unpublished and owns the gateway wire, so serde is an intentional mandatory responsibility, dispositioned once here rather than feature-gated).

### 2.1 Root facade (`lib.rs`)

```rust
pub use crate::cancel::CancelHandle;
pub use crate::execute::{run, ResolutionContext, RunConfig, RunLimits, RunError};
pub use crate::parser::{Prompt, ParseError, promptforge_version};

pub mod client;
pub mod debug;
pub mod dialects;
pub mod execute;
pub mod model;
pub mod observe;
pub mod parser;
pub mod store;
pub mod tools;
```

There is no crate-wide `Error`/`Result`. Each operation returns its own error; the orchestration boundary returns `RunError`.

### 2.2 execute (owner of the run)

```rust
#[non_exhaustive]
pub struct ResolutionContext<'a> { /* private */ }
impl<'a> ResolutionContext<'a> {
    pub fn new(picker: &'a ToolPicker, models: &'a ModelCatalog) -> Self;
}

#[non_exhaustive]
pub struct RunLimits { /* private */ }
impl RunLimits {
    #[must_use] pub fn new() -> Self;                       // safe non-env defaults
    #[must_use] pub fn max_tool_iterations(self, n: NonZeroU32) -> Self;   // default 24
    #[must_use] pub fn fanout_concurrency(self, n: NonZeroUsize) -> Self;  // default 8
    #[must_use] pub fn max_response_bytes(self, n: NonZeroU64) -> Self;    // default 16 MiB
    #[must_use] pub fn lua_memory_bytes(self, n: NonZeroUsize) -> Self;    // default 64 MiB
    #[must_use] pub fn lua_log_events(self, n: NonZeroU32) -> Self;        // default 1024
    #[must_use] pub fn request_timeout(self, d: Duration) -> Self;        // default 120 s
    // borrowed getters for each field
}
impl Default for RunLimits { /* == new() */ }

#[non_exhaustive]
pub struct RunConfig { /* private; owned, no lifetime */ }
impl RunConfig {
    pub fn new(execution: impl Into<String>) -> Self;
    #[must_use] pub fn observer(self, observer: Arc<dyn Observer>) -> Self;   // owned: reaches infer hook
    #[must_use] pub fn debug(self, debug: Arc<dyn DebugCapture>) -> Self;     // owned: reaches infer hook
    #[must_use] pub fn client(self, client: GatewayClient) -> Self;
    #[must_use] pub fn cancel(self, handle: CancelHandle) -> Self;            // explicit, not ambient
    #[must_use] pub fn limits(self, limits: RunLimits) -> Self;
}

pub async fn run(
    prompt: &Prompt,
    args: &str,
    resolution: ResolutionContext<'_>,
    tools: &[Arc<dyn Tool>],
    store: &StoreRef,
    config: RunConfig,
) -> Result<String, RunError>;
```

`RunConfig` replaces `RunOptions` (renamed because it is now owned, carries limits and cancellation, and no longer literal-constructible). `RunLimits` is threaded end to end: `max_tool_iterations` bounds the model/infer loop, `fanout_concurrency` bounds the `JoinSet`, `max_response_bytes`/`request_timeout` bound HTTP, `lua_memory_bytes`/`lua_log_events` are installed on every VM. Frontmatter `max_tool_iterations` still overrides the limit when present.

```rust
#[derive(Debug)]
#[non_exhaustive]
pub struct RunError { /* private: kind + #[source] narrow error */ }
impl RunError {
    #[must_use] pub fn kind(&self) -> RunErrorKind;   // Parse|Bind|Model|Tool|Store|Substitution|Lua|Cancelled|Backend|...
    #[must_use] pub fn is_cancelled(&self) -> bool;
    #[must_use] pub fn is_retryable(&self) -> bool;
}
#[non_exhaustive] pub enum RunErrorKind { Parse, Version, Binding, Completion, Tool, Store, Lua, Cancelled, Internal }
impl std::error::Error for RunError { fn source(&self) -> Option<&(dyn Error + 'static)>; }
```

Cancellation is explicit: interruption returns `RunError` with `is_cancelled() == true`, threaded into model calls, each `tool.call` (checked before every dispatch), the Lua instruction hook, and fanout arms.

### 2.3 parser

```rust
#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub struct Prompt { /* private */ }
impl Prompt {
    pub fn parse(source: &str, execution: &str, observer: &dyn Observer) -> Result<Prompt, ParseError>;
    pub fn frontmatter(&self) -> &Frontmatter;
    pub fn title(&self) -> &str;
    pub fn replay(&self) -> Option<&LuaProgram>;
    pub fn h1_blocks(&self) -> &[Block];
    pub fn sections(&self) -> &[Section];
    pub fn entry(&self) -> &Section;                 // structurally non-empty; no panic
    pub fn strip_h1_prose(&mut self);                // invariant-preserving H1 transform (replaces field mutation)
}

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frontmatter { /* private */ }
impl Frontmatter {
    pub fn name(&self) -> &str;
    pub fn description(&self) -> &str;
    pub fn promptforge(&self) -> Option<u32>;
    pub fn default_return(&self) -> Option<&str>;
    pub fn max_tool_iterations(&self) -> Option<NonZeroU32>;   // zero rejected at parse
}

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Lua(LuaProgram),
    #[non_exhaustive] Prose { text: String, loop_capable: bool },
}

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section { /* private */ }
impl Section {
    pub fn name(&self) -> &str;
    pub fn level(&self) -> u8;
    pub fn blocks(&self) -> &[Block];
    pub fn children(&self) -> &[Section];
    pub fn items(&self) -> &[String];
    pub fn prologue(&self) -> Option<&LuaProgram>;
    pub fn prose(&self) -> &str;
    pub fn epilog(&self) -> Option<&LuaProgram>;
    pub fn is_list_only(&self) -> bool;
}

/// Opaque compiled Lua; owns source + private bytecode. `compile`/`load` are crate-private.
#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub struct LuaProgram { /* private */ }
impl LuaProgram {
    pub fn source(&self) -> &str;
    pub fn source_line(&self) -> NonZeroU32;
    pub fn location(&self) -> &str;
}

#[non_exhaustive] pub struct ParseError { /* private: kind + span + optional source */ }
impl ParseError {
    pub fn kind(&self) -> ParseErrorKind;   // Frontmatter|Structure|Fence|List|Lua|Version
    pub fn span(&self) -> Option<(usize, usize)>;
}

pub fn promptforge_version(source: &str) -> Option<u32>;
```

`LuaProgram` is the single surviving public item from the old `lua` module; it is re-exported through `parser` (its compile-time owner) and no longer exposes `mlua`. Parser now enforces: first and every root heading is exactly H2 (PF-PARSER-001), unique non-empty sibling names (PF-PARSER-002), `NonZeroU32` iterations with an upper bound (PF-PARSER-004), coherent list classification (PF-PARSER-005/006), and `deny_unknown_fields` frontmatter (PF-PARSER-007).

### 2.4 tools

```rust
#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ToolId { /* private */ }
impl ToolId {
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Result<ToolId, ToolIdError>; // rejects empty/separator
    pub fn server(&self) -> &str;
    pub fn name(&self) -> &str;
}

#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputTrust { Trusted, Untrusted }

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolOutput { /* private: text + trust */ }
impl ToolOutput {
    pub fn trusted(text: impl Into<String>) -> Self;
    pub fn untrusted(text: impl Into<String>) -> Self;
    pub fn text(&self) -> &str;
    pub fn trust(&self) -> OutputTrust;
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn id(&self) -> ToolId;
    fn wire_name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> serde_json::Value;
    async fn call(&self, args: serde_json::Value) -> Result<ToolOutput, ToolError>;
    // no defaulted trust: OutputTrust is mandatory, carried in ToolOutput
}

#[non_exhaustive] pub struct ToolError { /* private */ }
impl ToolError {
    pub fn message(text: impl Into<String>) -> Self;                 // caller-facing, model-safe
    pub fn with_source(text: impl Into<String>, src: impl Error + Send + Sync + 'static) -> Self;
    pub fn kind(&self) -> ToolErrorKind;      // InvalidArguments|Backend|Transport|Cancelled|Other
    pub fn is_cancelled(&self) -> bool;
    pub fn is_retryable(&self) -> bool;
}

#[non_exhaustive] pub struct ToolRegistry<'a> { /* private */ }
impl<'a> ToolRegistry<'a> {
    pub fn new(tools: impl IntoIterator<Item = &'a dyn Tool>) -> Result<Self, DuplicateToolId>; // rejects dup ids
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn tools(&self) -> &[&'a dyn Tool];
    pub fn get(&self, id: &ToolId) -> Option<&'a dyn Tool>;
}

pub use web_search::WebSearch;   // web_search module made pub(crate); only the type is re-exported
```

`OutputTrust` in `ToolOutput` makes trust mandatory and structurally fail-closed (tools.rs F2 critical, web_search F2 critical). `Tool::call` returns a narrow `ToolError` instead of the crate-wide error (tools.rs F5). Trust flows to the untrusted-wrap boundary in `execute` (lua.rs LUA-005 addressed by wrapping any `Untrusted` value before it can reach `model:infer`).

### 2.5 store

```rust
#[non_exhaustive] #[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[non_exhaustive] NotFound { path: String },
    #[non_exhaustive] AnchorNotFound { path: String, anchor: String },
    #[non_exhaustive] AnchorAmbiguous { path: String, anchor: String, count: usize },
    #[non_exhaustive] InvalidPath { path: String, reason: PathReason },
    #[non_exhaustive] InvalidPattern { pattern: String, reason: String },
    #[non_exhaustive] Backend(/* opaque Send+Sync+'static source */),
}
impl StoreError {
    pub fn kind(&self) -> StoreErrorKind;
    pub fn is_not_found(&self) -> bool;
    pub fn path(&self) -> Option<&str>;
    pub fn backend(source: impl Error + Send + Sync + 'static) -> Self;  // ctor for downstream backends
}

pub trait Store: Send {                      // Sync bound dropped (STORE-008)
    fn write(&mut self, path: &str, contents: &str) -> Result<(), StoreError>;
    fn append(&mut self, path: &str, contents: &str) -> Result<(), StoreError>;
    fn read_lines(&self, path: &str) -> Result<String, StoreError>;
    fn read(&self, path: &str) -> Result<String, StoreError>;
    fn str_replace(&mut self, path: &str, old: &str, new: &str) -> Result<(), StoreError>;
    fn delete(&mut self, path: &str) -> Result<(), StoreError>;
    fn glob(&self, pattern: &str) -> Result<Vec<String>, StoreError>;
    fn exists(&self, path: &str) -> Result<bool, StoreError>;   // fallible (STORE-002); no default
}

#[non_exhaustive] pub struct MemStore { /* private */ }
impl MemStore { pub fn new() -> Self; }

#[non_exhaustive] pub struct StoreRef { /* private */ }
impl StoreRef {
    pub fn new(backend: Box<dyn Store + Send>) -> Self;
    pub fn memory() -> Self;
    // forwarding write/append/read_lines/read/inject/str_replace/delete/glob/exists, all -> Result<_, StoreError>
}
```

Trait boundary paths stay `&str`; `StoreRef` validates each into an internal `StorePath` (one canonical separator; rejects empty/absolute/traversal/control/reserved) before dispatch (STORE-003), and rejects empty anchors and over-limit glob patterns (STORE-006/007) with an iterative matcher (STORE-005). `exists` becomes fallible.

### 2.6 observe (typed)

```rust
pub trait Observer: Send + Sync {
    fn observe(&self, execution: &str, section: &str, event: Observation<'_>);
}

#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Observation<'a> {
    ParseStarted, ParseSucceeded, ParseFailed,
    RunStarted, RunSucceeded, RunFailed,
    SectionStarted, SectionFinished,
    ModelTurnCompleted, ModelTurnFailed, ModelTurnTruncated,
    ToolCallSucceeded, ToolCallFailed,
    // ... the remaining lifecycle kinds, one variant per prior detail:: constant ...
    FanoutArmStarted, FanoutArmFinished,
    Lua(&'a str),          // the constrained author checkpoint (was "Lua: <message>")
    Other(&'a str),        // forward-compat escape hatch
}

#[non_exhaustive] #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullObserver;
impl Observer for NullObserver { fn observe(&self, _: &str, _: &str, _: Observation<'_>) {} }
```

The 64 `&'static str` constants and the `detail` module are removed. `Observation` variants mirror them 1:1 so downstream matching is mechanical. The dead `MODEL_REPLY_EMPTY` is dropped rather than carried (observe.rs F5). Nested-inference observation loss (observe.rs F1, execute.rs F4) is fixed because `RunConfig` now owns `Arc<dyn Observer>` that the infer hook retains.

### 2.7 debug

```rust
pub trait DebugCapture: Send + Sync {
    fn on_event(&self, execution: &str, section: &str, turn_index: u32, event: DebugEvent);  // nonblocking contract
}

#[non_exhaustive] #[derive(Debug, Clone)]
pub enum DebugEvent {
    #[non_exhaustive] Request { body: serde_json::Value },
    #[non_exhaustive] Response { body: serde_json::Value, finish_reason: Option<String>, reasoning_content: Option<String> },
}
```

Signature unchanged (dev `TraceCapture` already wildcard-matches); variants gain per-variant `#[non_exhaustive]` (debug.rs F4). Docs state the sensitive-payload and nonblocking contract (debug.rs F1/F2/F5). `serde_json::Value` stays as the intentional raw-capture wire contract (debug.rs F6 dispositioned as accepted).

### 2.8 model

```rust
#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ModelId { /* private */ }
impl ModelId {
    pub const GATEWAY: &'static str;
    pub fn new(server: impl Into<String>, name: impl Into<String>) -> Result<ModelId, ModelIdError>;
    pub fn gateway(name: impl Into<String>) -> Result<ModelId, ModelIdError>;
    pub fn server(&self) -> &str;
    pub fn name(&self) -> &str;
}

#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThinkingMode { Never, Always, Switchable }

#[non_exhaustive] #[derive(Debug, Clone)]
pub struct ModelDescriptor { /* private */ }
impl ModelDescriptor {
    pub fn new(id: ModelId, description: impl Into<String>, context: NonZeroU32, thinking: ThinkingMode) -> Self;
    pub fn with_dialect(self, dialect: ToolDialectId) -> Self;
    pub fn id(&self) -> &ModelId; pub fn description(&self) -> &str;
    pub fn context(&self) -> NonZeroU32; pub fn thinking(&self) -> ThinkingMode;
    pub fn tool_dialect(&self) -> ToolDialectId; pub fn tools_mode(&self) -> ToolsMode;
}

#[non_exhaustive] #[derive(Debug, Clone)]
pub struct ModelCatalog { /* private */ }
impl ModelCatalog {
    pub fn new(models: impl IntoIterator<Item = ModelDescriptor>) -> Result<Self, ModelCatalogError>; // rejects dup ids
    pub fn empty() -> Self;
    pub fn models(&self) -> &[ModelDescriptor];
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, id: &ModelId) -> Option<&ModelDescriptor>;
    pub fn contains(&self, id: &ModelId) -> bool;
}

pub async fn fetch_model_catalog(base_url: &str, key: &str) -> Result<ModelCatalog, CompletionError>;

#[non_exhaustive] pub struct CompletionError { /* private */ }   // shared client/model transport+decode error
impl CompletionError {
    pub fn kind(&self) -> CompletionErrorKind;   // Transport|Backend|MalformedResponse|EmptyReply|Disabled|Config
    pub fn is_retryable(&self) -> bool;
    pub fn is_timeout(&self) -> bool;
    pub fn status(&self) -> Option<u16>;
}
```

Removed from the public surface: `ModelNeedOpts`, `ModelInvocation`, `ModelBinding`, `CompletionOptions` (public matrix), `ModelBindings`, `ModelRegistry`, `ModelResolver`, `ResolvedModel`, `to_picker_catalog`, `model_picker_from`, `PickerModelResolver`, `pinned_qwen_dev_catalog`. `Temperature` and non-zero `context` become validated internal newtypes (MODEL-001..006). `CompletionOptions` stays only where the gateway integration test needs it (see 2.9); it becomes a validated builder, not a public field matrix.

### 2.9 client (narrowed, validated)

```rust
#[non_exhaustive] #[derive(Clone)]
pub struct GatewayClient { /* private; Debug redacts key */ }
impl GatewayClient {
    pub fn new(endpoint: GatewayEndpoint, key: SecretString) -> Self;   // validated inputs
    pub fn disabled() -> Self;
    pub fn from_env() -> Result<Self, CompletionError>;                 // runtime env only
    pub async fn complete(&self, messages: &[Message], tools: Option<&[ToolSchema]>, options: &CompletionOptions)
        -> Result<Completion, CompletionError>;
}

#[non_exhaustive] pub struct GatewayEndpoint { /* private */ }
impl TryFrom<&str> for GatewayEndpoint { type Error = CompletionError; }

#[non_exhaustive] #[derive(Clone)] pub struct Message { /* private; validated constructors */ }
impl Message {
    pub fn user(content: impl Into<String>) -> Self;
    pub fn assistant(content: impl Into<String>) -> Self;
    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self;
    pub fn role(&self) -> Role; pub fn content(&self) -> &str;
}

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionResult { Text(String), ToolCalls(Vec<ToolCall>) }

#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq)] pub struct ToolCall { /* private; validated args */ }
#[non_exhaustive] #[derive(Debug, Clone)] pub struct ToolSchema { /* private; validated wire name + object schema */ }
#[non_exhaustive] #[derive(Debug)] pub struct Completion { /* private */ }
impl Completion { pub fn result(&self) -> &CompletionResult; pub fn finish_reason(&self) -> Option<&str>; }

#[non_exhaustive] pub struct CompletionOptions { /* private */ }
impl CompletionOptions { pub fn new(model: impl Into<String>, dialect: ToolDialectId) -> Self; /* + builder setters */ }
```

`with_normalizer` and the whole `normalize` module are removed (client F2, PF-NORM-001/007): dialect parsing is the sole path. The bearer key never appears in `Debug` (client F1, web_search F1). Bodies are size-capped (`RunLimits::max_response_bytes`) before UTF-8/JSON (client F4). Malformed tool arguments are rejected at parse instead of coerced (client F3/F8, normalize F5). `assistant_tool_calls` and raw `Value` fields leave the public API; raw echo stays a crate-private dialect concern.

### 2.10 dialects (detection only)

```rust
#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ToolDialectId { OpenAi, Gemma3ToolCode }
impl ToolDialectId { pub fn tools_mode(&self) -> ToolsMode; }
impl std::fmt::Display for ToolDialectId { /* stable strings */ }

#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsMode { Native, Emulated }
impl std::fmt::Display for ToolsMode { /* ... */ }

#[non_exhaustive] #[derive(Debug, Clone, Default)]
pub struct DialectEvidence {                 // passive evidence bag; fields stay public + readable/writable
    pub supports_tool_calls: Option<bool>,
    pub chat_template: Option<String>,
    pub model_id: Option<String>,
    pub source: Option<String>,
}
impl DialectEvidence { pub fn new(/* 4 opts */) -> Self; }

#[non_exhaustive] pub struct ToolDialectRegistry { /* private */ }
impl ToolDialectRegistry {
    pub fn builtin() -> Self;
    pub fn resolve(&self, evidence: &DialectEvidence) -> Result<ToolDialectId, DialectError>;
}

#[non_exhaustive] pub struct DialectError { /* private */ }
impl DialectError { pub fn kind(&self) -> DialectErrorKind; }   // NoMatch|Tie|Unknown
```

Made `pub(crate)`: `ToolDialect` trait, `OpenAiDialect`, `Gemma3ToolCodeDialect`, `DetectScore`, `DialectRequest`, `ToolDialectRegistry::get`. `DialectEvidence` stays a public passive evidence bag (the gateway reads and mutates `chat_template`), now `#[non_exhaustive]` so `new` remains the constructor and no external literal is possible. Detection is the only externally used dialect capability (gateway `local/mod.rs`); dispatch, parsing, and echo are internal. Call/result correlation is validated in a crate-private paired type (dialects DIALECTS-002, openai F4, gemma F13).

## 3. Removals and visibility reductions

- **Modules to `pub(crate)`**: `lua`, `lua_models`, `normalize`, `subst`, `untrusted`, `cancel` (type re-exported, free fns removed), `error`. `fanout`, `resolve` already private.
- **Removed items**: crate-wide `Error`/`Result`; `Http(Box<dyn Error>)`; `NearDuplicateDiagnostic` from root (moves to a crate-private tool-scope diagnostic); `with_normalizer`, `CompletionNormalizer`, `NormalizedTurn`, `OpenAiChatNormalizer`; `Message::assistant_tool_calls`; all raw-`Value` public fields; `subst::substitute`; `untrusted::{wrap, nonce}`; `cancel::{scope, wait_cancelled, is_cancelled}`; `ModelNeedOpts`, `ModelInvocation`, `ModelBinding`(pub ctor), `ModelBindings`, `ModelRegistry`, `ModelResolver`, `ResolvedModel`, `to_picker_catalog`, `model_picker_from`, `PickerModelResolver`, `pinned_qwen_dev_catalog`; `OpenAiDialect`, `Gemma3ToolCodeDialect`, `ToolDialect`, `DetectScore`, `DialectRequest`; all of `lua`'s handles/VM/counters/scopes/`run_chunk`/`ToolResolver`/`ModelInferHook`/`LuaModelHandle` except opaque `LuaProgram`.
- **Field privatization**: `Frontmatter`, `Section`, `Prompt`, `Message`, `ToolSchema`, `ToolCall`, `Completion`, `DetectScore`, `RunConfig`, `ResolutionContext`, all model records.
- **`#[non_exhaustive]`** added to every surviving public struct/enum and every data-carrying variant.

## 4. Responsibility moves

- Response canonicalization: from `normalize` (removed) into crate-private dialect helpers shared by both dialects (normalize F6).
- Untrusted wrapping + guard nonce: from public `untrusted` into a crate-private trust module owning one wrap-per-result with a CSPRNG nonce (untrusted 001-006), invoked by `execute` from `OutputTrust` and by the store `inject` path.
- Near-duplicate diagnostic: from root `error` into crate-private tool-scope validation (error F10).
- `pinned_qwen_dev_catalog`: from core into `promptforge-core-tests` (MODEL-016).
- Picker encoding/adapters (`model_picker_from`, `PickerModelResolver`, `to_picker_catalog`): into crate-private `resolve`/`model` internals (MODEL-013/015).
- One section-execution engine: merge `run_sections` + `run_execute_section` internals (execute F6). No public effect.

## 5. Invariants

- A `Prompt` always has >=1 top-level H2; `entry()` cannot panic. Names are non-empty and sibling-unique. `max_tool_iterations` is `NonZeroU32` within bound.
- Every `Tool` result carries an explicit `OutputTrust`; any `Untrusted` value is nonce-wrapped before it can reach model input, including the Lua `store.read`->`model:infer` path.
- Every public error exposes `kind()` plus the relevant `is_cancelled`/`is_retryable`/`is_timeout` classifier; no caller matches private variants; dependency errors live behind private `#[source]`.
- `RunLimits` bounds are honored at every unbounded site (tool loop, fanout fan, HTTP body/timeout, Lua memory/log). A clean build needs no env or provisioning.
- Cancellation is explicit and reaches tools, models, Lua, and fanout arms; missing cancellation cannot masquerade as "not cancelled" (cancel 003).
- Store paths are validated to one canonical form before any backend sees them; `exists` distinguishes absence from failure.
- `GatewayClient` never discloses its key; response bodies are bounded before parse.

## 6. Per-file downstream migration map (all 35 paths)

Legend: **CS** = call-site-only edit; **FORCED** = a forced downstream signature/impl change (a removed/renamed core type in a downstream signature, or a changed core trait the file implements).

1. `promptforge-cli/src/tools.rs` - CS: `WebSearch`/`Tool` unchanged as names; no change beyond `Tool::call` return handled internally. No forced signature (accessors return `&[Arc<dyn Tool>]`).
2. `promptforge-mcp-server/src/watch/reload/tests.rs` - CS: `ModelCatalog::empty()` unchanged.
3. `promptforge-dev/src/tools.rs` - CS: same as cli tools.rs.
4. `promptforge-core-tests/src/scenarios.rs` - **FORCED**: `impl Observer for Recorder::observe` -> `Observation<'_>`; `impl Tool for StringFixtureTool::call` -> `Result<ToolOutput, ToolError>` (+ trust); `use Error` -> `RunError`/`ToolError`; `pinned_qwen_dev_catalog` now defined locally.
5. `promptforge-mcp-server/src/retrieval/tests.rs` - CS: `ModelCatalog::empty()`.
6. `promptforge-mcp-server/src/server/bind.rs` - **FORCED** (test module): `prompt.h1_blocks.retain(..)` -> `prompt.strip_h1_prose()`; CS: `ResolutionContext::new(..)`, `RunConfig::new(..).observer(..)`, `run(..) -> RunError`; `Block`/`Prompt::parse` via accessors; the `gateway_client` Debug test string loses the key (assert redacted).
7. `promptforge-cli/src/main.rs` - CS: `RunConfig`/`ResolutionContext` builders; `run` returns `RunError`; cancellation passed via `RunConfig::cancel` instead of `cancel::scope`; **FORCED** (test module): `impl Observer for Recorder`.
8. `promptforge-mcp-server/src/server/runner.rs` - CS: `RunConfig`/`ResolutionContext` builders; `run` error type.
9. `promptforge-dev/src/run.rs` - **FORCED**: production `impl Observer::observe` -> `Observation<'_>`; CS: `RunConfig`/`ResolutionContext`, `run`/`fetch_model_catalog` error types.
10. `promptforge-dev/src/watch.rs` - CS: `cancel` free fns removed; use `CancelHandle` + `RunConfig::cancel`.
11. `promptforge-mcp-server/src/progress.rs` - **FORCED**: `impl Observer for McpObserver::observe(.., report:&str)` -> `Observation<'_>`; the `detail::` constant matching is rewritten to match `Observation` variants (mechanical, 1:1).
12. `promptforge-mcp-server/src/server/tests.rs` - CS: `ModelDescriptor::new` now takes `NonZeroU32` context, `ModelCatalog::new` fallible; `detail` -> `Observation`.
13. `promptforge-core-tests/src/suite.rs` - **FORCED**: `impl Observer for Recorder`; pub `run(..) -> Result<String>` helper -> `RunError`; `use Error` removed; `LuaProgram` now via `parser` (source() only); parser field reads -> accessors (`prompt.frontmatter().promptforge()`, `transform.prologue()`, etc.).
14. `promptforge-dev/src/dump.rs` - **FORCED**: `impl Store for MirrorStore::exists` -> `Result<bool, StoreError>`; `impl DebugCapture for TraceCapture` unchanged (already wildcard).
15. `promptforge-webfetch/src/lib.rs` - **FORCED**: `impl Tool for WebFetch::call` -> `Result<ToolOutput, ToolError>` returning `ToolOutput::untrusted(..)`; `untrusted_output` removed (trust now in output). `use {Error, Result}` removed.
16. `promptforge-gateway/tests/it/main.rs` - CS: `CompletionOptions::new(..)` builder; `Message::user`; match `CompletionResult::Text`; `GatewayClient` construction via `GatewayEndpoint`/`SecretString`.
17. `promptforge-gateway/src/local/mod.rs` - CS: none required for dialect detection (`DialectEvidence`, `ToolDialectRegistry::builtin/resolve`, `ToolDialectId::tools_mode`, `Display`, mutable `chat_template` all retained).
18. `promptforge-webfetch/src/error.rs` - **FORCED**: `impl From<FetchError> for promptforge_core::Error` -> `From<FetchError> for ToolError` (map `model_facing()` into `ToolError::message`, using `is_recoverable` for kind).
19. `promptforge-cli/Cargo.toml` - none.
20. `promptforge-dev/src/main.rs` - CS: `CancelHandle` retained; ambient `cancel` free fns removed -> build handle, pass via `RunConfig::cancel`.
21. `promptforge-gateway/src/local/artifacts.rs` - none (no core public type in signature).
22. `promptforge-gateway/Cargo.toml` - none.
23. `promptforge-core-tests/src/main.rs` - CS: wire-up to renamed helpers only.
24. `promptforge-core-tests/Cargo.toml` - none.
25. `promptforge-core-tests/src/gateway.rs` - CS: doc reference to `GatewayClient` only.
26. `promptforge-dev/Cargo.toml` - none.
27. `promptforge-mcp-server/tests/it/progress.rs` - CS: `ModelCatalog::empty()`; `detail` -> `Observation` in assertions.
28. `promptforge-mcp-server/tests/it/watch.rs` - CS: `ModelCatalog::empty()`.
29. `promptforge-mcp-server/src/transport/tests.rs` - CS: `ModelCatalog::empty()`.
30. `promptforge-mcp-server/src/tools/tests.rs` - CS: `Prompt::parse` + `NullObserver`; parser accessors.
31. `promptforge-mcp-server/src/catalog/resolve.rs` - CS: `Prompt`/`promptforge_version`/`NullObserver`; parser accessors if fields read.
32. `promptforge-mcp-server/src/transport.rs` - CS: `ModelCatalog::empty()` doc/usage.
33. `promptforge-mcp-server/src/catalog.rs` - CS: `Prompt` via accessors.
34. `promptforge-mcp-server/Cargo.toml` - none.
35. `promptforge-webfetch/Cargo.toml` - none.

**Forced downstream signature changes: 11 methods across 7 files** - `webfetch/src/lib.rs` (Tool::call, remove untrusted_output = 2), `webfetch/src/error.rs` (From target = 1), `dev/src/dump.rs` (Store::exists = 1), `dev/src/run.rs` (Observer::observe = 1), `mcp-server/src/progress.rs` (Observer::observe = 1), `core-tests/src/suite.rs` (Observer::observe + run helper = 2), `core-tests/src/scenarios.rs` (Observer::observe + Tool::call + trust = 3). Test-module `impl Observer` in `cli/src/main.rs` and the `bind.rs` test are forced impl edits but sit in `#[cfg(test)]` and are listed inline above. All other downstream edits are call-site-only; none adds a `pub` item downstream.

## 7. Disposition of every API-related finding

Mechanisms: **P** privatize/remove item; **T** stronger type / newtype / validated ctor; **NE** `#[non_exhaustive]`; **E** narrow error + classifier; **DOC** doc/example; **LIM** threaded into `RunLimits`; **CANCEL** explicit cancellation; **TEST** required test; **KEEP** accepted with rationale (rejected as change); **DEFER** owned by another crate's later loop.

- **Cargo.toml** MANIFEST-001 T(version 0.0.0); -002 DOC(add README/keywords, workspace repo); -003 KEEP/DEFER serde_yaml migration is a workspace-dependency decision (no build-gate regression allowed) - flagged, applied at workspace closure; -004 KEEP (unpublished, feature-unification tuning deferred to workspace closure, no API effect); -005 DEFER (CI/MSRV, workspace closure).
- **cancel** 001 fixed (replace hand-rolled Notify with a proven token via CANCEL); 002 CANCEL (explicit propagation into arms); 003 CANCEL+E (no silent "not cancelled"); 004 DOC; 005 TEST; 006 TEST; 007 P (free fns removed; `CancelHandle` NE).
- **client** F1 fixed (redact Debug); F2 P/E (`CompletionError`, no `with_normalizer`); F3 T (reject malformed args); F4 LIM (max_response_bytes); F5 E (bounded diagnostic); F6 T (validated `Message`/`Role`); F7 T (validated `ToolSchema`); F8 P (raw `Value` out of API); F9 derive PartialEq/Eq; F10 DOC; F11 E (exhaustive `CompletionError`); F12 T (`GatewayEndpoint`/`SecretString`); F13 E (classify env); F14 TEST.
- **debug** F1 DOC(sensitive contract); F2 DOC(nonblocking); F3 DOC(panic); F4 NE(variants); F5 DOC(ordering); F6 KEEP(raw JSON is intentional capture wire); F7 DOC; F8 KEEP(positional metadata retained; signature stable for dev impl).
- **dialects** 001 P(seal/privatize `ToolDialect`+impls; keep detection); 002 T(validated paired result, private); 003 P(`DetectScore` private); 004 P(`DialectRequest` private); 005 T(derive mode from id; wire tools_mode validated); 006 P(remove concrete re-exports); 007 KEEP `DialectEvidence::new` + public passive fields (gateway needs read/mutate) + NE; 008 KEEP serde mandatory (dispositioned once); 009 DOC; 010 perf(single scan).
- **gemma3_tool_code** F1-F14 all internal now (dialect dispatch privatized): F1/F4/F5/F9 T(one codec, escaping); F2/F3/F6/F8 T(syntax-aware parse); F7 T(monotonic ids); F10/F11 E(three-way parse result, reject malformed); F12 T(validate-before-mutate); F13 T(validated correlation); F14 T(trust carried in result type); F15/F16 P(type private); F17 split; F18/F19 fixed detection/guide. No external effect.
- **openai** F1-F4 T(preserve raw args / validated correlation, internal); F5/F6 T(detection); F7 P; F8 KEEP(Default on private type); F9 DOC; F10 TEST. Internal now.
- **error** F1 E(split into `ParseError`/`CompletionError`/`ToolError`/`StoreError`/`DialectError`/binding errors + `RunError`); F2 E(opaque `CompletionError`, `reqwest` as private source); F3 E(`ParseError` with source); F4 E(internal Lua errors, `mlua` private source); F5 E(crate-private binding error); F6 E(internal substitution error); F7 NE(per-variant on survivors); F8 DOC(message style); F9 DOC; F10 P(move diagnostic to tool-scope, drop picker fields).
- **execute** F1 CANCEL(tool dispatch); F2 CANCEL+LIM(Lua hook polls cancel); F3 fixed(remove `block_in_place` panic path or gate + DOC) - resolved by owning blocking work; F4 fixed(owned `Arc` observer/debug in `RunConfig`); F5 E(defer+return concrete gateway error); F6 refactor(one engine); F7 NE+builders(`RunConfig`/`ResolutionContext`); F8 KEEP(`ToolPicker` is intentional workspace-internal interop, documented, `ResolutionContext::new`); F9 DOC; F10 TEST(Send/Sync); F11 E(timestamp).
- **execute/tests** 001-006 TEST (fix fixtures; owned mock-server guard; real resolver path; count via loop; split file).
- **fanout** 001 LIM(fanout_concurrency + bounded channels); 002 T(heading address parser shared with parser); 003 TEST(in-flight cancel); 004 fixed(balanced terminal telemetry); 005 E(retain JoinError source); 006 refactor(scope guard); 007 split; 008 T(non-wrapping turn counter). Internal.
- **lib** F1 P(facade); F2 fixed(coherent root re-exports); F3 P(remove `serde_json`/`mlua`/picker from public API except documented interop); F4 E(no crate-wide `Error`); F5 NE; F6 T(typed `Observation`); F7 P/seal(traits); F8 P(lua machinery private); F9 DOC(entry-point example); F10 P(`normalize`/`subst`/`untrusted` private).
- **lua** 001 LIM(memory); 002 LIM(log events); 003 P(`set_global_string` private); 004 T(`NonZeroU32` source line); 005 T(trust provenance to model input); 006 E(poison); 007 fixed(cleanup guard); 008 fixed(atomic scope close); 009 P(module private); 010 NE(only `LuaProgram` survives); 011 P(no `mlua` in API); 012 E(no stringly callbacks); 013 DOC; 014 split; 015 DOC(source sensitivity). Only `LuaProgram` (source/source_line/location) stays public via `parser`.
- **lua_models** PF-LM-001 P(`ModelInferHook` private); -002 T(reject/parse infer opts); -003 T(atomic always); -004 T(finite `Temperature`); -005 T(unified numeric validation); -006 T(private phased state); -007 fixed(validate-before-close); -008 DOC; -009 P; -010 P; -011 T(`dialect()` returns id); -012 E; -013 split; -014 TEST. All internal.
- **model** 001 fixed(drop bad `Eq`); 002 T(`Temperature`); 003 T(`NonZeroU32`); 004 T(fallible `ModelId`); 005 E(`ModelCatalog::new` rejects dup); 006 T(atomic binding, private); 007 E(mismatch error, private); 008 T/E(validate wire tools_mode); 009 E(malformed->`CompletionError`); 010 E(preserve body-read source); 011 P/NE(records private); 012 DOC; 013 P(picker adapters private); 014 P(remove `ModelRegistry`; add `contains`); 015 P; 016 P(move fixture to core-tests); 017 perf(borrowed filter, internal); 018 perf(shared client, internal); 019 split; 020 fixed(`#[expect]`/remove).
- **normalize** 001 P(remove normalizer, dialect is sole path); 002/003/004/005 T(trim-aware emptiness, validate shapes/ids/args - in private dialect helpers); 006 refactor(shared helpers); 007 P(remove overlap); 008 DOC; 009 derive. Module removed from API.
- **observe** F1 fixed(owned observer reaches infer); F2 DOC/T(labels opaque or documented untrusted); F3 DOC(one honest boundary) + KEEP report-only prose with note; F4 T(`Observation` enum); F5 P(drop dead constant); F6 DOC; F7 TEST; F8 DOC(state machine) + T(terminal variants).
- **parser** 001 T(H2-only roots); 002 T(unique non-empty names); 003 T(private fields + accessors + `strip_h1_prose`, non-panicking `entry`); 004 T(`NonZeroU32` bounded); 005 T(list classification) ; 006 T(shared marker classifier); 007 T(`deny_unknown_fields`); 008 E(`ParseError`); 009 NE(Prose variant); 010 DOC; 011 derive Eq; 012 split.
- **resolve** F1 fixed(short lock); F2 P(drop unused diagnostics); F3 E(resolver-state error, not `Lua`); F4 perf(normalize once); F5 TEST(cache); F6 must_use; F7 perf(rebuild from filtered). Internal.
- **store** 001 E(`Backend` variant + ctor); 002 T(`exists -> Result<bool>`); 003 T(`StorePath` validation in `StoreRef`); 004 E(poison -> unavailable); 005 fixed(iterative matcher) + LIM; 006 E(`InvalidPattern`); 007 E(reject empty anchor); 008 fixed(drop `Sync`); 009 TEST(Send/Sync); 010 E(`kind`/`is_not_found`/`path`); 011 split.
- **subst** 001 T(trust context, internal); 002 P(module private); 003 T(escape grammar); 004 T(validate path segments); 005 E(bounded diagnostic); 006 DOC/TEST. Module removed from API.
- **tools** F1 T/DEFER(single canonical `ToolId` at picker boundary re-exported - coordinated, but core keeps its `ToolId` facade; conversion centralized); F2 T(mandatory `OutputTrust` in `ToolOutput`); F3 E(`ToolRegistry::new` rejects dup); F4 T(fallible `ToolId::new`; validated wire name); F5 E/T(narrow `ToolError` + `ToolOutput`; split descriptor from execution kept minimal); F6 TEST(Send/Sync); F7 NE; F8 DOC; F9 perf; F10 perf.
- **tools/web_search** F1 fixed(redact Debug); F2 T(`OutputTrust::Untrusted`); F3 T(validated request); F4 T(fallible ctor/`GatewayEndpoint`); F5 LIM(bounded body); F6 E(preserve source); F7 E(sanitized diagnostic); F8 DOC; F9 TEST; F10 LIM(timeout); F11 NE. (WebSearch lives in core; fixed in-crate.)
- **untrusted** 001 T(complete encoder); 002 T(`GuardNonce`/owned nonce); 003 T(CSPRNG); 004 T(per-result nonce); 005 fixed(preface); 006 P(module private); 007 DOC; 008 TEST.

## 8. Required tests and docs

- Cancellation: before batch, during a slow tool, between calls, mid-fanout, and Lua compute under cancel (execute F1/F2, fanout 003, cancel 005/006).
- Trust: every concrete `Tool` asserts its `OutputTrust`; execution-level test that `Untrusted` output is nonce-wrapped before `model:infer` (tools F2, web_search F2, lua 005, untrusted 008).
- Limits: response-byte cap, fanout concurrency cap, Lua memory/log quota each fail with a typed error (client F4, fanout 001, lua 001/002).
- Errors: `RunError`/`CompletionError`/`StoreError`/`ParseError`/`ToolError` classifier tests; wildcard-match doctests (all error findings, section 10).
- Wire validation: malformed tool arguments rejected; oversize/malformed bodies rejected; redacted `Debug` (client F1/F3/F4, web_search F1/F5).
- Parser: H3-only reject, duplicate/empty names, zero/overflow iterations, unknown frontmatter key, list classification (PF-PARSER-001..007).
- Compile-time `Send + Sync` assertions for `RunConfig`, `StoreRef`, `ToolRegistry` (execute F10, store 009, tools F6).
- `# Examples` doctests on every surviving public item (no-network / `no_run` for I/O).

## 9. Compatibility decisions

- `ToolPicker` in `ResolutionContext` is an accepted workspace-internal interoperability commitment (not a third-party leak); documented, `ResolutionContext::new` gates it (execute F8, model F13 for adapters privatized).
- `serde` stays a mandatory dependency and derive set: this unpublished crate owns the gateway wire; dispositioned once rather than feature-gated (dialects 008).
- `serde_json::Value` remains only in `Tool::parameters_schema`/`Tool::call(args)` and `DebugEvent` as intentional JSON-schema and raw-capture contracts; removed everywhere else (lib F3, debug F6, tools F5).
- `LuaProgram` is the single retained public item from the old `lua` module, re-exported through `parser`, `mlua`-free.
- `serde_yaml` migration and Tokio feature trimming are workspace-level and applied at workspace closure so no baseline `cargo build --workspace` gate regresses.

## 10. Implementation order (one whole-crate sweep)

1. New error types (`ParseError`, `CompletionError`, `ToolError`, `StoreError`, `DialectError`, binding errors, `RunError`) with classifiers; delete crate-wide `Error`/`Result`.
2. `tools` (`OutputTrust`/`ToolOutput`, fallible `ToolId`/`ToolRegistry`) and fix in-crate `WebSearch`.
3. `store` (fallible `exists`, `StorePath`, backend variant, iterative glob, drop `Sync`).
4. `client`/`model` validated types + bounded/redacted transport; remove `normalize`, model adapters/fixtures.
5. `dialects`: privatize dispatch, keep detection; internal codec + correlation.
6. `parser`: private fields + accessors, opaque `LuaProgram`, validation; privatize `lua`/`lua_models`/`subst`/`untrusted`.
7. `observe` typed `Observation`; `debug` NE variants.
8. `execute`: `RunConfig`/`RunLimits`/`ResolutionContext`, thread limits + explicit cancellation, owned instrumentation, one engine; privatize `cancel` free fns.
9. `lib.rs` facade re-exports.
10. Migrate the 35 downstream paths per section 6 (call-site-only except the 11 listed forced signatures); move `pinned_qwen_dev_catalog` into core-tests.
11. Add tests/docs (section 8); run scoped section 12 gates + `cargo build --workspace` to green.

## 11. Reconciliation with the shipped public surface

This section reconciles `cargo public-api -p promptforge-core` (exit 0) at branch `wip/core-redesign` against sections 1-10. Section 2 states the original intent; where the implemented redesign legitimately evolved past it, the shipped item is declared intentional here and this section governs. Every public item is now either declared in section 2 or recorded below; nothing in the surface is accidental. No item was privatized in this pass because each unexpected item carries a coherent redesign rationale; the reductions in section 3 that were already applied stand.

### 11.1 Root facade re-exports (supersedes 2.1)

The root re-exports the paired classifiers and the two most-handled boundary error types alongside their base types, for caller ergonomics:

```rust
pub use crate::cancel::CancelHandle;
pub use crate::dialects::{DialectError, DialectErrorKind};
pub use crate::execute::{run, ResolutionContext, RunConfig, RunLimits, RunError, RunErrorKind};
pub use crate::model::{CompletionError, CompletionErrorKind};
pub use crate::parser::{Prompt, ParseError, ParseErrorKind, promptforge_version};
```

`RunErrorKind`, `ParseErrorKind`, `DialectErrorKind`, `CompletionError`, `CompletionErrorKind` are intentional additions to the facade (each pairs with a base type already exported or is a primary error a host branches on). The 9 public modules match section 2.1's code block exactly (the "8" in the prose is a miscount).

### 11.2 execute (augments 2.2)

- `RunErrorKind` adds `Quota` (Lua host-resource exhaustion) and `Substitution` (`{{ }}` prose failure) to the section-2.2 set; both are real, classifiable run failures. Kept and declared.
- `RunLimits` carries an additional bound `max_fanout_items` (with getter `fanout_items`) beside `fanout_concurrency`; it caps the number of fanout arms, distinct from their concurrency. Kept.
- `RunLimits` exposes borrowed getters `tool_iterations`, `fanout`, `response_bytes`, `lua_memory`, `lua_logs`, `timeout` (section 2.2 declared "borrowed getters for each field" generically; these are their names).
- `RunConfig` exposes getters `execution` and `limits_ref` for the nested/observability paths. Kept.

### 11.3 parser (supersedes 2.3 for iterations)

- `MaxToolIterations` (with `Default`, `Limit`, `resolve`, `limit`) and the public `MAX_TOOL_ITERATIONS` bound are the shipped representation of the iteration cap. `Frontmatter::max_tool_iterations()` returns `MaxToolIterations`, not `Option<NonZeroU32>`. This is a legitimate enrichment (it carries both the default and the parsed override with a resolve step); it supersedes the section-2.3 signature. Kept and declared.
- `ParseErrorKind::Version` is superseded: an unsupported `promptforge:` major is refused at the run boundary as `RunErrorKind::Version` (via `Error::UnsupportedVersion`), not classified as a parse-kind. `ParseErrorKind` therefore does not carry `Version`. Design updated to drop it.

### 11.4 tools (supersedes 2.4 for the dup error)

- `ToolRegistryError` + `ToolRegistryErrorKind` supersede the designed `DuplicateToolId`: the registry is the schema/transport boundary, so besides a repeated identity it also rejects an illegal `wire_name`. `ToolRegistry::new` returns `Result<Self, ToolRegistryError>`. `DuplicateToolId` is renamed away. Declared.
- `ToolIdError` exposes `kind() -> ToolIdErrorKind` and `field()` (satisfies the section-5 classifier invariant). `ToolError::with_kind` is an additional typed constructor. `WebSearch::new` is the re-exported type's constructor. All kept.

### 11.5 client / model (supersedes 2.8-2.9 where noted)

- `Message::role()` returns `&str` (the wire role verbatim), not a `Role` enum. `Role` is superseded: the validated constructors (`user`/`assistant`/`tool`) already gate role creation, so a separate public enum adds surface without adding an invariant. Design updated; `Role` is not part of the surface.
- `CompletionOptions` is a validated builder exposed from `model` (its canonical location, used by the gateway integration test), not `client`. Design updated to record `model` as its location; `client::complete` still consumes it.
- `ToolSchema` stays an opaque public wire type (it appears in `GatewayClient::complete`'s signature), but its raw-`serde_json::Value` constructor `ToolSchema::new` and its `ToolSchemaError` are now `pub(crate)` (client F8, lib F3): `ToolSchema` is built only inside the crate from the `Tool::parameters_schema` contract, so raw JSON never appears in a public constructor. This reduces the surface below the earlier reconciliation.
- Kept and declared: `SecretString::new` + `SecretError`; `ToolArguments` (+ `to_json_string`/`is_empty`/`contains`/`names`); `ToolCall::{id,name,arguments}`; `GatewayEndpoint::{new,url}` + `TryFrom<&str>`; `GatewayClient::with_request_limits` (threads `RunLimits`); `Completion::reasoning_content` (payload-free side-channel accessor); `TemperatureError`; `CompletionError::backend_body` (opt-in bounded diagnostic matching the private `Backend { body }`).

### 11.6 dialects / store / observe / debug (augments 2.5-2.7, 2.10)

- `dialects`: `DialectEvidence::supports_tool_calls_authoritative` (public field) and `authoritative_tool_support()` distinguish an authoritative tool-support signal from an inferred one; the evidence bag is a public passive struct by design, so the extra field is kept and declared.
- `store`: `StoreError::InvalidAnchor` + `StoreErrorKind::InvalidAnchor` (rejecting an empty anchor, STORE-007). Kept.
- `observe`: `Observation::label()` returns the event's stable label. Kept.
- `debug`: `DebugEvent::request` / `DebugEvent::response` convenience constructors. Kept.

### 11.7 Error-classifier invariant (scopes section 5)

The section-5 "every public error exposes `kind()`" invariant is scoped to the boundary errors a host branches on: `RunError`, `CompletionError`, `StoreError`, `ParseError`, `ToolError`, `DialectError`, `ToolIdError`, `ToolRegistryError`. The small leaf construction errors `SecretError`, `ToolSchemaError`, `ModelIdError`, `ModelCatalogError`, `TemperatureError` are already `#[non_exhaustive]` enums/structs whose variants are matched directly; a redundant classifier is not required and not added.

*2026-08-09 19:20 - Opus 4.8 High*
*Reconciliation appended 2026-08-10 - Opus 4.8*
