//! The crate's internal error substrate.
//!
//! [`Error`] is a `pub(crate)` substrate: it is never part of the public API.
//! Every public boundary returns its own typed error ([`crate::RunError`],
//! [`crate::ParseError`], [`crate::CompletionError`], [`crate::DialectError`],
//! [`crate::tools::ToolError`], [`crate::store::StoreError`]); those wrappers
//! classify this substrate and preserve its source. See the module wrappers for
//! the `From` bridges that let internal `?` keep flowing through the substrate.

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
    /// The prompt file could not be parsed (bad frontmatter, no sections, etc.).
    ///
    /// The message is the specific failure as a noun phrase; the public wrapper
    /// already conveys that this is a parse failure, so no redundant `parse
    /// error:` type label is prepended here (F8).
    #[error("{0}")]
    Parse(String),

    /// The prompt frontmatter was not valid YAML, preserving the parser cause.
    ///
    /// Unlike [`Error::Parse`], this retains the originating YAML decode failure
    /// (a [`serde_yaml::Error`]) as the `#[source]` cause (F3) so
    /// [`crate::ParseError::source`] can expose the frontmatter syntax location
    /// instead of flattening it into the message.
    #[error("invalid frontmatter: {message}")]
    #[non_exhaustive]
    ParseFrontmatter {
        /// The human-readable diagnostic (no raw source dump).
        message: String,
        /// The originating YAML parse failure, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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

    /// Gateway access was explicitly disabled by the host.
    #[error("gateway access is disabled")]
    GatewayDisabled,

    /// The HTTP request to the model backend failed at the transport layer.
    #[error("http transport failed")]
    Http(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The backend returned a non-success status.
    ///
    /// The `Display` is deliberately body-free (F5): the bounded, control-escaped
    /// body rides only in the private `body` field, reachable through the
    /// explicit [`crate::CompletionError::backend_body`] opt-in, so a raw or
    /// hostile payload cannot forge log lines or leak into an error message.
    #[error("backend returned {status}")]
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
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Reading a non-success backend response body failed at the transport
    /// layer.
    ///
    /// Retains the [`reqwest::Error`] as the `#[source]` cause (MODEL-010)
    /// rather than flattening the read failure into display text, so the error
    /// chain (timeout, connection reset) survives. The status the backend had
    /// already returned is preserved for classification.
    #[error("failed to read backend error body (status {status})")]
    #[non_exhaustive]
    BackendBodyRead {
        /// The non-success HTTP status whose body could not be read.
        status: u16,
        /// The originating transport read failure, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The model returned neither non-empty tool calls nor non-empty text.
    ///
    /// Reasoning side-channel text, when present, is never promoted into the
    /// answer; `detail` may note that it was ignored, without pasting it.
    #[error("{detail}")]
    #[non_exhaustive]
    EmptyModelReply {
        /// Fixed phrase naming the empty product (and ignored reasoning).
        detail: &'static str,
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
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// Lua source was not syntactically valid at its prompt location.
    #[error("lua compilation error at {location} (line {source_line}): {message}")]
    LuaCompile {
        /// The prompt region supplied by the parser, such as a section prologue.
        location: String,
        /// 1-based line number in the prompt source where this Lua region starts.
        source_line: u32,
        /// The retained source that failed to compile.
        lua_source: String,
        /// The Lua 5.4 compiler diagnostic.
        message: String,
    },

    /// The concrete picker failed while resolving a capability declaration.
    #[error("could not bind tool capability {capability:?}: {detail}")]
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
    #[error("could not build the model-facing schema for tool alias {alias:?}")]
    #[non_exhaustive]
    BindSchema {
        /// The prompt-local alias whose schema could not be built.
        alias: String,
        /// The originating schema validation failure, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The picker's query failed while resolving a capability, retaining the
    /// picker's own typed error as the private `#[source]` cause (resolve F4)
    /// so the failure chain survives the resolution cache instead of being
    /// flattened to a string.
    #[error("could not bind tool capability {capability:?}: {source}")]
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
    #[error("could not analyze the selected tool scope: {detail}")]
    #[non_exhaustive]
    ToolScopeAnalysis {
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// The picker's near-duplicate analysis of the selected tool scope failed,
    /// retaining the picker's typed selection error as the private `#[source]`
    /// cause (F5) rather than flattening it into a string.
    #[error("could not analyze the selected tool scope")]
    #[non_exhaustive]
    ToolScopeAnalysisSource {
        /// The picker's typed selection failure, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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
    #[error("could not bind model capability {capability:?}: {detail}")]
    #[non_exhaustive]
    ModelBind {
        /// The exact capability description passed to `models.need`.
        capability: String,
        /// The picker failure without exposing its concrete error type.
        detail: String,
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
    /// prompt-wide `models.always` binding.
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
    #[error("tool call failed: {message}")]
    Tool {
        /// The tool's model-safe failure message.
        message: String,
        /// The originating tool error, kept as the cause.
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
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

    /// A Lua host resource quota (log events, log bytes, or instructions) was
    /// exhausted. A stable typed error rather than a bare `Lua(String)` so hosts
    /// can distinguish quota exhaustion from an authoring error.
    #[error("lua {resource} quota exceeded")]
    #[non_exhaustive]
    LuaQuota {
        /// The exhausted resource: `"log event"`, `"log byte"`, or `"instruction"`.
        resource: &'static str,
    },
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
        assert!(
            std::error::Error::source(&bind).is_some(),
            "BindSchema must preserve the schema validation cause"
        );
        assert_eq!(
            bind.to_string(),
            "could not build the model-facing schema for tool alias \"echo\""
        );
        assert!(
            std::error::Error::source(&crate::RunError::from(bind)).is_some(),
            "the public RunError wrapper must keep the binding cause reachable"
        );

        let analysis = Error::ToolScopeAnalysisSource {
            source: Box::new(std::io::Error::other("picker selection failed")),
        };
        assert!(
            std::error::Error::source(&analysis).is_some(),
            "ToolScopeAnalysisSource must preserve the picker selection cause"
        );
        assert!(std::error::Error::source(&crate::RunError::from(analysis)).is_some());
    }
}
