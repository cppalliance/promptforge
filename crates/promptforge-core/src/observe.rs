//! Report-only observation for a run in flight.
//!
//! [`Observer`] receives a borrowed `(execution, section)` pair and one typed
//! [`Observation`] at operational boundaries. The observation is the complete
//! trace record. Fixed runtime observations carry no raw prompt prose, model
//! input or output, tool arguments or results, store paths or contents,
//! credentials, or fetched content. The sole author-controlled exception is a
//! validated [`Observation::Lua`] checkpoint from the phase-local Lua
//! `log(message)` callback. Reports are synchronous and never consulted for a
//! decision. [`NullObserver`] provides silence without a second execution path.

use std::fmt;

/// One typed operational observation emitted by the runtime.
///
/// Every fixed variant maps 1:1 to a fixed lifecycle boundary; its
/// [`Display`](fmt::Display) rendering is the stable trace string. A consumer
/// may match individual variants for cosmetic presentation, but must tolerate
/// unknown variants (this enum is `#[non_exhaustive]`) and must never use an
/// observation to steer execution.
///
/// [`Observation::Lua`] carries the one intentionally author-controlled
/// checkpoint (the Lua `log(message)` callback); [`Observation::Other`] is a
/// forward-compatible escape hatch. Both own their message, so an observation
/// crosses a thread boundary (fanout arms report through a channel) without
/// borrowing the emitting frame.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
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
    /// A section VM began executing its prologue.
    LuaPrologueStarted,
    /// A section VM executed its prologue successfully.
    LuaPrologueSucceeded,
    /// A section VM failed to execute its prologue.
    LuaPrologueFailed,
    /// A section VM began binding a model reply.
    LuaReplyBindingStarted,
    /// A section VM bound a model reply successfully.
    LuaReplyBindingSucceeded,
    /// A section VM failed to bind a model reply.
    LuaReplyBindingFailed,
    /// A section VM began executing its epilog.
    LuaEpilogStarted,
    /// A section VM executed its epilog successfully.
    LuaEpilogSucceeded,
    /// A section VM failed to execute its epilog.
    LuaEpilogFailed,
    /// A section VM began teardown.
    LuaTeardownStarted,
    /// A section VM completed teardown.
    LuaTeardownSucceeded,
    /// Live-registry and one-to-one binding validation began.
    ToolRegistryValidationStarted,
    /// Live-registry and one-to-one binding validation succeeded.
    ToolRegistryValidationSucceeded,
    /// Live-registry or one-to-one binding validation failed.
    ToolRegistryValidationFailed,
    /// A section began closing its effective tool scope.
    ToolScopeClosing,
    /// A section's effective tool scope was closed successfully.
    ToolScopeClosed,
    /// A section's effective tool scope could not be closed.
    ToolScopeFailed,
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
    /// A section began closing its model selection.
    ModelScopeClosing,
    /// A section's model selection was closed successfully.
    ModelScopeClosed,
    /// A section's model selection could not be closed.
    ModelScopeFailed,
    /// A harness-mediated store write succeeded.
    StoreWriteSucceeded,
    /// A harness-mediated store write failed.
    StoreWriteFailed,
    /// A harness-mediated store append succeeded.
    StoreAppendSucceeded,
    /// A harness-mediated store append failed.
    StoreAppendFailed,
    /// A harness-mediated store read_lines succeeded.
    StoreReadLinesSucceeded,
    /// A harness-mediated store read_lines failed.
    StoreReadLinesFailed,
    /// A harness-mediated store read (verbatim) succeeded.
    StoreReadSucceeded,
    /// A harness-mediated store read (verbatim) failed.
    StoreReadFailed,
    /// A harness-mediated store inject succeeded.
    StoreInjectSucceeded,
    /// A harness-mediated store inject failed.
    StoreInjectFailed,
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
    /// A fanout arm began execution.
    FanoutArmStarted,
    /// A fanout arm completed execution.
    FanoutArmFinished,
    /// The one author-controlled checkpoint: a validated Lua `log(message)`.
    ///
    /// Prompt authors must never place arguments, replies, tool data,
    /// credentials, paths, or store contents in this message.
    Lua(String),
    /// A forward-compatible escape hatch for an observation with no fixed
    /// variant.
    Other(String),
}

impl Observation {
    /// Returns the fixed trace label for a fixed variant, or `None` for the
    /// message-carrying [`Observation::Lua`] / [`Observation::Other`].
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
            Observation::LuaPrologueStarted => "Lua prologue started",
            Observation::LuaPrologueSucceeded => "Lua prologue succeeded",
            Observation::LuaPrologueFailed => "Lua prologue failed",
            Observation::LuaReplyBindingStarted => "Lua reply binding started",
            Observation::LuaReplyBindingSucceeded => "Lua reply binding succeeded",
            Observation::LuaReplyBindingFailed => "Lua reply binding failed",
            Observation::LuaEpilogStarted => "Lua epilog started",
            Observation::LuaEpilogSucceeded => "Lua epilog succeeded",
            Observation::LuaEpilogFailed => "Lua epilog failed",
            Observation::LuaTeardownStarted => "Lua teardown started",
            Observation::LuaTeardownSucceeded => "Lua teardown succeeded",
            Observation::ToolRegistryValidationStarted => "Tool registry validation started",
            Observation::ToolRegistryValidationSucceeded => "Tool registry validation succeeded",
            Observation::ToolRegistryValidationFailed => "Tool registry validation failed",
            Observation::ToolScopeClosing => "Tool scope closing",
            Observation::ToolScopeClosed => "Tool scope closed",
            Observation::ToolScopeFailed => "Tool scope failed",
            Observation::ToolScopeValidationStarted => "Tool scope validation started",
            Observation::ToolScopeValidationSucceeded => "Tool scope validation succeeded",
            Observation::ToolScopeValidationFailed => "Tool scope validation failed",
            Observation::ModelCatalogValidationStarted => "Model catalog validation started",
            Observation::ModelCatalogValidationSucceeded => "Model catalog validation succeeded",
            Observation::ModelCatalogValidationFailed => "Model catalog validation failed",
            Observation::ModelScopeClosing => "Model scope closing",
            Observation::ModelScopeClosed => "Model scope closed",
            Observation::ModelScopeFailed => "Model scope failed",
            Observation::StoreWriteSucceeded => "Store write succeeded",
            Observation::StoreWriteFailed => "Store write failed",
            Observation::StoreAppendSucceeded => "Store append succeeded",
            Observation::StoreAppendFailed => "Store append failed",
            Observation::StoreReadLinesSucceeded => "Store read_lines succeeded",
            Observation::StoreReadLinesFailed => "Store read_lines failed",
            Observation::StoreReadSucceeded => "Store read succeeded",
            Observation::StoreReadFailed => "Store read failed",
            Observation::StoreInjectSucceeded => "Store inject succeeded",
            Observation::StoreInjectFailed => "Store inject failed",
            Observation::StoreReplaceSucceeded => "Store replace succeeded",
            Observation::StoreReplaceFailed => "Store replace failed",
            Observation::StoreDeleteSucceeded => "Store delete succeeded",
            Observation::StoreDeleteFailed => "Store delete failed",
            Observation::StoreGlobSucceeded => "Store glob succeeded",
            Observation::StoreGlobFailed => "Store glob failed",
            Observation::FanoutArmStarted => "Fanout arm started",
            Observation::FanoutArmFinished => "Fanout arm finished",
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
            fixed => f.write_str(fixed.label().unwrap_or_default()),
        }
    }
}

/// Fixed observations emitted by the currently shipped runtime.
///
/// These crate-private constants let emit sites name a lifecycle boundary
/// (`detail::RUN_STARTED`) without repeating the enum path; each is exactly the
/// matching [`Observation`] variant.
pub(crate) mod detail {
    use super::Observation;

    pub(crate) const PARSE_STARTED: Observation = Observation::ParseStarted;
    pub(crate) const PARSE_SUCCEEDED: Observation = Observation::ParseSucceeded;
    pub(crate) const PARSE_FAILED: Observation = Observation::ParseFailed;
    pub(crate) const RUN_STARTED: Observation = Observation::RunStarted;
    pub(crate) const RUN_SUCCEEDED: Observation = Observation::RunSucceeded;
    pub(crate) const RUN_FAILED: Observation = Observation::RunFailed;
    pub(crate) const SECTION_STARTED: Observation = Observation::SectionStarted;
    pub(crate) const SECTION_FINISHED: Observation = Observation::SectionFinished;
    pub(crate) const MODEL_TURN_COMPLETED: Observation = Observation::ModelTurnCompleted;
    pub(crate) const MODEL_TURN_FAILED: Observation = Observation::ModelTurnFailed;
    pub(crate) const MODEL_TURN_TRUNCATED: Observation = Observation::ModelTurnTruncated;
    pub(crate) const TOOL_CALL_SUCCEEDED: Observation = Observation::ToolCallSucceeded;
    pub(crate) const TOOL_CALL_FAILED: Observation = Observation::ToolCallFailed;
    pub(crate) const LUA_COMPILATION_STARTED: Observation = Observation::LuaCompilationStarted;
    pub(crate) const LUA_COMPILATION_SUCCEEDED: Observation = Observation::LuaCompilationSucceeded;
    pub(crate) const LUA_COMPILATION_FAILED: Observation = Observation::LuaCompilationFailed;
    pub(crate) const LUA_SHARED_LOAD_STARTED: Observation = Observation::LuaSharedLoadStarted;
    pub(crate) const LUA_SHARED_LOAD_SUCCEEDED: Observation = Observation::LuaSharedLoadSucceeded;
    pub(crate) const LUA_SHARED_LOAD_FAILED: Observation = Observation::LuaSharedLoadFailed;
    pub(crate) const LUA_PROLOGUE_STARTED: Observation = Observation::LuaPrologueStarted;
    pub(crate) const LUA_PROLOGUE_SUCCEEDED: Observation = Observation::LuaPrologueSucceeded;
    pub(crate) const LUA_PROLOGUE_FAILED: Observation = Observation::LuaPrologueFailed;
    pub(crate) const LUA_REPLY_BINDING_STARTED: Observation = Observation::LuaReplyBindingStarted;
    pub(crate) const LUA_REPLY_BINDING_SUCCEEDED: Observation =
        Observation::LuaReplyBindingSucceeded;
    pub(crate) const LUA_REPLY_BINDING_FAILED: Observation = Observation::LuaReplyBindingFailed;
    pub(crate) const LUA_EPILOG_STARTED: Observation = Observation::LuaEpilogStarted;
    pub(crate) const LUA_EPILOG_SUCCEEDED: Observation = Observation::LuaEpilogSucceeded;
    pub(crate) const LUA_EPILOG_FAILED: Observation = Observation::LuaEpilogFailed;
    pub(crate) const LUA_TEARDOWN_STARTED: Observation = Observation::LuaTeardownStarted;
    pub(crate) const LUA_TEARDOWN_SUCCEEDED: Observation = Observation::LuaTeardownSucceeded;
    pub(crate) const TOOL_SCOPE_CLOSING: Observation = Observation::ToolScopeClosing;
    pub(crate) const TOOL_SCOPE_CLOSED: Observation = Observation::ToolScopeClosed;
    pub(crate) const TOOL_SCOPE_FAILED: Observation = Observation::ToolScopeFailed;
    pub(crate) const TOOL_SCOPE_VALIDATION_STARTED: Observation =
        Observation::ToolScopeValidationStarted;
    pub(crate) const TOOL_SCOPE_VALIDATION_SUCCEEDED: Observation =
        Observation::ToolScopeValidationSucceeded;
    pub(crate) const TOOL_SCOPE_VALIDATION_FAILED: Observation =
        Observation::ToolScopeValidationFailed;
    pub(crate) const MODEL_SCOPE_CLOSING: Observation = Observation::ModelScopeClosing;
    pub(crate) const MODEL_SCOPE_CLOSED: Observation = Observation::ModelScopeClosed;
    pub(crate) const MODEL_SCOPE_FAILED: Observation = Observation::ModelScopeFailed;
    pub(crate) const STORE_WRITE_SUCCEEDED: Observation = Observation::StoreWriteSucceeded;
    pub(crate) const STORE_WRITE_FAILED: Observation = Observation::StoreWriteFailed;
    pub(crate) const STORE_APPEND_SUCCEEDED: Observation = Observation::StoreAppendSucceeded;
    pub(crate) const STORE_APPEND_FAILED: Observation = Observation::StoreAppendFailed;
    pub(crate) const STORE_READ_LINES_SUCCEEDED: Observation = Observation::StoreReadLinesSucceeded;
    pub(crate) const STORE_READ_LINES_FAILED: Observation = Observation::StoreReadLinesFailed;
    pub(crate) const STORE_READ_SUCCEEDED: Observation = Observation::StoreReadSucceeded;
    pub(crate) const STORE_READ_FAILED: Observation = Observation::StoreReadFailed;
    pub(crate) const STORE_INJECT_SUCCEEDED: Observation = Observation::StoreInjectSucceeded;
    pub(crate) const STORE_INJECT_FAILED: Observation = Observation::StoreInjectFailed;
    pub(crate) const STORE_REPLACE_SUCCEEDED: Observation = Observation::StoreReplaceSucceeded;
    pub(crate) const STORE_REPLACE_FAILED: Observation = Observation::StoreReplaceFailed;
    pub(crate) const STORE_DELETE_SUCCEEDED: Observation = Observation::StoreDeleteSucceeded;
    pub(crate) const STORE_DELETE_FAILED: Observation = Observation::StoreDeleteFailed;
    pub(crate) const STORE_GLOB_SUCCEEDED: Observation = Observation::StoreGlobSucceeded;
    pub(crate) const STORE_GLOB_FAILED: Observation = Observation::StoreGlobFailed;
    pub(crate) const FANOUT_ARM_STARTED: Observation = Observation::FanoutArmStarted;
    pub(crate) const FANOUT_ARM_FINISHED: Observation = Observation::FanoutArmFinished;
}

/// A report-only sink for operational observations.
///
/// The runtime calls [`observe`](Self::observe) synchronously from the task
/// driving a run, so implementations must be `Send + Sync`, non-blocking, and
/// non-panicking. A forwarding implementation should copy the observation into
/// a queue and return rather than awaiting or performing I/O. Concrete
/// observers own synchronization; core provides no global observer lock and
/// holds no observer-owned guard across an await.
///
/// An observation is never read back by the runtime. Recording every report or
/// discarding all of them must leave outputs, errors, ordering, and side effects
/// unchanged.
///
/// # Examples
/// ```
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// use promptforge_core::observe::{Observation, Observer};
///
/// #[derive(Default)]
/// struct Counter(AtomicUsize);
///
/// impl Observer for Counter {
///     fn observe(&self, _execution: &str, _section: &str, _event: Observation) {
///         self.0.fetch_add(1, Ordering::Relaxed);
///     }
/// }
///
/// let counter = Counter::default();
/// counter.observe("example-run", "Gather", Observation::SectionFinished);
/// assert_eq!(counter.0.load(Ordering::Relaxed), 1);
/// ```
pub trait Observer: Send + Sync {
    /// Reports one typed [`Observation`] for `execution` and `section`.
    ///
    /// Fixed runtime observations carry no payloads or secrets. The only
    /// author-controlled variant is [`Observation::Lua`]; prompt authors must
    /// never put arguments, replies, tool data, credentials, paths, or store
    /// contents in it. Reports must not affect any execution decision.
    /// Implementations must return promptly and must not panic.
    fn observe(&self, execution: &str, section: &str, event: Observation);
}

/// An [`Observer`] that discards every observation.
///
/// This is what a caller wanting no progress passes, so the executor never
/// needs an `Option<&dyn Observer>` and never branches on one.
///
/// # Examples
/// ```
/// use promptforge_core::observe::{Observation, NullObserver, Observer};
///
/// let observer = NullObserver;
/// observer.observe("example-run", "Example prompt", Observation::RunSucceeded);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullObserver;

impl Observer for NullObserver {
    fn observe(&self, _execution: &str, _section: &str, _event: Observation) {}
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;

    #[test]
    fn null_observer_accepts_reports() {
        let observer = NullObserver;
        observer.observe("example-run", "Prompt", Observation::RunStarted);
        observer.observe("example-run", "Gather", Observation::SectionStarted);
        observer.observe("example-run", "Gather", Observation::SectionFinished);
        observer.observe("example-run", "Prompt", Observation::RunSucceeded);
    }

    #[test]
    fn display_renders_stable_strings() {
        assert_eq!(Observation::RunStarted.to_string(), "Run started");
        assert_eq!(
            Observation::StoreReadLinesSucceeded.to_string(),
            "Store read_lines succeeded"
        );
        assert_eq!(Observation::Lua("hi".to_owned()).to_string(), "Lua: hi");
        assert_eq!(Observation::Other("x".to_owned()).to_string(), "x");
        assert_eq!(Observation::RunStarted.label(), Some("Run started"));
        assert_eq!(Observation::Lua("hi".to_owned()).label(), None);
    }

    #[test]
    fn observer_is_dyn_compatible_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Observer>();

        let observer: &dyn Observer = &NullObserver;
        observer.observe("example-run", "Gather", Observation::SectionFinished);
    }

    #[test]
    fn mutex_recorder_keeps_interleaved_execution_ids_ordered() {
        #[derive(Default)]
        struct Recorder(Mutex<Vec<(String, String, Observation)>>);

        impl Observer for Recorder {
            fn observe(&self, execution: &str, section: &str, event: Observation) {
                self.0
                    .lock()
                    .expect("recorder mutex must remain usable")
                    .push((execution.to_owned(), section.to_owned(), event));
            }
        }

        let recorder = Arc::new(Recorder::default());
        let barrier = Arc::new(Barrier::new(2));
        let first_recorder = Arc::clone(&recorder);
        let first_barrier = Arc::clone(&barrier);
        let first = std::thread::spawn(move || {
            first_recorder.observe("execution-a", "First", detail::SECTION_STARTED);
            first_barrier.wait();
            first_barrier.wait();
            first_recorder.observe("execution-a", "First", detail::SECTION_FINISHED);
            first_barrier.wait();
            first_barrier.wait();
        });
        let second_recorder = Arc::clone(&recorder);
        let second = std::thread::spawn(move || {
            barrier.wait();
            second_recorder.observe("execution-b", "Second", detail::SECTION_STARTED);
            barrier.wait();
            barrier.wait();
            second_recorder.observe("execution-b", "Second", detail::SECTION_FINISHED);
            barrier.wait();
        });

        first.join().expect("first recording thread must finish");
        second.join().expect("second recording thread must finish");
        assert_eq!(
            *recorder
                .0
                .lock()
                .expect("recorder mutex must remain usable"),
            [
                (
                    "execution-a".to_owned(),
                    "First".to_owned(),
                    Observation::SectionStarted,
                ),
                (
                    "execution-b".to_owned(),
                    "Second".to_owned(),
                    Observation::SectionStarted,
                ),
                (
                    "execution-a".to_owned(),
                    "First".to_owned(),
                    Observation::SectionFinished,
                ),
                (
                    "execution-b".to_owned(),
                    "Second".to_owned(),
                    Observation::SectionFinished,
                ),
            ]
        );
    }
}
