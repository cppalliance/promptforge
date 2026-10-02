//! The suites' observation vocabulary: [`Observation`], the payload-free
//! view of a lifecycle [`Event`](promptforge_types::event::Event), the
//! named constants in the test-only `detail` module that the suites'
//! expected sequences spell, and the stable trace rendering a recorder
//! stores.
//!
//! Kept beside the recorder rather than in it so each file stays inside
//! the repository's line ceiling; the recorder re-exports this module's
//! public items, so a suite names `recording::Observation`.

use std::fmt;

use promptforge_types::ids::{AbandonReason, TaskId, TaskOrigin};
use serde_json::Value;

/// The payload-free observations as named constants, the spelling the
/// suites' expected sequences use.
#[cfg(test)]
#[expect(
    dead_code,
    reason = "the constants name the whole observation vocabulary; a suite uses the subset it asserts on"
)]
pub(crate) mod detail {
    use super::Observation;

    macro_rules! constants {
        ($($name:ident => $variant:ident),* $(,)?) => {
            $(
                #[doc = concat!("The [`Observation::", stringify!($variant), "`] boundary.")]
                pub(crate) const $name: Observation = Observation::$variant;
            )*
        };
    }

    constants! {
        PARSE_STARTED => ParseStarted,
        PARSE_SUCCEEDED => ParseSucceeded,
        PARSE_FAILED => ParseFailed,
        RUN_STARTED => RunStarted,
        RUN_SUCCEEDED => RunSucceeded,
        RUN_FAILED => RunFailed,
        SECTION_STARTED => SectionStarted,
        SECTION_FINISHED => SectionFinished,
        MODEL_TURN_COMPLETED => ModelTurnCompleted,
        MODEL_TURN_FAILED => ModelTurnFailed,
        MODEL_TURN_TRUNCATED => ModelTurnTruncated,
        TOOL_CALL_SUCCEEDED => ToolCallSucceeded,
        TOOL_CALL_FAILED => ToolCallFailed,
        LUA_COMPILATION_STARTED => LuaCompilationStarted,
        LUA_COMPILATION_SUCCEEDED => LuaCompilationSucceeded,
        LUA_COMPILATION_FAILED => LuaCompilationFailed,
        LUA_SHARED_LOAD_STARTED => LuaSharedLoadStarted,
        LUA_SHARED_LOAD_SUCCEEDED => LuaSharedLoadSucceeded,
        LUA_SHARED_LOAD_FAILED => LuaSharedLoadFailed,
        LUA_CHUNK_STARTED => LuaChunkStarted,
        LUA_CHUNK_SUCCEEDED => LuaChunkSucceeded,
        LUA_CHUNK_FAILED => LuaChunkFailed,
        LUA_REPLY_BINDING_STARTED => LuaReplyBindingStarted,
        LUA_REPLY_BINDING_SUCCEEDED => LuaReplyBindingSucceeded,
        LUA_REPLY_BINDING_FAILED => LuaReplyBindingFailed,
        LUA_TEARDOWN_STARTED => LuaTeardownStarted,
        LUA_TEARDOWN_SUCCEEDED => LuaTeardownSucceeded,
        TOOL_SCOPE_VALIDATION_STARTED => ToolScopeValidationStarted,
        TOOL_SCOPE_VALIDATION_SUCCEEDED => ToolScopeValidationSucceeded,
        TOOL_SCOPE_VALIDATION_FAILED => ToolScopeValidationFailed,
        MODEL_CATALOG_VALIDATION_STARTED => ModelCatalogValidationStarted,
        MODEL_CATALOG_VALIDATION_SUCCEEDED => ModelCatalogValidationSucceeded,
        MODEL_CATALOG_VALIDATION_FAILED => ModelCatalogValidationFailed,
        VFS_WRITE_SUCCEEDED => VfsWriteSucceeded,
        VFS_WRITE_FAILED => VfsWriteFailed,
        VFS_APPEND_SUCCEEDED => VfsAppendSucceeded,
        VFS_APPEND_FAILED => VfsAppendFailed,
        VFS_READ_SUCCEEDED => VfsReadSucceeded,
        VFS_READ_FAILED => VfsReadFailed,
        VFS_READ_NUMBERED_SUCCEEDED => VfsReadNumberedSucceeded,
        VFS_READ_NUMBERED_FAILED => VfsReadNumberedFailed,
        VFS_REPLACE_SUCCEEDED => VfsReplaceSucceeded,
        VFS_REPLACE_FAILED => VfsReplaceFailed,
        VFS_DELETE_SUCCEEDED => VfsDeleteSucceeded,
        VFS_DELETE_FAILED => VfsDeleteFailed,
        VFS_GLOB_SUCCEEDED => VfsGlobSucceeded,
        VFS_GLOB_FAILED => VfsGlobFailed,
        VFS_EXISTS_SUCCEEDED => VfsExistsSucceeded,
        VFS_EXISTS_FAILED => VfsExistsFailed,
    }
}

/// One typed operational observation, as the suites name it: the
/// payload-free view of a lifecycle [`Event`](promptforge_types::event::Event),
/// the author's `log` checkpoint, or a task boundary with its seeds.
///
/// Every fixed variant maps 1:1 to an event variant; its [`Display`](fmt::Display)
/// rendering is the stable trace string the suites compare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Observation {
    /// Prompt parsing began.
    ParseStarted,
    /// Prompt parsing and parse-time compilation completed successfully.
    ParseSucceeded,
    /// Prompt parsing or parse-time compilation returned an error.
    ParseFailed,
    /// A run passed its version gate and began.
    RunStarted,
    /// A run returned a value.
    RunSucceeded,
    /// A run returned an error.
    RunFailed,
    /// A top-level section began.
    SectionStarted,
    /// A top-level section completed successfully.
    SectionFinished,
    /// A model round trip completed successfully.
    ModelTurnCompleted,
    /// A model round trip returned an error.
    ModelTurnFailed,
    /// A successful parse ended because the model hit its length limit.
    ModelTurnTruncated,
    /// A tool dispatch completed successfully.
    ToolCallSucceeded,
    /// A tool dispatch returned an error.
    ToolCallFailed,
    /// Lua source compilation began.
    LuaCompilationStarted,
    /// Lua source compilation completed successfully.
    LuaCompilationSucceeded,
    /// Lua source compilation returned an error.
    LuaCompilationFailed,
    /// A section VM began loading and executing its shared program.
    LuaSharedLoadStarted,
    /// A section VM loaded and executed its shared program successfully.
    LuaSharedLoadSucceeded,
    /// A section VM failed to load or execute its shared program.
    LuaSharedLoadFailed,
    /// A section VM began executing a Lua chunk.
    LuaChunkStarted,
    /// A section VM executed a Lua chunk successfully.
    LuaChunkSucceeded,
    /// A section VM failed to execute a Lua chunk.
    LuaChunkFailed,
    /// A section VM began binding a model reply.
    LuaReplyBindingStarted,
    /// A section VM bound a model reply successfully.
    LuaReplyBindingSucceeded,
    /// A section VM failed to bind a model reply.
    LuaReplyBindingFailed,
    /// A section VM began teardown.
    LuaTeardownStarted,
    /// A section VM completed teardown.
    LuaTeardownSucceeded,
    /// Semantic validation of a model-visible tool scope began.
    ToolScopeValidationStarted,
    /// A model-visible tool scope passed semantic validation.
    ToolScopeValidationSucceeded,
    /// A model-visible tool scope failed semantic validation.
    ToolScopeValidationFailed,
    /// Live-catalog model binding validation began.
    ModelCatalogValidationStarted,
    /// Live-catalog model binding validation succeeded.
    ModelCatalogValidationSucceeded,
    /// Live-catalog model binding validation failed.
    ModelCatalogValidationFailed,
    /// A harness-mediated store write succeeded.
    VfsWriteSucceeded,
    /// A harness-mediated store write failed.
    VfsWriteFailed,
    /// A harness-mediated store append succeeded.
    VfsAppendSucceeded,
    /// A harness-mediated store append failed.
    VfsAppendFailed,
    /// A harness-mediated store read (verbatim) succeeded.
    VfsReadSucceeded,
    /// A harness-mediated store read (verbatim) failed.
    VfsReadFailed,
    /// A harness-mediated store read_numbered succeeded.
    VfsReadNumberedSucceeded,
    /// A harness-mediated store read_numbered failed.
    VfsReadNumberedFailed,
    /// A harness-mediated store replacement succeeded.
    VfsReplaceSucceeded,
    /// A harness-mediated store replacement failed.
    VfsReplaceFailed,
    /// A harness-mediated store deletion succeeded.
    VfsDeleteSucceeded,
    /// A harness-mediated store deletion failed.
    VfsDeleteFailed,
    /// A harness-mediated store glob succeeded.
    VfsGlobSucceeded,
    /// A harness-mediated store glob failed.
    VfsGlobFailed,
    /// A harness-mediated store existence check succeeded.
    VfsExistsSucceeded,
    /// A harness-mediated store existence check failed.
    VfsExistsFailed,
    /// A task chain was started; the payload is its spawn seeds.
    TaskStarted {
        /// The task's id.
        task: TaskId,
        /// The name of the section the task's chain starts at.
        target: String,
        /// The principal that started the task.
        origin: TaskOrigin,
        /// The `opts.input` override, when given.
        input: Option<String>,
        /// The `opts.item` seed, when given.
        item: Option<Value>,
        /// The `opts.index` seed, when given.
        index: Option<u64>,
        /// The spawner's `var` snapshot.
        var: Value,
    },
    /// Terminal: a task's chain ended with a result.
    TaskSucceeded {
        /// The task's id.
        task: TaskId,
    },
    /// Terminal: a task's chain ended with an error.
    TaskFailed {
        /// The task's id.
        task: TaskId,
    },
    /// Terminal: the task was cancelled on purpose by its owner.
    TaskCancelled {
        /// The task's id.
        task: TaskId,
    },
    /// Terminal: the task's owner chain ended while the task was live.
    TaskAbandoned {
        /// The task's id.
        task: TaskId,
        /// How the owner ended.
        reason: AbandonReason,
    },
    /// The one author-controlled checkpoint: a validated Lua `log(message)`.
    Lua(String),
    /// Any other event, by its trace line.
    Other(String),
}

impl Observation {
    /// Returns the fixed trace label for a fixed variant, or `None` for
    /// [`Observation::Lua`] / [`Observation::Other`], which hold a message.
    #[must_use]
    pub fn label(&self) -> Option<&'static str> {
        let label = match self {
            Observation::ParseStarted => "Parse started",
            Observation::ParseSucceeded => "Parse succeeded",
            Observation::ParseFailed => "Parse failed",
            Observation::RunStarted => "Run started",
            Observation::RunSucceeded => "Run succeeded",
            Observation::RunFailed => "Run failed",
            Observation::SectionStarted => "Section started",
            Observation::SectionFinished => "Section finished",
            Observation::ModelTurnCompleted => "Model turn completed",
            Observation::ModelTurnFailed => "Model turn failed",
            Observation::ModelTurnTruncated => "Model turn truncated",
            Observation::ToolCallSucceeded => "Tool call succeeded",
            Observation::ToolCallFailed => "Tool call failed",
            Observation::LuaCompilationStarted => "Lua compilation started",
            Observation::LuaCompilationSucceeded => "Lua compilation succeeded",
            Observation::LuaCompilationFailed => "Lua compilation failed",
            Observation::LuaSharedLoadStarted => "Lua shared load started",
            Observation::LuaSharedLoadSucceeded => "Lua shared load succeeded",
            Observation::LuaSharedLoadFailed => "Lua shared load failed",
            Observation::LuaChunkStarted => "Lua chunk started",
            Observation::LuaChunkSucceeded => "Lua chunk succeeded",
            Observation::LuaChunkFailed => "Lua chunk failed",
            Observation::LuaReplyBindingStarted => "Lua reply binding started",
            Observation::LuaReplyBindingSucceeded => "Lua reply binding succeeded",
            Observation::LuaReplyBindingFailed => "Lua reply binding failed",
            Observation::LuaTeardownStarted => "Lua teardown started",
            Observation::LuaTeardownSucceeded => "Lua teardown succeeded",
            Observation::ToolScopeValidationStarted => "Tool scope validation started",
            Observation::ToolScopeValidationSucceeded => "Tool scope validation succeeded",
            Observation::ToolScopeValidationFailed => "Tool scope validation failed",
            Observation::ModelCatalogValidationStarted => "Model catalog validation started",
            Observation::ModelCatalogValidationSucceeded => "Model catalog validation succeeded",
            Observation::ModelCatalogValidationFailed => "Model catalog validation failed",
            Observation::VfsWriteSucceeded => "Vfs write succeeded",
            Observation::VfsWriteFailed => "Vfs write failed",
            Observation::VfsAppendSucceeded => "Vfs append succeeded",
            Observation::VfsAppendFailed => "Vfs append failed",
            Observation::VfsReadSucceeded => "Vfs read succeeded",
            Observation::VfsReadFailed => "Vfs read failed",
            Observation::VfsReadNumberedSucceeded => "Vfs read_numbered succeeded",
            Observation::VfsReadNumberedFailed => "Vfs read_numbered failed",
            Observation::VfsReplaceSucceeded => "Vfs replace succeeded",
            Observation::VfsReplaceFailed => "Vfs replace failed",
            Observation::VfsDeleteSucceeded => "Vfs delete succeeded",
            Observation::VfsDeleteFailed => "Vfs delete failed",
            Observation::VfsGlobSucceeded => "Vfs glob succeeded",
            Observation::VfsGlobFailed => "Vfs glob failed",
            Observation::VfsExistsSucceeded => "Vfs exists succeeded",
            Observation::VfsExistsFailed => "Vfs exists failed",
            Observation::TaskStarted { .. } => "Task started",
            Observation::TaskSucceeded { .. } => "Task succeeded",
            Observation::TaskFailed { .. } => "Task failed",
            Observation::TaskCancelled { .. } => "Task cancelled",
            Observation::TaskAbandoned { .. } => "Task abandoned",
            Observation::Lua(_) | Observation::Other(_) => return None,
        };
        Some(label)
    }
}

impl fmt::Display for Observation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Observation::Lua(message) => write!(f, "Lua: {message}"),
            Observation::Other(message) => f.write_str(message),
            Observation::TaskAbandoned { reason, .. } => {
                write!(f, "Task abandoned: {}", reason.why())
            }
            fixed => f.write_str(fixed.label().unwrap_or_default()),
        }
    }
}
