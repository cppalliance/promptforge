//! Report-only observation for a run in flight.
//!
//! [`Observer`] receives a borrowed `(execution, section)` pair and one typed
//! [`Observation`] at operational boundaries. The observation is the complete
//! trace record. Fixed runtime observations carry no raw prompt prose, model
//! input or output, tool arguments or results, store paths or contents,
//! credentials, or fetched content. Reports are synchronous and never consulted
//! for a decision. [`NullObserver`] provides silence without a second execution
//! path.
//!
//! # Sensitivity of metadata
//! The variant *identity* of a fixed [`Observation`] is safe, but three inputs
//! are author-controlled and must be treated as potentially sensitive untrusted
//! metadata, not as safe fixed vocabulary:
//! - `execution` - a caller-chosen run identifier;
//! - `section` - the prompt's H2 heading text, authored in the prompt file;
//! - [`Observation::Lua`] and [`Observation::Other`] messages - a validated Lua
//!   `log(message)` checkpoint and the forward-compatible escape hatch.
//!
//! An [`Observer`] that persists or forwards reports owns treating `execution`,
//! `section`, and any message-carrying variant as untrusted: they can echo
//! prompt-authored text, so a sink must not log them into a trusted context, and
//! prompt authors must never place arguments, replies, tool data, credentials,
//! paths, or store contents in a `log(message)`.

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
///
/// # Examples
/// Match the variants a consumer cares about, use [`label`](Observation::label)
/// and [`Display`](fmt::Display), and tolerate unknown variants through a
/// wildcard arm (the enum is `#[non_exhaustive]`):
///
/// ```
/// use promptforge_core_support::observe::Observation;
///
/// fn describe(event: &Observation) -> String {
///     match event {
///         Observation::RunStarted => "run began".to_owned(),
///         // The author-controlled checkpoint owns its message.
///         Observation::Lua(message) => format!("lua says: {message}"),
///         // A forward-compatible escape hatch.
///         Observation::Other(message) => format!("other: {message}"),
///         // Any other fixed variant renders through its stable label.
///         fixed => fixed.label().unwrap_or("unknown").to_owned(),
///     }
/// }
///
/// assert_eq!(describe(&Observation::RunStarted), "run began");
/// assert_eq!(describe(&Observation::Lua("hi".to_owned())), "lua says: hi");
/// assert_eq!(describe(&Observation::Other("x".to_owned())), "other: x");
/// assert_eq!(describe(&Observation::SectionFinished), "Section finished");
///
/// // Fixed variants expose a stable label; message-carrying ones do not.
/// assert_eq!(Observation::RunStarted.label(), Some("Run started"));
/// assert_eq!(Observation::Lua("hi".to_owned()).label(), None);
/// assert_eq!(Observation::RunStarted.to_string(), "Run started");
/// ```
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
    /// A fanout arm began execution.
    ///
    /// Every arm emits exactly one [`FanoutArmStarted`](Observation::FanoutArmStarted)
    /// followed by exactly one terminal event: one of
    /// [`FanoutArmSucceeded`](Observation::FanoutArmSucceeded),
    /// [`FanoutArmExhausted`](Observation::FanoutArmExhausted),
    /// [`FanoutArmFailed`](Observation::FanoutArmFailed), or
    /// [`FanoutArmCancelled`](Observation::FanoutArmCancelled). The runtime
    /// enforces this state machine with a drop guard, so an aborted or
    /// cancelled arm still reports a terminal event.
    FanoutArmStarted,
    /// Legacy generic terminal, retained only so an older consumer's match arm
    /// stays valid. The current runtime never emits it: a finishing arm always
    /// reports one of the specific terminal variants below (succeeded /
    /// exhausted / failed / cancelled).
    FanoutArmFinished,
    /// Terminal: a fanout arm finished with a normal successful result.
    FanoutArmSucceeded,
    /// Terminal: a fanout arm soft-degraded because its tool loop was exhausted.
    FanoutArmExhausted,
    /// Terminal: a fanout arm ended with a hard error.
    FanoutArmFailed,
    /// Terminal: a fanout arm was cancelled or aborted (Ctrl-C or a sibling's
    /// hard error) before it could finalize.
    FanoutArmCancelled,
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
            Observation::FanoutArmStarted => "Fanout arm started",
            Observation::FanoutArmFinished => "Fanout arm finished",
            Observation::FanoutArmSucceeded => "Fanout arm succeeded",
            Observation::FanoutArmExhausted => "Fanout arm exhausted",
            Observation::FanoutArmFailed => "Fanout arm failed",
            Observation::FanoutArmCancelled => "Fanout arm cancelled",
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
/// These constants let emit sites name a lifecycle boundary
/// (`detail::RUN_STARTED`) without repeating the enum path; each is exactly the
/// matching [`Observation`] variant.
///
/// `#[doc(hidden)]`: a cross-crate emit-site seam for the runtime crates, not
/// host API.
#[doc(hidden)]
pub mod detail {
    use super::Observation;

    pub const PARSE_STARTED: Observation = Observation::ParseStarted;
    pub const PARSE_SUCCEEDED: Observation = Observation::ParseSucceeded;
    pub const PARSE_FAILED: Observation = Observation::ParseFailed;
    pub const RUN_STARTED: Observation = Observation::RunStarted;
    pub const RUN_SUCCEEDED: Observation = Observation::RunSucceeded;
    pub const RUN_FAILED: Observation = Observation::RunFailed;
    pub const SECTION_STARTED: Observation = Observation::SectionStarted;
    pub const SECTION_FINISHED: Observation = Observation::SectionFinished;
    pub const MODEL_TURN_COMPLETED: Observation = Observation::ModelTurnCompleted;
    pub const MODEL_TURN_FAILED: Observation = Observation::ModelTurnFailed;
    pub const MODEL_TURN_TRUNCATED: Observation = Observation::ModelTurnTruncated;
    pub const TOOL_CALL_SUCCEEDED: Observation = Observation::ToolCallSucceeded;
    pub const TOOL_CALL_FAILED: Observation = Observation::ToolCallFailed;
    pub const LUA_COMPILATION_STARTED: Observation = Observation::LuaCompilationStarted;
    pub const LUA_COMPILATION_SUCCEEDED: Observation = Observation::LuaCompilationSucceeded;
    pub const LUA_COMPILATION_FAILED: Observation = Observation::LuaCompilationFailed;
    pub const LUA_SHARED_LOAD_STARTED: Observation = Observation::LuaSharedLoadStarted;
    pub const LUA_SHARED_LOAD_SUCCEEDED: Observation = Observation::LuaSharedLoadSucceeded;
    pub const LUA_SHARED_LOAD_FAILED: Observation = Observation::LuaSharedLoadFailed;
    pub const LUA_CHUNK_STARTED: Observation = Observation::LuaChunkStarted;
    pub const LUA_CHUNK_SUCCEEDED: Observation = Observation::LuaChunkSucceeded;
    pub const LUA_CHUNK_FAILED: Observation = Observation::LuaChunkFailed;
    pub const LUA_REPLY_BINDING_STARTED: Observation = Observation::LuaReplyBindingStarted;
    pub const LUA_REPLY_BINDING_SUCCEEDED: Observation = Observation::LuaReplyBindingSucceeded;
    pub const LUA_REPLY_BINDING_FAILED: Observation = Observation::LuaReplyBindingFailed;
    pub const LUA_TEARDOWN_STARTED: Observation = Observation::LuaTeardownStarted;
    pub const LUA_TEARDOWN_SUCCEEDED: Observation = Observation::LuaTeardownSucceeded;
    pub const TOOL_SCOPE_VALIDATION_STARTED: Observation = Observation::ToolScopeValidationStarted;
    pub const TOOL_SCOPE_VALIDATION_SUCCEEDED: Observation =
        Observation::ToolScopeValidationSucceeded;
    pub const TOOL_SCOPE_VALIDATION_FAILED: Observation = Observation::ToolScopeValidationFailed;
    pub const STORE_WRITE_SUCCEEDED: Observation = Observation::StoreWriteSucceeded;
    pub const STORE_WRITE_FAILED: Observation = Observation::StoreWriteFailed;
    pub const STORE_APPEND_SUCCEEDED: Observation = Observation::StoreAppendSucceeded;
    pub const STORE_APPEND_FAILED: Observation = Observation::StoreAppendFailed;
    pub const STORE_READ_SUCCEEDED: Observation = Observation::StoreReadSucceeded;
    pub const STORE_READ_FAILED: Observation = Observation::StoreReadFailed;
    pub const STORE_READ_NUMBERED_SUCCEEDED: Observation = Observation::StoreReadNumberedSucceeded;
    pub const STORE_READ_NUMBERED_FAILED: Observation = Observation::StoreReadNumberedFailed;
    pub const STORE_REPLACE_SUCCEEDED: Observation = Observation::StoreReplaceSucceeded;
    pub const STORE_REPLACE_FAILED: Observation = Observation::StoreReplaceFailed;
    pub const STORE_DELETE_SUCCEEDED: Observation = Observation::StoreDeleteSucceeded;
    pub const STORE_DELETE_FAILED: Observation = Observation::StoreDeleteFailed;
    pub const STORE_GLOB_SUCCEEDED: Observation = Observation::StoreGlobSucceeded;
    pub const STORE_GLOB_FAILED: Observation = Observation::StoreGlobFailed;
    pub const FANOUT_ARM_STARTED: Observation = Observation::FanoutArmStarted;
    pub const FANOUT_ARM_SUCCEEDED: Observation = Observation::FanoutArmSucceeded;
    pub const FANOUT_ARM_EXHAUSTED: Observation = Observation::FanoutArmExhausted;
    pub const FANOUT_ARM_FAILED: Observation = Observation::FanoutArmFailed;
    pub const FANOUT_ARM_CANCELLED: Observation = Observation::FanoutArmCancelled;
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
/// use promptforge_core_support::observe::{Observation, Observer};
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
    ///
    /// # Examples
    /// A handler matches the typed event and treats the author-controlled
    /// [`Observation::Lua`] checkpoint as untrusted metadata (never logged
    /// verbatim or forwarded to a model-facing sink), while fixed lifecycle
    /// variants carry no payload and are safe to record. [`Observation`] is
    /// `#[non_exhaustive]`, so a wildcard arm is required:
    /// ```
    /// use promptforge_core_support::observe::{Observation, NullObserver, Observer};
    ///
    /// let observer = NullObserver::default();
    /// let event = Observation::Lua("author checkpoint text".to_owned());
    /// match event {
    ///     Observation::Lua(note) => {
    ///         // Author-controlled: keep only a payload-free signal (its length),
    ///         // never `note` verbatim.
    ///         let _sensitive_len = note.len();
    ///     }
    ///     safe => observer.observe("example-run", "Gather", safe),
    /// }
    /// ```
    fn observe(&self, execution: &str, section: &str, event: Observation);
}

/// An [`Observer`] that discards every observation.
///
/// This is what a caller wanting no progress passes, so the executor never
/// needs an `Option<&dyn Observer>` and never branches on one.
///
/// # Examples
/// ```
/// use promptforge_core_support::observe::{Observation, NullObserver, Observer};
///
/// // `#[non_exhaustive]`, so construct it through `Default` rather than the
/// // unit literal.
/// let observer = NullObserver::default();
/// observer.observe("example-run", "Example prompt", Observation::RunSucceeded);
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
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
            Observation::StoreReadNumberedSucceeded.to_string(),
            "Store read_numbered succeeded"
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
    fn unknown_and_message_variants_are_tolerated_by_a_wildcard_consumer() {
        // F7 (unknown events): a consumer that matches only the variants it
        // knows must tolerate `Other` (a forward-compatible variant it does not
        // model) through a wildcard arm, and the message-carrying variants must
        // preserve their author-controlled text verbatim.
        fn classify(event: &Observation) -> &'static str {
            match event {
                Observation::RunStarted => "known-fixed",
                Observation::Lua(_) => "lua-checkpoint",
                _ => "unknown-or-other",
            }
        }
        assert_eq!(classify(&Observation::RunStarted), "known-fixed");
        assert_eq!(
            classify(&Observation::Lua("hi".to_owned())),
            "lua-checkpoint"
        );
        // `Other` stands in for a future variant this consumer has never seen.
        assert_eq!(
            classify(&Observation::Other("future".to_owned())),
            "unknown-or-other"
        );
        assert_eq!(classify(&Observation::SectionFinished), "unknown-or-other");
        assert_eq!(
            Observation::Lua("secret note".to_owned()).to_string(),
            "Lua: secret note"
        );
        assert_eq!(
            Observation::Other("verbatim".to_owned()).to_string(),
            "verbatim"
        );
    }

    #[test]
    fn interleaved_reports_stay_correlated_by_execution_and_section() {
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
