//! Report-only observation for a run in flight.
//!
//! [`Observer`] receives borrowed `(execution, section, detail)` strings at
//! operational boundaries. The strings are the complete trace record. Fixed
//! runtime details contain no raw prompt prose, model input or output, tool
//! arguments or results, store paths or contents, credentials, or fetched
//! content. The sole author-controlled exception is a validated `Lua:
//! <message>` checkpoint from the phase-local Lua `log(message)` callback.
//! Reports are synchronous and never consulted for a decision.
//! [`NullObserver`] provides silence without a second execution path.

/// Stable detail strings emitted by the currently shipped runtime.
///
/// Consumers may recognize individual constants for cosmetic presentation, but
/// must tolerate unknown details and must never use a report to steer execution.
/// Accepted Lua checkpoints are dynamic `Lua: <message>` details and therefore
/// have no constant in this module.
pub mod detail {
    /// Prompt parsing began.
    pub const PARSE_STARTED: &str = "Parse started";
    /// Prompt parsing and parse-time compilation completed successfully.
    pub const PARSE_SUCCEEDED: &str = "Parse succeeded";
    /// Prompt parsing or parse-time compilation returned an error.
    pub const PARSE_FAILED: &str = "Parse failed";
    /// A run passed its version gate and began.
    pub const RUN_STARTED: &str = "Run started";
    /// A run returned a value.
    pub const RUN_SUCCEEDED: &str = "Run succeeded";
    /// A run returned an error.
    pub const RUN_FAILED: &str = "Run failed";
    /// A top-level section began.
    pub const SECTION_STARTED: &str = "Section started";
    /// A top-level section completed successfully.
    pub const SECTION_FINISHED: &str = "Section finished";
    /// A model round trip completed successfully.
    pub const MODEL_TURN_COMPLETED: &str = "Model turn completed";
    /// A model round trip returned an error.
    pub const MODEL_TURN_FAILED: &str = "Model turn failed";
    /// Formerly reported when a successful parse bound empty final text.
    ///
    /// Empty model product now fails the turn via [`crate::Error::EmptyModelReply`]
    /// and [`MODEL_TURN_FAILED`]; this constant is retained for host compatibility.
    pub const MODEL_REPLY_EMPTY: &str = "Model reply was empty";
    /// A successful parse ended because the model hit its length limit.
    pub const MODEL_TURN_TRUNCATED: &str = "Model turn truncated";
    /// A tool dispatch completed successfully.
    pub const TOOL_CALL_SUCCEEDED: &str = "Tool call succeeded";
    /// A tool dispatch returned an error.
    pub const TOOL_CALL_FAILED: &str = "Tool call failed";
    /// Lua source compilation began.
    pub const LUA_COMPILATION_STARTED: &str = "Lua compilation started";
    /// Lua source compilation completed successfully.
    pub const LUA_COMPILATION_SUCCEEDED: &str = "Lua compilation succeeded";
    /// Lua source compilation returned an error.
    pub const LUA_COMPILATION_FAILED: &str = "Lua compilation failed";
    /// A section VM began loading and executing its shared program.
    pub const LUA_SHARED_LOAD_STARTED: &str = "Lua shared load started";
    /// A section VM loaded and executed its shared program successfully.
    pub const LUA_SHARED_LOAD_SUCCEEDED: &str = "Lua shared load succeeded";
    /// A section VM failed to load or execute its shared program.
    pub const LUA_SHARED_LOAD_FAILED: &str = "Lua shared load failed";
    /// A section VM began executing its preamble.
    pub const LUA_PREAMBLE_STARTED: &str = "Lua preamble started";
    /// A section VM executed its preamble successfully.
    pub const LUA_PREAMBLE_SUCCEEDED: &str = "Lua preamble succeeded";
    /// A section VM failed to execute its preamble.
    pub const LUA_PREAMBLE_FAILED: &str = "Lua preamble failed";
    /// A section VM began binding a model reply.
    pub const LUA_REPLY_BINDING_STARTED: &str = "Lua reply binding started";
    /// A section VM bound a model reply successfully.
    pub const LUA_REPLY_BINDING_SUCCEEDED: &str = "Lua reply binding succeeded";
    /// A section VM failed to bind a model reply.
    pub const LUA_REPLY_BINDING_FAILED: &str = "Lua reply binding failed";
    /// A section VM began executing its epilog.
    pub const LUA_EPILOG_STARTED: &str = "Lua epilog started";
    /// A section VM executed its epilog successfully.
    pub const LUA_EPILOG_SUCCEEDED: &str = "Lua epilog succeeded";
    /// A section VM failed to execute its epilog.
    pub const LUA_EPILOG_FAILED: &str = "Lua epilog failed";
    /// A section VM began teardown.
    pub const LUA_TEARDOWN_STARTED: &str = "Lua teardown started";
    /// A section VM completed teardown.
    pub const LUA_TEARDOWN_SUCCEEDED: &str = "Lua teardown succeeded";
    /// Prompt-level tool declarations began binding.
    pub const TOOL_BINDING_STARTED: &str = "Tool binding started";
    /// Prompt-level tool declarations bound successfully.
    pub const TOOL_BINDING_SUCCEEDED: &str = "Tool binding succeeded";
    /// Prompt-level tool declaration binding failed.
    pub const TOOL_BINDING_FAILED: &str = "Tool binding failed";
    /// Live-registry and one-to-one binding validation began.
    pub const TOOL_REGISTRY_VALIDATION_STARTED: &str = "Tool registry validation started";
    /// Live-registry and one-to-one binding validation succeeded.
    pub const TOOL_REGISTRY_VALIDATION_SUCCEEDED: &str = "Tool registry validation succeeded";
    /// Live-registry or one-to-one binding validation failed.
    pub const TOOL_REGISTRY_VALIDATION_FAILED: &str = "Tool registry validation failed";
    /// A section VM began replaying prompt-level tool declarations.
    pub const TOOL_REPLAY_STARTED: &str = "Tool replay started";
    /// A section VM replayed prompt-level tool declarations exactly.
    pub const TOOL_REPLAY_SUCCEEDED: &str = "Tool replay succeeded";
    /// A section VM's prompt-level tool declaration replay differed.
    pub const TOOL_REPLAY_FAILED: &str = "Tool replay failed";
    /// A section began closing its effective tool scope.
    pub const TOOL_SCOPE_CLOSING: &str = "Tool scope closing";
    /// A section's effective tool scope was closed successfully.
    pub const TOOL_SCOPE_CLOSED: &str = "Tool scope closed";
    /// A section's effective tool scope could not be closed.
    pub const TOOL_SCOPE_FAILED: &str = "Tool scope failed";
    /// Semantic validation of a model-visible tool scope began.
    pub const TOOL_SCOPE_VALIDATION_STARTED: &str = "Tool scope validation started";
    /// A model-visible tool scope passed semantic validation.
    pub const TOOL_SCOPE_VALIDATION_SUCCEEDED: &str = "Tool scope validation succeeded";
    /// A model-visible tool scope failed semantic validation.
    pub const TOOL_SCOPE_VALIDATION_FAILED: &str = "Tool scope validation failed";
    /// Prompt-level model declarations began binding.
    pub const MODEL_BINDING_STARTED: &str = "Model binding started";
    /// Prompt-level model declarations bound successfully.
    pub const MODEL_BINDING_SUCCEEDED: &str = "Model binding succeeded";
    /// Prompt-level model declaration binding failed.
    pub const MODEL_BINDING_FAILED: &str = "Model binding failed";
    /// Live-catalog model binding validation began.
    pub const MODEL_CATALOG_VALIDATION_STARTED: &str = "Model catalog validation started";
    /// Live-catalog model binding validation succeeded.
    pub const MODEL_CATALOG_VALIDATION_SUCCEEDED: &str = "Model catalog validation succeeded";
    /// Live-catalog model binding validation failed.
    pub const MODEL_CATALOG_VALIDATION_FAILED: &str = "Model catalog validation failed";
    /// A section VM began replaying prompt-level model declarations.
    pub const MODEL_REPLAY_STARTED: &str = "Model replay started";
    /// A section VM replayed prompt-level model declarations exactly.
    pub const MODEL_REPLAY_SUCCEEDED: &str = "Model replay succeeded";
    /// A section VM's prompt-level model declaration replay differed.
    pub const MODEL_REPLAY_FAILED: &str = "Model replay failed";
    /// A section began closing its model selection.
    pub const MODEL_SCOPE_CLOSING: &str = "Model scope closing";
    /// A section's model selection was closed successfully.
    pub const MODEL_SCOPE_CLOSED: &str = "Model scope closed";
    /// A section's model selection could not be closed.
    pub const MODEL_SCOPE_FAILED: &str = "Model scope failed";
    /// A harness-mediated store write succeeded.
    pub const STORE_WRITE_SUCCEEDED: &str = "Store write succeeded";
    /// A harness-mediated store write failed.
    pub const STORE_WRITE_FAILED: &str = "Store write failed";
    /// A harness-mediated store append succeeded.
    pub const STORE_APPEND_SUCCEEDED: &str = "Store append succeeded";
    /// A harness-mediated store append failed.
    pub const STORE_APPEND_FAILED: &str = "Store append failed";
    /// A harness-mediated store read_lines succeeded.
    pub const STORE_READ_LINES_SUCCEEDED: &str = "Store read_lines succeeded";
    /// A harness-mediated store read_lines failed.
    pub const STORE_READ_LINES_FAILED: &str = "Store read_lines failed";
    /// A harness-mediated store read (verbatim) succeeded.
    pub const STORE_READ_SUCCEEDED: &str = "Store read succeeded";
    /// A harness-mediated store read (verbatim) failed.
    pub const STORE_READ_FAILED: &str = "Store read failed";
    /// A harness-mediated store inject succeeded.
    pub const STORE_INJECT_SUCCEEDED: &str = "Store inject succeeded";
    /// A harness-mediated store inject failed.
    pub const STORE_INJECT_FAILED: &str = "Store inject failed";
    /// A harness-mediated store replacement succeeded.
    pub const STORE_REPLACE_SUCCEEDED: &str = "Store replace succeeded";
    /// A harness-mediated store replacement failed.
    pub const STORE_REPLACE_FAILED: &str = "Store replace failed";
    /// A harness-mediated store deletion succeeded.
    pub const STORE_DELETE_SUCCEEDED: &str = "Store delete succeeded";
    /// A harness-mediated store deletion failed.
    pub const STORE_DELETE_FAILED: &str = "Store delete failed";
    /// A harness-mediated store glob succeeded.
    pub const STORE_GLOB_SUCCEEDED: &str = "Store glob succeeded";
    /// A harness-mediated store glob failed.
    pub const STORE_GLOB_FAILED: &str = "Store glob failed";
}

/// A report-only sink for operational observations.
///
/// The runtime calls [`observe`](Self::observe) synchronously from the task
/// driving a run, so implementations must be `Send + Sync`, non-blocking, and
/// non-panicking. A forwarding implementation should copy the borrowed strings
/// into a queue and return rather than awaiting or performing I/O. Concrete
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
/// use promptforge_core::observe::Observer;
///
/// #[derive(Default)]
/// struct Counter(AtomicUsize);
///
/// impl Observer for Counter {
///     fn observe(&self, _execution: &str, _section: &str, _detail: &str) {
///         self.0.fetch_add(1, Ordering::Relaxed);
///     }
/// }
///
/// let counter = Counter::default();
/// counter.observe("example-run", "Gather", "Section finished");
/// assert_eq!(counter.0.load(Ordering::Relaxed), 1);
/// ```
pub trait Observer: Send + Sync {
    /// Reports one deterministic statement for `execution` and `section`.
    ///
    /// Fixed runtime reports contain no payloads or secrets. The only
    /// author-controlled detail is a constrained `Lua: <message>` checkpoint;
    /// prompt authors must never put arguments, replies, tool data,
    /// credentials, paths, or store contents in it. Reports must not affect any
    /// execution decision. Implementations must return promptly and must not
    /// panic.
    fn observe(&self, execution: &str, section: &str, detail: &str);
}

/// An [`Observer`] that discards every observation.
///
/// This is what a caller wanting no progress passes, so the executor never
/// needs an `Option<&dyn Observer>` and never branches on one.
///
/// # Examples
/// ```
/// use promptforge_core::observe::{NullObserver, Observer};
///
/// let observer = NullObserver;
/// observer.observe("example-run", "Example prompt", "Run succeeded");
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullObserver;

impl Observer for NullObserver {
    fn observe(&self, _execution: &str, _section: &str, _detail: &str) {}
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Barrier, Mutex};

    use super::*;

    #[test]
    fn null_observer_accepts_reports() {
        let observer = NullObserver;
        observer.observe("example-run", "Prompt", "Run started");
        observer.observe("example-run", "Gather", "Section started");
        observer.observe("example-run", "Gather", "Section finished");
        observer.observe("example-run", "Prompt", "Run succeeded");
    }

    #[test]
    fn observer_is_dyn_compatible_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Observer>();

        let observer: &dyn Observer = &NullObserver;
        observer.observe("example-run", "Gather", "Section finished");
    }

    #[test]
    fn mutex_recorder_keeps_interleaved_execution_ids_ordered() {
        #[derive(Default)]
        struct Recorder(Mutex<Vec<(String, String, String)>>);

        impl Observer for Recorder {
            fn observe(&self, execution: &str, section: &str, detail: &str) {
                self.0
                    .lock()
                    .expect("recorder mutex must remain usable")
                    .push((execution.to_owned(), section.to_owned(), detail.to_owned()));
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
                    detail::SECTION_STARTED.to_owned(),
                ),
                (
                    "execution-b".to_owned(),
                    "Second".to_owned(),
                    detail::SECTION_STARTED.to_owned(),
                ),
                (
                    "execution-a".to_owned(),
                    "First".to_owned(),
                    detail::SECTION_FINISHED.to_owned(),
                ),
                (
                    "execution-b".to_owned(),
                    "Second".to_owned(),
                    detail::SECTION_FINISHED.to_owned(),
                ),
            ]
        );
    }
}
