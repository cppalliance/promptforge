//! The crate's internal error type.
//!
//! [`Error`] is a `pub(crate)` internal error type, never part of the public API.
//! Every public boundary returns its own typed error ([`crate::RunError`],
//! [`ParseError`](promptforge_parser::ParseError), [`CompletionError`](promptforge_model_client::model::CompletionError),
//! [`promptforge_types::tools::ToolError`]); those wrappers
//! classify this internal type and preserve its source. See the module wrappers for
//! the `From` bridges that let internal `?` keep flowing through the error type.

use promptforge_types::ids::TaskId;

mod convert;
#[cfg(test)]
mod tests;
mod value;

/// A type-erased owned error cause used by the internal error type.
type BoxedSource = Box<dyn std::error::Error + Send + Sync>;

/// Renders task ids as a comma-separated list: the [`Error::TasksLive`]
/// message and its `tasks` field.
fn join_task_ids(tasks: &[TaskId]) -> String {
    tasks
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// The crate's internal error type, spanning parsing, model, and execution
/// failures.
///
/// This type is `pub(crate)` and never appears in the public API; the public
/// boundary errors wrap and classify it. Marked `#[non_exhaustive]` so future
/// variants are not a breaking change. The model variant holds the broker's
/// closed error whole, so no HTTP client's error type leaks through the
/// wrappers' `source()`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub(crate) enum Error {
    /// The prompt frontmatter was not valid YAML, preserving the parser cause.
    ///
    /// This retains the originating YAML decode failure (a
    /// `serde_yaml_ng::Error`) as the `#[source]` cause so
    /// [`ParseError`](promptforge_parser::ParseError) can expose the frontmatter syntax location through
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
        /// retained cause's location when it has one.
        line: Option<u32>,
        /// The 1-based file column of the YAML failure, when known.
        column: Option<u32>,
    },

    /// A structurally-classified parse failure with a stable kind and an
    /// optional source byte span, so [`ParseError`](promptforge_parser::ParseError) can expose the
    /// classification and location from stored fields instead of inferring them
    /// from message text.
    #[error("{message}")]
    #[non_exhaustive]
    ParseStructured {
        /// The stable classification of this parse failure.
        kind: crate::parser::ParseErrorKind,
        /// The byte span of the offending region, when known, relative to the
        /// document body after the frontmatter and a leading BOM, with CRLF
        /// normalized to LF.
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

    /// A model round or a catalog fetch failed, as the broker that performed
    /// it reported.
    ///
    /// Holds the broker's [`CompletionError`](promptforge_model_client::model::CompletionError)
    /// whole, so the closed kind, the fixed message, the opt-in detail, and
    /// the cause chain survive the public wrappers. The Engine branches on
    /// its kind only: a context overflow takes the provider overflow path
    /// and an empty reply takes the empty-answer path.
    #[error(transparent)]
    Completion(crate::model::CompletionError),

    /// The run was cancelled, or the caller dropped a call the run was
    /// waiting on.
    #[error("interrupted: the run was cancelled or this call was stopped")]
    Interrupted,

    /// A section's Lua phase failed an Engine contract or hit a poisoned lock: a
    /// runtime-internal condition with no originating `mlua` error to preserve
    /// (for example "Engine values have not been injected" or a poisoned mutex).
    ///
    /// Failures that *do* have an `mlua` cause use [`Error::LuaRuntime`], which
    /// retains that cause as a private source. The message is the specific
    /// failure as a noun phrase; the public wrapper classifies this as a Lua
    /// failure, so no redundant `lua error:` type label is prepended.
    #[error("{0}")]
    Lua(String),

    /// A section's Lua phase failed at runtime or while bridging Engine values,
    /// retaining the originating `mlua` error as the private `#[source]` cause
    /// alongside the mapped prompt-location message.
    ///
    /// This is the source-bearing counterpart to [`Error::Lua`]: the Lua
    /// crate builds it from a concrete `mlua::Error` (see
    /// [`crate::lua::LuaProgram::map_runtime_error`]) and the conversion from
    /// that crate's error keeps it intact, so the failure chain survives
    /// through the public wrappers' `source()` instead of being flattened to
    /// a string.
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
    /// cause alongside the location metadata, so the compiler diagnostic
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
    /// the schema validation error as the private `#[source]` cause rather
    /// than flattening it into `detail`.
    ///
    /// Constructed only by the tool-scope preparation.
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
    /// Holds a typed [`crate::subst::SubstitutionError`] with a stable kind,
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
    /// names the ids in spawn order; the Lua table lists them as `tasks`.
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
    /// resolves against the run's whole catalog and fails with
    /// [`Error::UnboundToolCall`] instead.
    #[error("tool {name:?} is not in this section's scope; in-scope aliases: {in_scope:?}{}", if *.global_exists { " (a catalog tool that was not offered in this section)" } else { "" })]
    #[non_exhaustive]
    OutOfScopeToolCall {
        /// The alias or identifier the model tried to use.
        name: String,
        /// Whether the name is the wire name or id of a tool the run
        /// offers.
        global_exists: bool,
        /// The aliases that are in scope for this VM.
        in_scope: Vec<String>,
    },

    /// A `tools.call` named no local tool and no tool in the run's catalog.
    ///
    /// Script-initiated dispatch resolves a canonical id against the run's
    /// whole catalog, not the section's advertised scope - the scope shapes
    /// what the model is offered, and the author's own code is not the
    /// model - so this error means the name is no catalog tool's id at
    /// all; a wire name is the model's, never a script's.
    #[error("tool {name:?} is not a tool in this run; catalog tools: {ids:?}")]
    #[non_exhaustive]
    UnboundToolCall {
        /// The name the call tried to dispatch.
        name: String,
        /// The id of every tool the run offers.
        ids: Vec<String>,
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

    /// The environment cannot satisfy the prompt: a required Plugin is
    /// missing, unavailable, or needs a service the run lacks, the filled
    /// model fails a declared requirement (a context minimum or a hard
    /// keyword), or an H1 block failed the prompt's hard gate.
    ///
    /// The notice is the whole message, written to be read by a model.
    #[error("{notice}")]
    #[non_exhaustive]
    RequirementsUnmet {
        /// The model-readable refusal notice, one line per gap.
        notice: String,
    },

    /// A dispatched tool returned a model-safe failure.
    ///
    /// The tool's own [`promptforge_types::tools::ToolError`] is preserved as the
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

    /// A Lua resource quota (log events, log bytes, or instructions) was
    /// exhausted. This stable typed variant lets the caller tell quota
    /// exhaustion apart from an authoring error.
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

    /// A run-scoped store operation failed at the virtual filesystem layer,
    /// retaining the concrete [`promptforge_vfs::VfsError`] as the `#[source]`
    /// cause so a backend failure survives the public wrappers instead of
    /// being flattened to a string. The message is the model-facing
    /// rendering for the operation (or the fixed probe text for a run
    /// whose handle declares no store), kept beside the error because the
    /// error alone does not name the operation.
    #[error("{message}")]
    Store {
        /// The rendered store failure message, from the Lua crate's
        /// `store_error_message` or the fixed probe text.
        message: String,
        /// The structured failure the message renders.
        #[source]
        source: promptforge_vfs::VfsError,
    },

    /// Two execution identities claimed one store path: the claims
    /// model's conflict, mapped from the store's write-race vocabulary at
    /// the yield-answer boundary. Fatal to the run on the spot and never
    /// resumed into Lua, so no author `pcall` can catch it; the message is
    /// the claims model's whole diagnosis, naming the canonical path, both
    /// identities, and both claim kinds.
    #[error("store determinism violation: {0}")]
    Determinism(String),
}

/// Crate-internal result alias over [`Error`].
pub(crate) type Result<T> = std::result::Result<T, Error>;
