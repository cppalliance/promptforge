//! The crate's internal error substrate.
//!
//! [`Error`] mirrors the role `promptforge-core`'s substrate plays there: it
//! is never part of the documented API. The executor's public boundary
//! (`promptforge_core::RunError`) wraps and classifies core's own substrate,
//! which maps this one back variant-for-variant through
//! `From<promptforge_lua::Error>`. The substrate is `#[doc(hidden)]` and
//! re-exported only so `promptforge-core` can perform that mapping verbatim;
//! it is not a stable API and is not marked `#[non_exhaustive]`, so the
//! mapping stays total.

use promptforge_gateway_client::Error as GatewayClientError;
use promptforge_gateway_client::model::ModelId;
use promptforge_tools::ToolId;

/// A type-erased owned error cause used by the internal substrate.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// A cloneable, shareable error cause.
///
/// Some caches re-produce a typed [`Error`] on every lookup (for example the
/// resolver decision cache), so a non-`Clone` dependency error cannot be moved
/// into a fresh [`Error`] each time. Wrapping it in a reference-counted
/// [`SharedSource`] lets the typed cause be retained as a `#[source]` and cloned
/// cheaply per lookup instead of being flattened to a string (resolve F4).
///
/// `promptforge-core`'s substrate carries this same type in its
/// `BindQuery`/`ModelBindQuery` variants, so the cross-crate mapping needs no
/// re-wrapping.
#[derive(Debug, Clone)]
#[doc(hidden)]
pub struct SharedSource(std::sync::Arc<dyn std::error::Error + Send + Sync>);

impl SharedSource {
    /// Wraps a concrete error as a shareable cause.
    #[doc(hidden)]
    #[must_use]
    pub fn new(source: impl std::error::Error + Send + Sync + 'static) -> SharedSource {
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

/// The crate's internal error substrate, spanning sandbox construction, host
/// bridging, capability binding, and Lua compile/runtime failures.
///
/// `#[doc(hidden)]`: this type exists in the public item tree only so the
/// companion `promptforge-core` crate can convert it back onto its own
/// substrate variant-for-variant. It is not host API.
#[derive(Debug, thiserror::Error)]
#[doc(hidden)]
pub enum Error {
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
    /// [`crate::LuaProgram::map_runtime_error`]), so the failure chain
    /// survives through the public wrappers' `source()` instead of being
    /// flattened to a string.
    #[error("{message}")]
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

    /// A Lua host resource quota (log events, log bytes, or instructions) was
    /// exhausted. A stable typed error rather than a bare `Lua(String)` so hosts
    /// can distinguish quota exhaustion from an authoring error.
    #[error("lua {resource} quota exceeded")]
    LuaQuota {
        /// The exhausted resource: `"log event"`, `"log byte"`, or `"instruction"`.
        resource: &'static str,
    },

    /// The host cancelled the run (for example Ctrl-C during fanout).
    #[error("interrupted by Ctrl-C")]
    Interrupted,

    /// An internal runtime invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur). Surfaced as a concrete error rather
    /// than silently skipping work, so an impossible state cannot masquerade as a
    /// successful fall-through.
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),

    /// One prompt-local alias was declared more than once.
    #[error("tool alias {alias:?} was declared more than once")]
    DuplicateAlias {
        /// The exact case-sensitive alias declared by the prompt.
        alias: String,
    },

    /// A picker-selected stable identity is not callable in the live tool
    /// catalog.
    #[error(
        "alias {alias:?} selected tool identity {id:?}, which is absent from the live tool catalog"
    )]
    PickedToolNotLive {
        /// The prompt-local alias whose selection cannot be fulfilled.
        alias: String,
        /// The selected stable identity absent from the catalog.
        id: ToolId,
    },

    /// Two prompt-local aliases selected the same stable tool identity.
    #[error(
        "tool identity {id:?} was selected by both aliases {first_alias:?} and {second_alias:?}"
    )]
    ToolIdSelectedTwice {
        /// The stable identity selected more than once.
        id: ToolId,
        /// The first alias in declaration order.
        first_alias: String,
        /// The later conflicting alias.
        second_alias: String,
    },

    /// The concrete picker failed while resolving a capability declaration.
    #[error("tool capability binding failure for {capability:?}: {detail}")]
    Bind {
        /// The exact capability description passed to `tools.bind`.
        capability: String,
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// The picker's query failed while resolving a capability, retaining the
    /// picker's own typed error as the private `#[source]` cause (resolve F4)
    /// so the failure chain survives the resolution cache instead of being
    /// flattened to a string.
    #[error("tool capability binding failure for {capability:?}: {source}")]
    BindQuery {
        /// The exact capability description passed to `tools.bind`.
        capability: String,
        /// The picker's typed query failure, kept as a shareable cause.
        #[source]
        source: SharedSource,
    },

    /// No picker catalog entry matched a declared capability.
    #[error("no tool matches capability {capability:?}")]
    Absent {
        /// The exact capability description passed to `tools.bind`.
        capability: String,
    },

    /// One server published duplicate matches for a declared capability.
    #[error("duplicate tools match capability {capability:?}: {candidates:?}")]
    Duplicate {
        /// The exact capability description passed to `tools.bind`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<ToolId>,
    },

    /// The picker could not choose uniquely among capability matches.
    #[error("ambiguous tools match capability {capability:?}: {candidates:?}")]
    Ambiguous {
        /// The exact capability description passed to `tools.bind`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<ToolId>,
    },

    /// The picker's near-duplicate analysis of the selected tool scope failed,
    /// retaining the picker's typed selection error as the private `#[source]`
    /// cause (F5) rather than flattening it into `detail`.
    #[error("selected tool-scope analysis failure")]
    ToolScopeAnalysisSource {
        /// The picker's typed selection failure, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// One prompt-local model alias was declared more than once.
    #[error("model alias {alias:?} was declared more than once")]
    DuplicateModelAlias {
        /// The exact case-sensitive alias declared by the prompt.
        alias: String,
    },

    /// The concrete picker failed while resolving a model capability declaration.
    #[error("model capability binding failure for {capability:?}: {detail}")]
    ModelBind {
        /// The exact capability description passed to `models.bind`.
        capability: String,
        /// The picker failure without exposing its concrete error type.
        detail: String,
    },

    /// The picker's rebuild or resolve failed while binding a model capability,
    /// retaining the picker's own typed error as the private `#[source]` cause
    /// (model/resolver F5) rather than flattening it into a `detail` string, so
    /// the failure chain survives the resolution path.
    #[error("model capability binding failure for {capability:?}: {source}")]
    ModelBindQuery {
        /// The exact capability description passed to `models.bind`.
        capability: String,
        /// The picker's typed rebuild/resolve failure, kept as a shareable cause.
        #[source]
        source: SharedSource,
    },

    /// No catalog entry matched a declared model capability under its constraints.
    #[error("no model matches capability {capability:?}")]
    ModelAbsent {
        /// The exact capability description passed to `models.bind`.
        capability: String,
    },

    /// One server published duplicate model matches for a declared capability.
    #[error("duplicate models match capability {capability:?}: {candidates:?}")]
    ModelDuplicate {
        /// The exact capability description passed to `models.bind`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<ModelId>,
    },

    /// The picker could not choose uniquely among model capability matches.
    #[error("ambiguous models match capability {capability:?}: {candidates:?}")]
    ModelAmbiguous {
        /// The exact capability description passed to `models.bind`.
        capability: String,
        /// The stable identities reported by the picker, in picker order.
        candidates: Vec<ModelId>,
    },
}

/// Stable messages emitted by Lua host-quota refusals.
///
/// Kept as constants so [`crate`] emits them and the runtime-error boundary
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
    /// Wrap an `mlua` failure as [`Error::LuaRuntime`], preserving it as the
    /// `#[source]` cause (F4) rather than flattening it to a string.
    pub(crate) fn lua(source: mlua::Error) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source),
        }
    }
}

/// Maps the gateway-client substrate onto this substrate. The model-binding
/// variants map variant-for-variant (they are the only ones a
/// `models.bind`/`models.default` resolution can produce), and
/// `ModelSetLock` flattens to [`Error::Lua`], matching the mapping
/// `promptforge-core` has always applied. Any remaining transport variant is
/// unreachable on the model-resolution path and degrades to its display
/// string rather than fabricating a classification.
impl From<GatewayClientError> for Error {
    fn from(error: GatewayClientError) -> Error {
        match error {
            GatewayClientError::ModelBind { capability, detail } => {
                Error::ModelBind { capability, detail }
            }
            GatewayClientError::ModelBindQuery { capability, source } => Error::ModelBindQuery {
                capability,
                source: SharedSource::new(source),
            },
            GatewayClientError::ModelAbsent { capability } => Error::ModelAbsent { capability },
            GatewayClientError::ModelDuplicate {
                capability,
                candidates,
            } => Error::ModelDuplicate {
                capability,
                candidates,
            },
            GatewayClientError::ModelAmbiguous {
                capability,
                candidates,
            } => Error::ModelAmbiguous {
                capability,
                candidates,
            },
            GatewayClientError::ModelSetLock(message) => Error::Lua(message),
            other => Error::Lua(other.to_string()),
        }
    }
}

/// Crate-internal result alias over the [`Error`] substrate.
pub(crate) type Result<T> = std::result::Result<T, Error>;
