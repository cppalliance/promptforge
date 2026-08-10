//! The crate's internal error substrate.
//!
//! [`Error`] is a `pub(crate)` substrate: it is never part of the public API.
//! Every public boundary returns its own typed error ([`crate::RunError`],
//! [`crate::ParseError`], [`crate::CompletionError`], [`crate::DialectError`],
//! [`crate::tools::ToolError`], [`crate::store::StoreError`]); those wrappers
//! classify this substrate and preserve its source. See the module wrappers for
//! the `From` bridges that let internal `?` keep flowing through the substrate.

/// Diagnostics for two semantic near-duplicates exposed in one model turn.
#[derive(Debug)]
#[non_exhaustive]
pub(crate) struct NearDuplicateDiagnostic {
    /// The first prompt-local alias in picker catalog pair order.
    pub(crate) first_alias: String,
    /// The first stable identity.
    pub(crate) first_id: crate::tools::ToolId,
    /// The second prompt-local alias in picker catalog pair order.
    pub(crate) second_alias: String,
    /// The second stable identity.
    pub(crate) second_id: crate::tools::ToolId,
    /// The cosine similarity reported by the picker.
    pub(crate) similarity: f32,
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
    #[error("parse error: {0}")]
    Parse(String),

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
    #[error("http transport error")]
    Http(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// The backend returned a non-success status.
    #[error("backend returned {status}: {body}")]
    Backend {
        /// The HTTP status code returned by the backend.
        status: u16,
        /// The (truncated) response body, for diagnostics.
        body: String,
    },

    /// The backend response could not be understood (missing choices, etc.).
    #[error("malformed response: {0}")]
    MalformedResponse(String),

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

    /// A section's Lua phase failed to build, run, or return a usable value.
    #[error("lua error: {0}")]
    Lua(String),

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
        diagnostic: Box<NearDuplicateDiagnostic>,
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
    #[error("substitution error: {0}")]
    Substitution(String),

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
}

impl Error {
    /// Wrap a transport-layer error, hiding its concrete type from the API.
    pub(crate) fn http(source: reqwest::Error) -> Error {
        Error::Http(Box::new(source))
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

/// Crate-internal result alias over the [`Error`] substrate.
pub(crate) type Result<T> = std::result::Result<T, Error>;
