//! The crate's internal error type.
//!
//! [`Error`] mirrors `promptforge-engine`'s own internal error type: it
//! is never part of the documented API. The executor's public boundary
//! (`promptforge_engine::RunError`) wraps and classifies core's own error type,
//! which maps this one back variant-for-variant through
//! `From<promptforge_lua::Error>`. The internal type is public only so
//! `promptforge-engine` can perform that mapping verbatim; the facade does
//! not re-export it, and it is not marked `#[non_exhaustive]`, so the
//! mapping stays total.

use promptforge_model_client::Error as GatewayClientError;

/// A type-erased owned error cause used by the internal error type.
pub(crate) type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// A cloneable, shareable error cause.
///
/// Some caches re-produce a typed [`Error`] on every lookup (for example the
/// resolver decision cache), so a non-`Clone` dependency error cannot be moved
/// into a fresh [`Error`] each time. Wrapping it in a reference-counted
/// [`SharedSource`] lets the typed cause be retained as a `#[source]` and cloned
/// cheaply per lookup instead of being flattened to a string (resolve F4).
/// The compiled-program statics (the coroutine shim and the messages
/// library) are the callers, through [`crate::detail::shared_source_new`].
#[derive(Debug, Clone)]
pub struct SharedSource(pub(crate) std::sync::Arc<dyn std::error::Error + Send + Sync>);

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

/// The crate's internal error type, spanning sandbox construction, host
/// bridging, capability binding, and Lua compile/runtime failures.
///
/// Public only so `promptforge-engine` can convert it back onto its own
/// internal type variant-for-variant; the facade does not re-export it.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A section's Lua phase failed a host contract or hit a poisoned lock: a
    /// runtime-internal condition with no originating `mlua` error to preserve
    /// (for example "host values have not been injected" or a poisoned mutex).
    ///
    /// Failures that *do* have an `mlua` cause use [`Error::LuaRuntime`], which
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
    /// from a concrete `mlua::Error` (see `Error::lua` and
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

    /// The selected compactor exhausted the model's context: a request
    /// overflowed the context window (the pre-dispatch precheck or a
    /// provider rejection) and the policy - `compactors.fail`, the only
    /// shipped one - does not compact. A stable typed error rather than a
    /// bare [`Error::Lua`] so hosts and `pcall` sites can distinguish
    /// context exhaustion from an authoring error.
    #[error("context exhausted: {reason}")]
    ContextExhausted {
        /// Which overflow check fired.
        reason: crate::compactors::OverflowReason,
    },

    /// The host cancelled the run (for example Ctrl-C during fanout).
    #[error("interrupted by Ctrl-C")]
    Interrupted,

    /// A dispatched tool's own failure, retaining the tool's typed error as
    /// the private `#[source]` cause rather than flattening it to a string.
    #[error("{message}")]
    Tool {
        /// The tool failure's rendered message.
        message: String,
        /// The tool's typed error, kept as the cause.
        #[source]
        source: BoxedSource,
    },

    /// An internal runtime invariant was violated (a state the surrounding code
    /// has already guaranteed cannot occur). Surfaced as a concrete error rather
    /// than silently skipping work, so an impossible state cannot masquerade as a
    /// successful fall-through.
    #[error("internal invariant violated: {0}")]
    Internal(&'static str),

    /// A structured error table raised in Lua surfaced as a block
    /// coroutine's failure with no retained typed error to substitute: the
    /// table's kind, message, and fields, kept rather than flattened to the
    /// message string. The executor maps it back onto its own error type by
    /// kind, so a shim raise classifies as the Rust-raised error it stands
    /// in for.
    #[error("{0}")]
    Raised(crate::error_value::Raised),
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
    /// Wraps an `mlua` failure as [`Error::LuaRuntime`], preserving it as the
    /// `#[source]` cause (F4) rather than flattening it to a string.
    pub(crate) fn lua(source: mlua::Error) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source),
        }
    }

    /// Re-produces a typed [`Error`] from a [`SharedSource`] a cache or
    /// static captured once and replays on every lookup (the compiled-program
    /// statics), cloning the `Arc` rather than flattening the cause to a
    /// string.
    pub(crate) fn shared(source: &SharedSource) -> Error {
        Error::LuaRuntime {
            message: source.to_string(),
            source: Box::new(source.clone()),
        }
    }

    /// Wraps a tool failure as [`Error::Tool`], preserving the tool's own
    /// error as the `#[source]` cause rather than discarding it.
    pub(crate) fn tool(source: promptforge_types::tools::ToolError) -> Error {
        Error::Tool {
            message: source.to_string(),
            source: Box::new(source),
        }
    }
}

/// Maps the gateway-client error type onto this one. `ModelSetLock`
/// flattens to [`Error::Lua`], matching the mapping `promptforge-engine`
/// has always applied. Any remaining transport variant is unreachable on the
/// model-resolution path and degrades to its display string rather than
/// fabricating a classification.
impl From<GatewayClientError> for Error {
    fn from(error: GatewayClientError) -> Error {
        match error {
            GatewayClientError::ModelSetLock(message) => Error::Lua(message),
            other => Error::Lua(other.to_string()),
        }
    }
}

/// Crate-internal result alias over [`Error`].
pub(crate) type Result<T> = std::result::Result<T, Error>;
