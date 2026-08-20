//! The crate's internal error substrate.
//!
//! [`Error`] is a `pub(crate)` substrate: it is never part of the public API.
//! Every public boundary returns its own typed error ([`crate::RunError`],
//! [`crate::ParseError`], [`crate::CompletionError`], [`crate::DialectError`],
//! [`crate::tools::ToolError`], [`crate::store::StoreError`]); those wrappers
//! classify this substrate and preserve its source. See the module wrappers for
//! the `From` bridges that let internal `?` keep flowing through the substrate.

/// A type-erased owned error cause used by the internal substrate.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// A cloneable, shareable error cause.
///
/// Some caches re-produce a typed [`Error`] on every lookup (for example the
/// resolver decision cache), so a non-`Clone` dependency error cannot be moved
/// into a fresh [`Error`] each time. Wrapping it in a reference-counted
/// [`SharedSource`] lets the typed cause be retained as a `#[source]` and cloned
/// cheaply per lookup instead of being flattened to a string (resolve F4).
#[derive(Debug, Clone)]
pub(crate) struct SharedSource(std::sync::Arc<dyn std::error::Error + Send + Sync>);

impl SharedSource {
    /// Wraps a concrete error as a shareable cause.
    pub(crate) fn new(source: impl std::error::Error + Send + Sync + 'static) -> SharedSource {
        SharedSource(std::sync::Arc::new(source))
    }
}

impl std::fmt::Display for SharedSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for SharedSource {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
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
    /// [`serde_yaml_ng::Error`]) as the `#[source]` cause (F3) so
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
    /// Retains the [`reqwest::Error`] as the `#[source]` cause (MODEL-010)
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
        /// Fixed phrase naming the empty product (and ignored reasoning).
        detail: &'static str,
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
        /// The Lua 5.4 compiler diagnostic.
        message: String,
        /// The originating `mlua` compile error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// The concrete picker failed while resolving a capability declaration.
    #[error("tool capability binding failure for {capability:?}: {detail}")]
    #[non_exhaustive]
    Bind {
        /// The exact capability description passed to `tools.need`.
        capability: String,
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// Building a model-facing tool schema for a bound alias failed, retaining
    /// the schema validation error as the private `#[source]` cause (F5) rather
    /// than flattening it into `detail`.
    #[error("model-facing schema build failure for tool alias {alias:?}")]
    #[non_exhaustive]
    BindSchema {
        /// The prompt-local alias whose schema could not be built.
        alias: String,
        /// The originating schema validation failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// The picker's query failed while resolving a capability, retaining the
    /// picker's own typed error as the private `#[source]` cause (resolve F4)
    /// so the failure chain survives the resolution cache instead of being
    /// flattened to a string.
    #[error("tool capability binding failure for {capability:?}: {source}")]
    #[non_exhaustive]
    BindQuery {
        /// The exact capability description passed to `tools.need`.
        capability: String,
        /// The picker's typed query failure, kept as a shareable cause.
        #[source]
        source: SharedSource,
    },

    /// No picker catalog entry matched a declared capability.
    #[error("no tool matches capability {capability:?}")]
    #[non_exhaustive]
    Absent {
        /// The exact capability description passed to `tools.need`.
        capability: String,
    },

    /// One server published duplicate matches for a declared capability.
    #[error("duplicate tools match capability {capability:?}: {candidates:?}")]
    #[non_exhaustive]
    Duplicate {
        /// The exact capability description passed to `tools.need`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<crate::tools::ToolId>,
    },

    /// The picker could not choose uniquely among capability matches.
    #[error("ambiguous tools match capability {capability:?}: {candidates:?}")]
    #[non_exhaustive]
    Ambiguous {
        /// The exact capability description passed to `tools.need`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<crate::tools::ToolId>,
    },

    /// One prompt-local alias was declared more than once.
    #[error("tool alias {alias:?} was declared more than once")]
    #[non_exhaustive]
    DuplicateAlias {
        /// The exact case-sensitive alias declared by the prompt.
        alias: String,
    },

    /// The live registry contains more than one entry with one stable identity.
    #[error("live tool registry contains duplicate identity {id:?}")]
    #[non_exhaustive]
    DuplicateLiveToolId {
        /// The repeated stable identity.
        id: crate::tools::ToolId,
    },

    /// Two prompt-local aliases selected the same stable tool identity.
    #[error(
        "tool identity {id:?} was selected by both aliases {first_alias:?} and {second_alias:?}"
    )]
    #[non_exhaustive]
    ToolIdSelectedTwice {
        /// The stable identity selected more than once.
        id: crate::tools::ToolId,
        /// The first alias in declaration order.
        first_alias: String,
        /// The later conflicting alias.
        second_alias: String,
    },

    /// A picker-selected stable identity is not callable in the live registry.
    #[error(
        "alias {alias:?} selected tool identity {id:?}, which is absent from the live registry"
    )]
    #[non_exhaustive]
    PickedToolNotLive {
        /// The prompt-local alias whose selection cannot be fulfilled.
        alias: String,
        /// The selected stable identity absent from the registry.
        id: crate::tools::ToolId,
    },

    /// The picker could not analyze the selected tool identities.
    #[error("selected tool-scope analysis failure: {detail}")]
    #[non_exhaustive]
    ToolScopeAnalysis {
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// The picker's near-duplicate analysis of the selected tool scope failed,
    /// retaining the picker's typed selection error as the private `#[source]`
    /// cause (F5) rather than flattening it into a string.
    #[error("selected tool-scope analysis failure")]
    #[non_exhaustive]
    ToolScopeAnalysisSource {
        /// The picker's typed selection failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// Two tools in one model-visible scope are semantic near-duplicates.
    #[error(
        "tool aliases {first_alias:?} ({first_id:?}) and {second_alias:?} ({second_id:?}) are near-duplicates with similarity {similarity}",
        first_alias = diagnostic.first_alias,
        first_id = diagnostic.first_id,
        second_alias = diagnostic.second_alias,
        second_id = diagnostic.second_id,
        similarity = diagnostic.similarity,
    )]
    #[non_exhaustive]
    NearDuplicateTools {
        /// The complete pair diagnostic, boxed to keep every crate error small.
        /// The diagnostic vocabulary lives in tool-scope validation (F10).
        diagnostic: Box<crate::tools::NearDuplicateDiagnostic>,
    },

    /// The concrete picker failed while resolving a model capability declaration.
    #[error("model capability binding failure for {capability:?}: {detail}")]
    #[non_exhaustive]
    ModelBind {
        /// The exact capability description passed to `models.need`.
        capability: String,
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// The picker's rebuild or resolve failed while binding a model capability,
    /// retaining the picker's own typed error as the private `#[source]` cause
    /// (model/resolver F5) rather than flattening it into a `detail` string, so
    /// the failure chain survives the resolution path.
    #[error("model capability binding failure for {capability:?}: {source}")]
    #[non_exhaustive]
    ModelBindQuery {
        /// The exact capability description passed to `models.need`.
        capability: String,
        /// The picker's typed rebuild/resolve failure, kept as a shareable cause.
        #[source]
        source: SharedSource,
    },

    /// No catalog entry matched a declared model capability under its constraints.
    #[error("no model matches capability {capability:?}")]
    #[non_exhaustive]
    ModelAbsent {
        /// The exact capability description passed to `models.need`.
        capability: String,
    },

    /// One server published duplicate model matches for a declared capability.
    #[error("duplicate models match capability {capability:?}: {candidates:?}")]
    #[non_exhaustive]
    ModelDuplicate {
        /// The exact capability description passed to `models.need`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<crate::model::ModelId>,
    },

    /// The picker could not choose uniquely among model capability matches.
    #[error("ambiguous models match capability {capability:?}: {candidates:?}")]
    #[non_exhaustive]
    ModelAmbiguous {
        /// The exact capability description passed to `models.need`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<crate::model::ModelId>,
    },

    /// One prompt-local model alias was declared more than once.
    #[error("model alias {alias:?} was declared more than once")]
    #[non_exhaustive]
    DuplicateModelAlias {
        /// The exact case-sensitive alias declared by the prompt.
        alias: String,
    },

    /// A `{{ }}` prose substitution failed (unknown/missing path, unclosed).
    ///
    /// Carries a typed [`crate::subst::SubstitutionError`] with a stable kind,
    /// the byte offset of the offending placeholder, a bounded preview, and any
    /// preserved serialization source, rather than a flattened string.
    #[error("{0}")]
    Substitution(#[source] Box<crate::subst::SubstitutionError>),

    /// The tool-call loop ran its iteration cap without a final text reply.
    #[error("tool-call loop did not converge")]
    ToolLoopExhausted,

    /// The model (or Lua) referenced a tool outside the VM's scoped aliases.
    #[error("tool {name:?} is not in this section's scope; in-scope aliases: {in_scope:?}{}", if *.global_exists { " (alias was declared by tools.need but not added to this section's scope)" } else { "" })]
    #[non_exhaustive]
    OutOfScopeToolCall {
        /// The alias or identifier the model/Lua code tried to use.
        name: String,
        /// Whether the name exists in the prompt-wide `tools.need` map.
        global_exists: bool,
        /// The aliases that are in scope for this VM.
        in_scope: Vec<String>,
    },

    /// A section's Lua prologue scoped a tool by a name absent from the run's
    /// supplied tool pool, so no matching tool could be advertised or dispatched.
    #[error("section scoped unknown tool {0}")]
    UnknownScopedTool(String),

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
    #[error("unsupported promptforge version: {0} (this build supports major 1)")]
    UnsupportedVersion(u32),

    /// No registered dialect matched the provided evidence.
    #[error("no tool dialect matched the provided evidence")]
    DialectNone,

    /// Two or more dialects tied for the highest detection score.
    #[error("tool dialect detection tied among: {candidates:?}")]
    #[non_exhaustive]
    DialectTie {
        /// The dialect identifiers that shared the top score.
        candidates: Vec<crate::dialects::ToolDialectId>,
    },

    /// `CompletionOptions.tool_dialect` named a dialect not in the registry.
    #[error("unknown tool dialect: {0}")]
    UnknownDialect(crate::dialects::ToolDialectId),

    /// A dispatched [`crate::tools::Tool`] returned a model-safe failure.
    ///
    /// The tool's own [`crate::tools::ToolError`] is preserved as the
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

    /// A spawned fanout arm task failed to join (it panicked or was aborted
    /// abnormally rather than returning a normal `Result`).
    ///
    /// The [`tokio::task::JoinError`] is preserved as the `#[source]` cause so
    /// the structured join failure survives; it is only stringified at the outer
    /// Lua callback boundary, never here.
    #[error("fanout arm join failed")]
    FanoutArmJoin(#[source] tokio::task::JoinError),

    /// An internal runtime invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur). Surfaced as a concrete error rather
    /// than silently skipping work, so an impossible state cannot masquerade as a
    /// successful fall-through.
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),

    /// A supplied tool's transport wire name was not legal when binding the
    /// live registry, retaining the structured [`crate::tools::ToolRegistryError`]
    /// as the private `#[source]` cause (tools AUDIT-DISCARDED-SOURCE) so the
    /// rejected name and reason survive instead of being flattened to a bare
    /// `&'static str`.
    #[error("internal invariant violated: invalid tool wire name")]
    #[non_exhaustive]
    InvalidToolWireName {
        /// The originating registry validation failure, kept as the cause.
        #[source]
        source: BoxedSource,
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

    /// Rendering the current time as an RFC 3339 string failed.
    ///
    /// Retains the [`time::error::Format`] failure as the private `#[source]`
    /// cause (execute source-audit discarded-error-002) rather than mapping
    /// every formatter failure to a source-free [`Error::Internal`], so the
    /// concrete formatting cause survives.
    #[error("could not format the current time as RFC 3339")]
    TimestampFormat(#[source] time::error::Format),
}

/// Stable messages emitted by Lua host-quota refusals.
///
/// Kept as constants so [`crate::lua`] emits them and the runtime-error boundary
/// recognizes them, mapping the refusal to the typed [`Error::LuaQuota`].
pub(crate) mod lua_quota {
    /// Log event-count budget exhausted.
    pub(crate) const LOG_EVENT: &str = "lua log event budget exceeded";
    /// Cumulative log byte budget exhausted.
    pub(crate) const LOG_BYTE: &str = "lua log cumulative byte budget exceeded";
    /// Per-VM instruction budget exhausted.
    pub(crate) const INSTRUCTION: &str = "lua instruction budget exceeded";
}

impl Error {
    /// Builds a parse failure with a stable classification and no source span.
    pub(crate) fn parse(kind: crate::parser::ParseErrorKind, message: impl Into<String>) -> Error {
        Error::ParseStructured {
            kind,
            span: None,
            message: message.into(),
        }
    }

    /// Wrap a transport-layer error, hiding its concrete type from the API.
    pub(crate) fn http(source: reqwest::Error) -> Error {
        Error::Http(Box::new(source))
    }

    /// Wrap an `mlua` failure as [`Error::LuaRuntime`], preserving it as the
    /// `#[source]` cause (F4) rather than flattening it to a string.
    pub(crate) fn lua(source: mlua::Error) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source),
        }
    }

    /// Wrap a tool failure, preserving the tool's own error as the `#[source]`
    /// cause rather than discarding it.
    pub(crate) fn tool(source: crate::tools::ToolError) -> Error {
        Error::Tool {
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

/// Crate-internal result alias over the [`Error`] substrate.
pub(crate) type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

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
    fn source_bearing_binding_errors_preserve_their_cause() {
        // F5: the binding and tool-scope failures keep the originating typed
        // error as a private `source()` instead of flattening it to a string,
        // and the chain survives through the public `RunError` wrapper.
        let schema_error = crate::client::ToolSchemaError::NonObjectSchema {
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

        let analysis = Error::ToolScopeAnalysisSource {
            source: Box::new(std::io::Error::other("picker selection failed")),
        };
        assert_source_survives_run_error(analysis);
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
    fn invalid_tool_wire_name_preserves_the_registry_error_and_its_context() {
        // tools AUDIT-DISCARDED-SOURCE: converting the registry error keeps the
        // rejected name and reason reachable through the private source instead
        // of collapsing to a bare `&'static str`.
        let registry = crate::tools::ToolRegistryError::InvalidWireName {
            wire_name: "bad name!".to_owned(),
            reason: "may contain only [A-Za-z0-9_.-]",
        };
        let error = Error::from(registry);
        let source = std::error::Error::source(&error).expect("registry cause preserved");
        assert!(
            source.to_string().contains("bad name!"),
            "the rejected wire name must survive on the source: {source}"
        );
        assert_source_survives_run_error(error);
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
    fn config_errors_preserve_the_secret_and_url_causes() {
        // client :419 / AUDIT-DISCARDED-SOURCE: an unusable credential and a bad
        // endpoint URL both retain their concrete cause through the public
        // CompletionError::source, classified as Config.
        use crate::client::{GatewayEndpoint, SecretString};
        use crate::model::{CompletionError, CompletionErrorKind};

        let secret_error = SecretString::new("").expect_err("blank key is rejected");
        let completion = CompletionError::from(secret_error);
        assert_eq!(completion.kind(), CompletionErrorKind::Config);
        assert!(
            std::error::Error::source(&completion).is_some(),
            "the SecretError cause must survive"
        );

        let url_error = GatewayEndpoint::new("not a url").expect_err("malformed URL is rejected");
        assert_eq!(url_error.kind(), CompletionErrorKind::Config);
        assert!(
            std::error::Error::source(&url_error).is_some(),
            "the url::ParseError cause must survive"
        );
    }

    #[test]
    fn model_bind_query_preserves_the_picker_cause() {
        // model/resolver F5: a picker rebuild/resolve failure keeps the concrete
        // picker error as a shareable private source rather than a `detail`
        // string.
        let bind = Error::ModelBindQuery {
            capability: "a fast model".to_owned(),
            source: SharedSource::new(std::io::Error::other("picker rebuild failed")),
        };
        assert_source_survives_run_error(bind);
    }
}
