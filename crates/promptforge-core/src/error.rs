//! The crate's error type and result alias.

/// Diagnostics for two semantic near-duplicates exposed in one model turn.
#[derive(Debug)]
#[non_exhaustive]
pub struct NearDuplicateDiagnostic {
    /// The first prompt-local alias in picker catalog pair order.
    pub first_alias: String,
    /// The first stable identity.
    pub first_id: crate::tools::ToolId,
    /// The first concrete picker description.
    pub first_description: String,
    /// The first concrete picker behavioural hints.
    pub first_annotations: promptforge_tool_picker::ToolAnnotations,
    /// The second prompt-local alias in picker catalog pair order.
    pub second_alias: String,
    /// The second stable identity.
    pub second_id: crate::tools::ToolId,
    /// The second concrete picker description.
    pub second_description: String,
    /// The second concrete picker behavioural hints.
    pub second_annotations: promptforge_tool_picker::ToolAnnotations,
    /// The cosine similarity reported by the picker.
    pub similarity: f32,
}

/// The crate's error type, spanning parsing, HTTP, and execution failures.
///
/// Marked `#[non_exhaustive]` so future variants are not a breaking change. The
/// data-carrying tool-binding variants are likewise non-exhaustive so fields can
/// be added compatibly, and the transport variant hides its concrete source
/// type so no dependency's error leaks through this crate's public API.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The prompt file could not be parsed (bad frontmatter, no sections, etc.).
    #[error("parse error: {0}")]
    Parse(String),

    /// A required environment variable was missing.
    #[error("missing environment variable: {0}")]
    MissingEnv(String),

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

    /// A section's Lua phase failed to build, run, or return a usable value.
    #[error("lua error: {0}")]
    Lua(String),

    /// Lua source was not syntactically valid at its prompt location.
    #[error("lua compilation error at {location}: {message}")]
    LuaCompile {
        /// The prompt region supplied by the parser, such as a section preamble.
        location: String,
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

    /// A picker-selected model identity is absent from the live catalog.
    #[error(
        "alias {alias:?} selected model identity {id:?}, which is absent from the live catalog"
    )]
    #[non_exhaustive]
    PickedModelNotLive {
        /// The prompt-local alias whose selection cannot be fulfilled.
        alias: String,
        /// The selected stable identity absent from the catalog.
        id: crate::model::ModelId,
    },

    /// A `{{ }}` prose substitution failed (unknown/missing path, unclosed).
    #[error("substitution error: {0}")]
    Substitution(String),

    /// The tool-call loop ran its iteration cap without a final text reply.
    #[error("tool-call loop did not converge")]
    ToolLoopExhausted,

    /// The model asked to call a tool that was not provided to the executor.
    #[error("model called unknown tool {0}")]
    UnknownTool(String),

    /// A section's Lua preamble scoped a tool by a name absent from the run's
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

    /// A dialect operation that is not yet implemented (step 1 stub).
    #[error("dialect {dialect} has not implemented {operation}")]
    #[non_exhaustive]
    DialectNotImplemented {
        /// Which dialect was called.
        dialect: crate::dialects::ToolDialectId,
        /// Which operation was attempted.
        operation: &'static str,
    },
}

impl Error {
    /// Wrap a transport-layer error, hiding its concrete type from the API.
    pub(crate) fn http(source: reqwest::Error) -> Error {
        Error::Http(Box::new(source))
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, Error>;
