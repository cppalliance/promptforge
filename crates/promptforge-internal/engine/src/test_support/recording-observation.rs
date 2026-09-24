//! The suites' observation vocabulary: [`Observation`], the payload-free
//! view of a lifecycle [`Event`](promptforge_types::event::Event), the
//! named constants in [`detail`] the suites' expected sequences spell, and
//! the stable trace rendering a recorder stores.
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
        STORE_WRITE_SUCCEEDED => StoreWriteSucceeded,
        STORE_WRITE_FAILED => StoreWriteFailed,
        STORE_APPEND_SUCCEEDED => StoreAppendSucceeded,
        STORE_APPEND_FAILED => StoreAppendFailed,
        STORE_READ_SUCCEEDED => StoreReadSucceeded,
        STORE_READ_FAILED => StoreReadFailed,
        STORE_READ_NUMBERED_SUCCEEDED => StoreReadNumberedSucceeded,
        STORE_READ_NUMBERED_FAILED => StoreReadNumberedFailed,
        STORE_REPLACE_SUCCEEDED => StoreReplaceSucceeded,
        STORE_REPLACE_FAILED => StoreReplaceFailed,
        STORE_DELETE_SUCCEEDED => StoreDeleteSucceeded,
        STORE_DELETE_FAILED => StoreDeleteFailed,
        STORE_GLOB_SUCCEEDED => StoreGlobSucceeded,
        STORE_GLOB_FAILED => StoreGlobFailed,
        USER_INPUT_WAIT_STARTED => UserInputWaitStarted,
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
    StoreWriteSucceeded,
    /// A harness-mediated store write failed.
    StoreWriteFailed,
    /// A harness-mediated store append succeeded.
    StoreAppendSucceeded,
    /// A harness-mediated store append failed.
    StoreAppendFailed,
    /// A harness-mediated store read (verbatim) succeeded.
    StoreReadSucceeded,
    /// A harness-mediated store read (verbatim) failed.
    StoreReadFailed,
    /// A harness-mediated store read_numbered succeeded.
    StoreReadNumberedSucceeded,
    /// A harness-mediated store read_numbered failed.
    StoreReadNumberedFailed,
    /// A harness-mediated store replacement succeeded.
    StoreReplaceSucceeded,
    /// A harness-mediated store replacement failed.
    StoreReplaceFailed,
    /// A harness-mediated store deletion succeeded.
    StoreDeleteSucceeded,
    /// A harness-mediated store deletion failed.
    StoreDeleteFailed,
    /// A harness-mediated store glob succeeded.
    StoreGlobSucceeded,
    /// A harness-mediated store glob failed.
    StoreGlobFailed,
    /// A section began waiting on operator input.
    UserInputWaitStarted,
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
            Observation::StoreWriteSucceeded => "Store write succeeded",
            Observation::StoreWriteFailed => "Store write failed",
            Observation::StoreAppendSucceeded => "Store append succeeded",
            Observation::StoreAppendFailed => "Store append failed",
            Observation::StoreReadSucceeded => "Store read succeeded",
            Observation::StoreReadFailed => "Store read failed",
            Observation::StoreReadNumberedSucceeded => "Store read_numbered succeeded",
            Observation::StoreReadNumberedFailed => "Store read_numbered failed",
            Observation::StoreReplaceSucceeded => "Store replace succeeded",
            Observation::StoreReplaceFailed => "Store replace failed",
            Observation::StoreDeleteSucceeded => "Store delete succeeded",
            Observation::StoreDeleteFailed => "Store delete failed",
            Observation::StoreGlobSucceeded => "Store glob succeeded",
            Observation::StoreGlobFailed => "Store glob failed",
            Observation::UserInputWaitStarted => "User input wait started",
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
