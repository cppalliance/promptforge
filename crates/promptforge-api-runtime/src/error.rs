//! The crate's internal error substrate.
//!
//! [`Error`] is a `pub(crate)` substrate: it is never part of the public API.
//! Every public boundary returns its own typed error ([`crate::RunError`],
//! [`crate::ParseError`], [`crate::CompletionError`],
//! [`promptforge_api_types::tools::ToolError`], [`promptforge_store::StoreError`]); those wrappers
//! classify this substrate and preserve its source. See the module wrappers for
//! the `From` bridges that let internal `?` keep flowing through the substrate.

use std::borrow::Cow;

use promptforge_api_types::ids::TaskId;
use promptforge_lua::Error as LuaError;
use promptforge_model_client::Error as GatewayClientError;
use promptforge_parser::Error as ParserError;

/// A type-erased owned error cause used by the internal substrate.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// Renders task ids as a comma-separated list: the [`Error::TasksLive`]
/// message and its `tasks` field.
fn join_task_ids(tasks: &[TaskId]) -> String {
    tasks
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The crate's internal error substrate, spanning parsing, HTTP, and execution
/// failures.
///
/// This type is `pub(crate)` and never appears in the public API; the public
/// boundary errors wrap and classify it. Marked `#[non_exhaustive]` so future
/// variants are not a breaking change. The transport variant hides its concrete
/// source type so no dependency's error leaks through the wrappers' `source()`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum Error {
    /// The prompt frontmatter was not valid YAML, preserving the parser cause.
    ///
    /// This retains the originating YAML decode failure (a
    /// `serde_yaml_ng::Error`) as the `#[source]` cause (F3) so
    /// [`crate::ParseError`] can expose the frontmatter syntax location through
    /// [`std::error::Error::source`] instead of flattening it into the message.
    #[error("invalid frontmatter: {message}")]
    #[non_exhaustive]
    ParseFrontmatter {
        /// The human-readable diagnostic (no raw source dump).
        message: String,
        /// The originating YAML parse failure, kept as the cause.
        #[source]
        source: BoxedSource,
        /// The 1-based file line of the YAML failure, surfaced from the
        /// retained cause's location when it carries one.
        line: Option<u32>,
        /// The 1-based file column of the YAML failure, when known.
        column: Option<u32>,
    },

    /// A structurally-classified parse failure carrying a stable kind and an
    /// optional source byte span, so [`crate::ParseError`] can expose the
    /// classification and location from stored fields instead of inferring them
    /// from message text.
    #[error("{message}")]
    #[non_exhaustive]
    ParseStructured {
        /// The stable classification of this parse failure.
        kind: crate::parser::ParseErrorKind,
        /// The byte span of the offending region within the source, when known.
        span: Option<(usize, usize)>,
        /// The human-readable diagnostic.
        message: String,
        /// The prompt's frontmatter name, when the failure postdates the
        /// frontmatter (a frontmatter failure predates the name).
        name: Option<String>,
        /// The 1-based file line of the span's start, when a span is known.
        line: Option<u32>,
        /// The 1-based byte column of the span's start, when a span is known.
        column: Option<u32>,
    },

    /// A required environment variable was missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

    /// An environment variable was set but its value was not valid Unicode.
    #[error("environment variable is set but not valid Unicode: {0}")]
    InvalidEnv(String),

    /// A client or endpoint configuration value failed semantic validation.
    #[error("{0}")]
    InvalidConfig(String),

    /// A client or endpoint configuration input was invalid, retaining the
    /// concrete cause (a secret or URL validation failure) as a private
    /// `#[source]` (client F13 / AUDIT-DISCARDED-SOURCE) instead of flattening
    /// it into the message.
    #[error("{message}")]
    #[non_exhaustive]
    Config {
        /// The human-readable configuration diagnostic (no raw source dump).
        message: String,
        /// The originating validation failure (secret or URL parse), kept as
        /// the cause.
        #[source]
        source: BoxedSource,
    },

    /// Gateway access was explicitly disabled by the host.
    #[error("gateway access is disabled")]
    GatewayDisabled,

    /// The HTTP request to the model backend failed at the transport layer.
    #[error("http transport failure")]
    Http(#[source] BoxedSource),

    /// The backend returned a non-success status.
    ///
    /// The `Display` is deliberately body-free (F5): the bounded, control-escaped
    /// body rides only in the private `body` field, reachable through the
    /// explicit [`crate::CompletionError::backend_body`] opt-in, so a raw or
    /// hostile payload cannot forge log lines or leak into an error message.
    #[error("non-success backend status {status}")]
    Backend {
        /// The HTTP status code returned by the backend.
        status: u16,
        /// The bounded, control-escaped response body, for opt-in diagnostics.
        body: String,
    },

    /// The backend response could not be understood (missing choices, etc.).
    #[error("malformed response: {0}")]
    MalformedResponse(String),

    /// The backend response could not be decoded, preserving the decoder cause.
    ///
    /// Like [`Error::MalformedResponse`] but retains the underlying decode
    /// failure (for example a [`serde_json::Error`]) as the `#[source]` cause
    /// rather than flattening it into the message (MODEL-009 / client F11), so
    /// the error chain survives through the public wrappers' `source()`.
    #[error("malformed response: {message}")]
    #[non_exhaustive]
    MalformedResponseSource {
        /// The human-readable diagnostic (no raw body).
        message: String,
        /// The originating decode failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// Reading a non-success backend response body failed at the transport
    /// layer.
    ///
    /// Retains the `reqwest::Error` as the `#[source]` cause (MODEL-010)
    /// rather than flattening the read failure into display text, so the error
    /// chain (timeout, connection reset) survives. The status the backend had
    /// already returned is preserved for classification.
    #[error("unreadable backend error body (status {status})")]
    #[non_exhaustive]
    BackendBodyRead {
        /// The non-success HTTP status whose body could not be read.
        status: u16,
        /// The originating transport read failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// The model returned neither non-empty tool calls nor non-empty text.
    ///
    /// Reasoning side-channel text, when present, is never promoted into the
    /// answer; `detail` may note that it was ignored, without pasting it. The
    /// choice's `finish_reason` rides along so the tool loop can classify the
    /// empty turn (a `"stop"` exit differs from a truncation or a missing
    /// reason).
    #[error("{detail}")]
    #[non_exhaustive]
    EmptyModelReply {
        /// The phrase naming the empty product (and ignored reasoning): the
        /// model client's fixed text when the client classified the turn,
        /// or the message a Lua-side `empty_model_reply` raise carried, so
        /// the error re-renders with the text the author saw.
        detail: Cow<'static, str>,
        /// The choice's `finish_reason`, when the backend supplied one.
        finish_reason: Option<String>,
    },

    /// The host cancelled the run (for example Ctrl-C during fanout).
    #[error("interrupted by Ctrl-C")]
    Interrupted,

    /// A section's Lua phase failed a host contract or hit a poisoned lock: a
    /// runtime-internal condition with no originating `mlua` error to preserve
    /// (for example "host values have not been injected" or a poisoned mutex).
    ///
    /// Failures that *do* carry an `mlua` cause use [`Error::LuaRuntime`], which
    /// retains that cause as a private source (F4). The message is the specific
    /// failure as a noun phrase; the public wrapper classifies this as a Lua
    /// failure, so no redundant `lua error:` type label is prepended (F8).
    #[error("{0}")]
    Lua(String),

    /// A section's Lua phase failed at runtime or while bridging host values,
    /// retaining the originating `mlua` error as the private `#[source]` cause
    /// (F4) alongside the mapped prompt-location message.
    ///
    /// This is the source-bearing counterpart to [`Error::Lua`]: it is built
    /// from a concrete `mlua::Error` (see [`Error::lua`] and
    /// [`crate::lua::LuaProgram::map_runtime_error`]), so the failure chain
    /// survives through the public wrappers' `source()` instead of being
    /// flattened to a string.
    #[error("{message}")]
    #[non_exhaustive]
    LuaRuntime {
        /// The mapped, location-tagged diagnostic (no redundant type label).
        message: String,
        /// The originating Lua error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// Lua source was not syntactically valid at its prompt location.
    ///
    /// Retains the originating `mlua` compile error as the private `#[source]`
    /// cause (F4) alongside the location metadata, so the compiler diagnostic
    /// chain survives through the public wrappers' `source()` instead of being
    /// flattened into `message` alone.
    #[error("lua compilation error at {location} (line {source_line}): {message}")]
    #[non_exhaustive]
    LuaCompile {
        /// The prompt region supplied by the parser, such as a section prologue.
        location: String,
        /// 1-based line number in the prompt source where this Lua region starts.
        source_line: u32,
        /// The retained source that failed to compile.
        lua_source: String,
        /// The Lua 5.5 compiler diagnostic.
        message: String,
        /// The originating `mlua` compile error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// Building a model-facing tool schema for a bound alias failed, retaining
    /// the schema validation error as the private `#[source]` cause (F5) rather
    /// than flattening it into `detail`.
    ///
    /// Constructed only by the tool-scope preparation, which is test-only
    /// until the `models.loop` step rewires it.
    #[error("model-facing schema build failure for tool alias {alias:?}")]
    #[non_exhaustive]
    BindSchema {
        /// The prompt-local alias whose schema could not be built.
        alias: String,
        /// The originating schema validation failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// A `{{ }}` prose substitution failed (unknown/missing path, unclosed).
    ///
    /// Carries a typed [`crate::subst::SubstitutionError`] with a stable kind,
    /// the byte offset of the offending placeholder, a bounded preview, and any
    /// preserved serialization source, rather than a flattened string. The
    /// substitution error is the whole message and its cause chain, so the
    /// variant is transparent over it.
    #[error(transparent)]
    Substitution(Box<crate::subst::SubstitutionError>),

    /// The tool-call loop ran its iteration cap without a final text reply.
    #[error("tool-call loop did not converge")]
    ToolLoopExhausted,

    /// A chain ended while author-origin tasks it owned were still live.
    ///
    /// A spawned task ends with its owner, so a task the author neither
    /// waited on nor cancelled is the author's bug: the chain's outcome
    /// becomes this error (the run's for the root walk, the call's answer
    /// for a `call` chain) and the leaked tasks are abandoned. The message
    /// names the ids in spawn order; the Lua table carries them as `tasks`.
    #[error(
        "chain ended with author tasks still live: {}; wait on or cancel every task a chain spawns before it ends",
        join_task_ids(.tasks)
    )]
    TasksLive {
        /// The live author tasks, in spawn order.
        tasks: Vec<TaskId>,
    },

    /// A task operation named a task the caller does not own.
    ///
    /// Only the spawning chain may wait on, inspect, or cancel a task; a
    /// chain may additionally read the status of, and annotate, the task
    /// it runs inside. An id that names no task at all is refused the same
    /// way, so a caller learns nothing about tasks it never started.
    #[error("task `{task}` is not a task this chain owns")]
    TaskNotOwned {
        /// The task the caller reached for.
        task: TaskId,
    },

    /// A wait named a task whose result was already delivered once.
    #[error("task `{task}` was already delivered: a task's result is taken by one wait")]
    TaskConsumed {
        /// The task whose result was taken.
        task: TaskId,
    },

    /// The failure a wait delivers for a task its owner cancelled instead
    /// of letting it end on its own: the member's `ok = false` error value,
    /// kind `cancelled`, with a `task` field. An abandoned task is never
    /// delivered - it lost its owner, and only the owner may wait - so
    /// this is the one non-`Done` delivery.
    #[error("task `{task}` was cancelled")]
    TaskCancelled {
        /// The task that was cancelled.
        task: TaskId,
    },

    /// The model referenced a tool outside the section's advertised scope.
    ///
    /// This is the model tool loop's error alone: a script `tools.call`
    /// resolves against the run's full bound catalog and fails with
    /// [`Error::UnboundToolCall`] instead.
    #[error("tool {name:?} is not in this section's scope; in-scope aliases: {in_scope:?}{}", if *.global_exists { " (alias is a bound tool slot but was not added to this section's scope)" } else { "" })]
    #[non_exhaustive]
    OutOfScopeToolCall {
        /// The alias or identifier the model tried to use.
        name: String,
        /// Whether the name exists in the prompt-wide `tools.bind` map.
        global_exists: bool,
        /// The aliases that are in scope for this VM.
        in_scope: Vec<String>,
    },

    /// A script `tools.call` referenced an alias with no binding in the run's
    /// tool catalog.
    ///
    /// Script-initiated dispatch resolves against the run's full bound set,
    /// not the section's advertised scope - the scope shapes what the model
    /// is offered, and the author's own code is not the model - so this
    /// error means the alias was never bound at all.
    #[error("tool {name:?} is not bound in this run; bound aliases: {bound:?}")]
    #[non_exhaustive]
    UnboundToolCall {
        /// The alias the script tried to dispatch.
        name: String,
        /// Every bound alias in the run's tool catalog.
        bound: Vec<String>,
    },

    /// A model-facing section has non-empty prose but no `models.use` or
    /// prompt-wide `models.default` binding.
    #[error("model binding required for section {section}")]
    #[non_exhaustive]
    ModelRequired {
        /// The H2 section heading that reached a model turn without a binding.
        section: String,
    },

    /// The prompt declares a `promptforge:` major this build does not support,
    /// so it is refused rather than run under mismatched rules.
    #[error("unsupported promptforge version: {0} (this build supports major 0)")]
    UnsupportedVersion(u32),

    /// The environment cannot satisfy the prompt's declared requirements:
    /// required capabilities are missing, or the filled model fails a
    /// declared hard requirement (a context minimum or hard keyword).
    ///
    /// The notice is the whole message, written to be read by a model: it
    /// may arrive as tool output when the prompt runs as a sub-run tool.
    #[error("{notice}")]
    #[non_exhaustive]
    RequirementsUnmet {
        /// The model-readable refusal notice, one line per gap.
        notice: String,
    },

    /// A dispatched tool returned a model-safe failure.
    ///
    /// The tool's own [`promptforge_api_types::tools::ToolError`] is preserved as the
    /// `#[source]` cause, so the failure chain (and any transport/parse error the
    /// tool wrapped) survives instead of being flattened to a string.
    #[error("tool call failure: {message}")]
    Tool {
        /// The tool's model-safe failure message.
        message: String,
        /// The originating tool error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// An internal runtime invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur). Surfaced as a concrete error rather
    /// than silently skipping work, so an impossible state cannot masquerade as a
    /// successful fall-through.
    ///
    /// The Rust source position of the construction site is captured (via
    /// [`Error::internal`], which is `#[track_caller]`) so
    /// [`crate::RunError::location`] can point at the broken invariant.
    #[error("internal invariant violated: {message}")]
    #[non_exhaustive]
    Internal {
        /// The violated invariant, as a noun phrase.
        message: &'static str,
        /// The Rust source file of the construction site (from `file!()`).
        file: &'static str,
        /// The 1-based line of the construction site (from `line!()`).
        line: u32,
    },

    /// A Lua host resource quota (log events, log bytes, or instructions) was
    /// exhausted. A stable typed error rather than a bare `Lua(String)` so hosts
    /// can distinguish quota exhaustion from an authoring error.
    #[error("lua {resource} quota exceeded")]
    #[non_exhaustive]
    LuaQuota {
        /// The exhausted resource: `"log event"`, `"log byte"`, or `"instruction"`.
        resource: &'static str,
    },

    /// The selected compactor exhausted the model's context window: the
    /// request overflowed on the pre-dispatch precheck or at the provider,
    /// and the policy (`compactors.fail`, the only shipped one) does not
    /// compact.
    #[error("context exhausted: {reason}")]
    #[non_exhaustive]
    ContextExhausted {
        /// Which overflow check fired.
        reason: crate::lua::OverflowReason,
    },

    /// The host's input broker failed a `user_input` request: the wait
    /// ended in failure rather than an answer or the unavailable fallback,
    /// so the call raises this typed error at its Lua call site.
    #[error("user input request was not answered: {message}")]
    Input {
        /// The broker's host-authored, model-safe failure message.
        message: String,
        /// The broker's own cause, retained when it supplied one.
        #[source]
        source: Option<BoxedSource>,
    },

    /// A run-scoped store operation failed at the virtual filesystem layer,
    /// retaining the concrete [`shared_vfs::VfsError`] as the `#[source]`
    /// cause so a backend failure survives the public wrappers instead of
    /// being flattened to a string. The message names only the operation;
    /// a renderer that wants the backend's diagnosis walks `source()`.
    #[error("store operation failed")]
    Store(#[source] shared_vfs::VfsError),

    /// Two live execution identities claimed one store path: the claims
    /// model's conflict, mapped from the store's write-race vocabulary at
    /// the yield-answer boundary. Fatal to the run on the spot and never
    /// resumed into Lua, so no author `pcall` can catch it; the message is
    /// the claims model's whole diagnosis, naming the canonical path, both
    /// identities, and both claim kinds.
    #[error("store determinism violation: {0}")]
    Determinism(String),
}

impl Error {
    /// Builds a parse failure with a stable classification and no source span.
    pub(crate) fn parse(kind: crate::parser::ParseErrorKind, message: impl Into<String>) -> Error {
        Error::ParseStructured {
            kind,
            span: None,
            message: message.into(),
            name: None,
            line: None,
            column: None,
        }
    }

    /// Stamps the prompt's frontmatter name onto a parse failure raised by
    /// `run` itself: the prompt is already parsed at that point, so the
    /// name is known and the location can name it. Any other variant passes
    /// through unchanged.
    pub(crate) fn with_prompt_name(mut self, name: &str) -> Error {
        if let Error::ParseStructured { name: slot, .. } = &mut self {
            *slot = Some(name.to_owned());
        }
        self
    }

    /// Builds an internal-invariant failure, capturing the Rust source
    /// position of the call site so [`crate::RunError::location`] can point
    /// at the broken invariant.
    #[track_caller]
    pub(crate) fn internal(message: &'static str) -> Error {
        let location = std::panic::Location::caller();
        Error::Internal {
            message,
            file: location.file(),
            line: location.line(),
        }
    }

    /// Wrap an `mlua` failure as [`Error::LuaRuntime`], preserving it as the
    /// `#[source]` cause (F4) rather than flattening it to a string.
    #[cfg(test)]
    pub(crate) fn lua(source: mlua::Error) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source),
        }
    }
}

impl From<crate::subst::SubstitutionError> for Error {
    fn from(error: crate::subst::SubstitutionError) -> Error {
        Error::Substitution(Box::new(error))
    }
}

impl From<crate::input::InputError> for Error {
    fn from(error: crate::input::InputError) -> Error {
        let (message, source) = error.into_parts();
        Error::Input { message, source }
    }
}

/// Maps the gateway-client substrate back onto this substrate variant for
/// variant, so `Display`, `source()` chains, and `RunError`/`CompletionError`
/// classification are unchanged by the extraction. The client crate's
/// substrate is not `#[non_exhaustive]` (the two crates version together), so
/// this match is total.
impl From<GatewayClientError> for Error {
    fn from(error: GatewayClientError) -> Error {
        match error {
            GatewayClientError::MissingEnv(name) => Error::MissingEnv(name),
            GatewayClientError::InvalidEnv(name) => Error::InvalidEnv(name),
            GatewayClientError::InvalidConfig(detail) => Error::InvalidConfig(detail),
            GatewayClientError::Config { message, source } => Error::Config { message, source },
            GatewayClientError::GatewayDisabled => Error::GatewayDisabled,
            GatewayClientError::Http(source) => Error::Http(source),
            GatewayClientError::Backend { status, body } => Error::Backend { status, body },
            GatewayClientError::MalformedResponse(message) => Error::MalformedResponse(message),
            GatewayClientError::MalformedResponseSource { message, source } => {
                Error::MalformedResponseSource { message, source }
            }
            GatewayClientError::BackendBodyRead { status, source } => {
                Error::BackendBodyRead { status, source }
            }
            GatewayClientError::EmptyModelReply {
                detail,
                finish_reason,
            } => Error::EmptyModelReply {
                detail: Cow::Borrowed(detail),
                finish_reason,
            },
            GatewayClientError::ModelSetLock(message) => Error::Lua(message),
        }
    }
}

impl From<crate::model::CompletionError> for Error {
    fn from(error: crate::model::CompletionError) -> Error {
        Error::from(GatewayClientError::from(error))
    }
}

/// Maps a parse failure back onto this substrate variant for variant, so
/// `Display`, `source()` chains, and `RunError` classification are unchanged
/// by the parser extraction. The parser crate's substrate is not
/// `#[non_exhaustive]` (the two crates version together), so this match is
/// total.
impl From<crate::parser::ParseError> for Error {
    fn from(error: crate::parser::ParseError) -> Self {
        match error.into_inner() {
            ParserError::ParseFrontmatter {
                message,
                source,
                line,
                column,
            } => Error::ParseFrontmatter {
                message,
                source,
                line,
                column,
            },
            ParserError::ParseStructured {
                kind,
                span,
                message,
                name,
                line,
                column,
            } => Error::ParseStructured {
                kind,
                span,
                message,
                name,
                line,
                column,
            },
            ParserError::Lua(lua) => Error::from(lua),
            ParserError::Internal(message) => Error::internal(message),
        }
    }
}

/// Maps the Lua crate's substrate back onto this substrate variant for
/// variant, so `Display`, `source()` chains, and `RunError`/`CompletionError`
/// classification are unchanged by the extraction. The Lua crate's substrate
/// is not `#[non_exhaustive]` (the two crates version together), so this match
/// is total.
impl From<LuaError> for Error {
    fn from(error: LuaError) -> Error {
        match error {
            LuaError::Lua(message) => Error::Lua(message),
            LuaError::LuaRuntime { message, source } => Error::LuaRuntime { message, source },
            LuaError::LuaCompile {
                location,
                source_line,
                lua_source,
                message,
                source,
            } => Error::LuaCompile {
                location,
                source_line,
                lua_source,
                message,
                source,
            },
            LuaError::LuaQuota { resource } => Error::LuaQuota { resource },
            LuaError::ContextExhausted { reason } => Error::ContextExhausted { reason },
            LuaError::Interrupted => Error::Interrupted,
            LuaError::Tool { message, source } => Error::Tool { message, source },
            LuaError::Internal(message) => Error::internal(message),
            LuaError::Raised(raised) => Error::from_raised(raised),
        }
    }
}

/// The `task` field of a raised task-error table, when it parses.
fn raised_task(raised: &promptforge_lua::Raised) -> Option<TaskId> {
    raised.fields.get("task").and_then(|task| task.parse().ok())
}

impl Error {
    /// Maps a structured error table that surfaced as a block's failure
    /// onto the variant its kind names, so a Lua-side raise classifies as
    /// the Rust-raised error it stands in for. A kind whose variant needs
    /// structure the table does not carry (the tool-scope errors, the task
    /// errors, `internal`) keeps its message as a Lua failure; those
    /// classifications arrive with the shims that raise them.
    fn from_raised(raised: promptforge_lua::Raised) -> Error {
        match raised.kind {
            promptforge_lua::ErrorKind::ToolLoopExhausted => Error::ToolLoopExhausted,
            promptforge_lua::ErrorKind::ContextExhausted => match raised.overflow_reason() {
                Some(reason) => Error::ContextExhausted { reason },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::EmptyModelReply => Error::EmptyModelReply {
                finish_reason: raised.fields.get("finish_reason").cloned(),
                detail: Cow::Owned(raised.message),
            },
            promptforge_lua::ErrorKind::Cancelled => Error::Interrupted,
            promptforge_lua::ErrorKind::Tool => Error::Tool {
                message: raised.message.clone(),
                source: Box::new(raised),
            },
            promptforge_lua::ErrorKind::TaskNotOwned => match raised_task(&raised) {
                Some(task) => Error::TaskNotOwned { task },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::TaskConsumed => match raised_task(&raised) {
                Some(task) => Error::TaskConsumed { task },
                None => Error::Lua(raised.message),
            },
            promptforge_lua::ErrorKind::OutOfScopeTool
            | promptforge_lua::ErrorKind::UnboundTool
            | promptforge_lua::ErrorKind::TasksLive
            | promptforge_lua::ErrorKind::Lua
            | promptforge_lua::ErrorKind::Internal => Error::Lua(raised.message),
        }
    }
}

/// The substrate's rendering into the Lua error table: the kind an author
/// branches on and the kind's fields. Host-side failures the author cannot
/// act on (transport, backend, configuration, store, input) render as
/// `internal`; every Lua-phase failure renders as `lua`.
impl promptforge_lua::ErrorValue for Error {
    fn kind(&self) -> promptforge_lua::ErrorKind {
        use promptforge_lua::ErrorKind;
        match self {
            Error::Lua(_)
            | Error::LuaRuntime { .. }
            | Error::LuaCompile { .. }
            | Error::LuaQuota { .. }
            | Error::Substitution(_) => ErrorKind::Lua,
            Error::ContextExhausted { .. } => ErrorKind::ContextExhausted,
            Error::EmptyModelReply { .. } => ErrorKind::EmptyModelReply,
            Error::Interrupted | Error::TaskCancelled { .. } => ErrorKind::Cancelled,
            Error::ToolLoopExhausted => ErrorKind::ToolLoopExhausted,
            Error::TasksLive { .. } => ErrorKind::TasksLive,
            Error::TaskNotOwned { .. } => ErrorKind::TaskNotOwned,
            Error::TaskConsumed { .. } => ErrorKind::TaskConsumed,
            Error::OutOfScopeToolCall { .. } => ErrorKind::OutOfScopeTool,
            Error::UnboundToolCall { .. } => ErrorKind::UnboundTool,
            Error::Tool { .. } => ErrorKind::Tool,
            Error::ParseFrontmatter { .. }
            | Error::ParseStructured { .. }
            | Error::MissingEnv(_)
            | Error::InvalidEnv(_)
            | Error::InvalidConfig(_)
            | Error::Config { .. }
            | Error::GatewayDisabled
            | Error::Http(_)
            | Error::Backend { .. }
            | Error::MalformedResponse(_)
            | Error::MalformedResponseSource { .. }
            | Error::BackendBodyRead { .. }
            | Error::BindSchema { .. }
            | Error::ModelRequired { .. }
            | Error::UnsupportedVersion(_)
            | Error::RequirementsUnmet { .. }
            | Error::Internal { .. }
            | Error::Input { .. }
            | Error::Store(_)
            | Error::Determinism(_) => ErrorKind::Internal,
        }
    }

    fn fields(&self) -> Vec<(String, String)> {
        match self {
            Error::ContextExhausted { reason } => {
                vec![("reason".to_owned(), reason.tag().to_owned())]
            }
            Error::EmptyModelReply {
                finish_reason: Some(finish_reason),
                ..
            } => vec![("finish_reason".to_owned(), finish_reason.clone())],
            Error::OutOfScopeToolCall { name, .. } | Error::UnboundToolCall { name, .. } => {
                vec![("name".to_owned(), name.clone())]
            }
            Error::TasksLive { tasks } => vec![("tasks".to_owned(), join_task_ids(tasks))],
            Error::TaskNotOwned { task }
            | Error::TaskConsumed { task }
            | Error::TaskCancelled { task } => {
                vec![("task".to_owned(), task.to_string())]
            }
            _ => Vec::new(),
        }
    }
}

/// Crate-internal result alias over the [`Error`] substrate.
pub(crate) type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {

    use super::*;
    use crate::parser::Prompt;

    fn assert_source_survives_run_error(error: Error) {
        assert!(
            std::error::Error::source(&error).is_some(),
            "the internal error must preserve its source"
        );
        assert!(
            std::error::Error::source(&crate::RunError::from(error)).is_some(),
            "the public RunError wrapper must keep the source reachable"
        );
    }

    #[test]
    fn context_exhaustion_maps_from_lua_and_classifies() {
        // The compactor's typed exhaustion crosses the crate seam
        // variant-for-variant and classifies as its own run-error kind, so a
        // host can distinguish context exhaustion from a transport failure.
        let lua_error = LuaError::ContextExhausted {
            reason: promptforge_lua::OverflowReason::Provider,
        };
        let error = Error::from(lua_error);
        assert!(
            matches!(
                error,
                Error::ContextExhausted {
                    reason: crate::lua::OverflowReason::Provider
                }
            ),
            "the mapping preserves the reason, got {error:?}"
        );
        assert!(
            error.to_string().starts_with("context exhausted: "),
            "the diagnostic names the exhaustion: {error}"
        );
        let run_error = crate::RunError::from(error);
        assert_eq!(run_error.kind(), crate::RunErrorKind::ContextExhausted);
        assert!(
            !run_error.is_retryable(),
            "retrying an over-window request cannot succeed"
        );
    }

    #[test]
    fn source_bearing_binding_errors_preserve_their_cause() {
        // F5: the binding and tool-scope failures keep the originating typed
        // error as a private `source()` instead of flattening it to a string,
        // and the chain survives through the public `RunError` wrapper.
        use promptforge_model_client::client::ToolSchemaError;

        let schema_error = ToolSchemaError::NonObjectSchema {
            name: "echo".to_owned(),
        };
        let bind = Error::BindSchema {
            alias: "echo".to_owned(),
            source: Box::new(schema_error),
        };
        assert_eq!(
            bind.to_string(),
            "model-facing schema build failure for tool alias \"echo\""
        );
        assert_source_survives_run_error(bind);
    }

    #[test]
    fn lua_compile_preserves_the_originating_compiler_error() {
        // F4: a compile failure keeps the concrete `mlua` error as a private
        // `source()` instead of flattening it into `message` alone, and the
        // chain survives through the public `RunError` wrapper.
        let compile = Error::LuaCompile {
            location: "section `S` prologue".to_owned(),
            source_line: 7,
            lua_source: "x =".to_owned(),
            message: "syntax error near '='".to_owned(),
            source: Box::new(mlua::Error::SyntaxError {
                message: "syntax error near '='".to_owned(),
                incomplete_input: false,
            }),
        };
        assert_source_survives_run_error(compile);
    }

    #[test]
    fn typed_error_survives_the_lua_external_boundary() {
        // LUA-012: passing the typed error (not its `to_string()`) to
        // `mlua::Error::external` keeps the original error as a downcastable
        // source across the Lua boundary, rather than flattening it to text.
        let original = Error::OutOfScopeToolCall {
            name: "echo".to_owned(),
            global_exists: false,
            in_scope: vec!["other".to_owned()],
        };
        let display = original.to_string();
        let external = mlua::Error::external(original);
        match &external {
            mlua::Error::ExternalError(cause) => {
                let recovered = cause
                    .downcast_ref::<Error>()
                    .expect("the original typed Error is preserved, not stringified");
                assert_eq!(recovered.to_string(), display);
            }
            other => panic!("expected an ExternalError carrying the typed error, got {other:?}"),
        }
        // Re-wrapping through the crate's Lua boundary keeps the chain reachable.
        let wrapped = Error::lua(external);
        assert!(std::error::Error::source(&wrapped).is_some());
    }

    #[test]
    fn config_errors_preserve_their_causes_across_the_substrate_bridge() {
        // AUDIT-DISCARDED-SOURCE: a transport's configuration failure (an
        // unusable credential, a bad endpoint URL) arrives as the client
        // substrate's `Config` variant with its concrete cause attached;
        // the cause survives both the public CompletionError::source and
        // the mapping onto this crate's substrate, classified as Config.
        use crate::model::{ClientError, CompletionError, CompletionErrorKind};

        let cause = std::io::Error::other("gateway URL is not a valid URL");
        let completion = CompletionError::from(ClientError::Config {
            message: "gateway endpoint is unusable".to_owned(),
            source: Box::new(cause),
        });
        assert_eq!(completion.kind(), CompletionErrorKind::Config);
        assert!(
            std::error::Error::source(&completion).is_some(),
            "the configuration cause must survive the public wrapper"
        );
        let bridged = Error::from(completion);
        assert!(
            matches!(bridged, Error::Config { .. }),
            "the substrate maps Config onto Config, got {bridged:?}"
        );
        assert!(
            std::error::Error::source(&bridged).is_some(),
            "the cause must survive the bridge"
        );
    }

    #[test]
    fn frontmatter_locations_surface_through_the_run_error() {
        // Step 6: the parser's surfaced YAML position crosses the substrate
        // bridge and lands on `RunError::location` for navigation. A
        // frontmatter failure predates the prompt's name, so the path is
        // the placeholder a host replaces with its own label for the source.
        let source = concat!(
            "---\n",
            "name: x\n",
            "description: d\n",
            "capabilities:\n",
            "  - not a capability id\n",
            "---\n",
            "\n# T\n\n## S\n\np\n",
        );
        let parse = Prompt::parse(source, "test")
            .0
            .expect_err("a capability id with spaces must be rejected");
        let run_error = crate::RunError::from(Error::from(parse));
        assert_eq!(run_error.kind(), crate::RunErrorKind::Parse);
        let location = run_error
            .location()
            .expect("a parse failure carries a location");
        assert_eq!(location.line, Some(5));
        assert_eq!(location.column, Some(5));
        assert_eq!(location.span, None);
    }

    #[test]
    fn structured_locations_carry_the_prompt_name_through_the_run_error() {
        // Step 6: a post-frontmatter parse failure carries the prompt's
        // frontmatter name as the location's path, plus the offending
        // span's line and column.
        let source = "---\nname: dup\ndescription: d\n---\n\n# T\n\n## S\n\np\n\n## S\n\nq\n";
        let parse = Prompt::parse(source, "test")
            .0
            .expect_err("duplicate sibling sections must be rejected");
        let run_error = crate::RunError::from(Error::from(parse));
        let location = run_error
            .location()
            .expect("a structured parse failure carries a location");
        assert_eq!(location.path, "dup");
        assert_eq!(location.line, Some(12));
        assert_eq!(location.column, Some(1));
        assert!(location.span.is_some());
    }

    #[test]
    fn internal_faults_carry_the_rust_file_and_line() {
        // Step 6: an internal invariant failure locates itself in the Rust
        // source, captured at the construction site.
        let expected_line = line!() + 1;
        let run_error = crate::RunError::from(Error::internal("a test invariant"));
        let location = run_error
            .location()
            .expect("an internal fault carries a location");
        assert!(
            location.path.ends_with("error.rs"),
            "the path is the Rust source file: {}",
            location.path
        );
        assert_eq!(location.line, Some(expected_line));
        assert_eq!(location.column, None);
    }

    #[test]
    fn errors_without_a_location_return_none() {
        // Cancellation and the other non-positional kinds have no source
        // position to navigate to.
        let run_error = crate::RunError::from(Error::Interrupted);
        assert!(run_error.location().is_none());
    }

    #[test]
    fn requirements_unmet_classifies_and_carries_the_notice_as_its_message() {
        // Step 10: the refusal notice is the whole Display - it may arrive
        // as tool output when the prompt runs as a sub-run tool - and the
        // kind classifies it for code. Retrying cannot help: the
        // environment, not the transport, is what falls short.
        let error = Error::RequirementsUnmet {
            notice: "the environment cannot satisfy this prompt:\n- role 'analyst': requires a context of at least 200000 tokens; the current model provides 32000".to_owned(),
        };
        let run_error = crate::RunError::from(error);
        assert_eq!(run_error.kind(), crate::RunErrorKind::RequirementsUnmet);
        assert!(!run_error.is_cancelled());
        assert!(!run_error.is_retryable());
        assert!(run_error.location().is_none());
        assert!(run_error.to_string().contains("analyst"));
    }
}
