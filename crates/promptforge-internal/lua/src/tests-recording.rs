//! Test-only read-back of the events this crate's seams emit, in the shape
//! the suites assert on: `(execution, section, Observation)` records.
//!
//! The engine reports as [`Event`] values through an [`Emitter`]; a suite
//! that wants to assert on the boundaries a VM crossed drains the emitter's
//! sink and folds each event to its kind. [`Recorder`] is that fold with
//! the emitter beside it, and [`null_emitter`] is an emitter whose sink is
//! never read.

use std::sync::Mutex;

use promptforge_api_types::emitter::{DebugMode, Emitter, EventSink};
use promptforge_api_types::event::Event;

/// One event folded to what a suite compares: a payload-free boundary by
/// its serialized `kind`, the author's `log` checkpoint with its message,
/// or anything else by its kind.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Observation {
    /// A payload-free lifecycle boundary, named by its `kind` label.
    Lifecycle(&'static str),
    /// The author's `log(message)` checkpoint.
    Lua(String),
    /// Any other event, by its `kind` label.
    Other(String),
}

impl std::fmt::Display for Observation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Observation::Lifecycle(kind) => f.write_str(kind),
            Observation::Lua(message) => write!(f, "Lua: {message}"),
            Observation::Other(kind) => f.write_str(kind),
        }
    }
}

/// Declares the boundaries the suites name, each as its `kind` label.
macro_rules! lifecycle_kinds {
    ($($name:ident => $kind:literal),* $(,)?) => {
        /// The payload-free boundaries the suites assert on.
        pub(crate) mod detail {
            use super::Observation;
            $(pub(crate) const $name: Observation = Observation::Lifecycle($kind);)*
        }

        const KINDS: &[&str] = &[$($kind,)*];
    };
}

lifecycle_kinds! {
    TOOL_CALL_SUCCEEDED => "tool_call_succeeded",
    TOOL_CALL_FAILED => "tool_call_failed",
    LUA_COMPILATION_STARTED => "lua_compilation_started",
    LUA_COMPILATION_SUCCEEDED => "lua_compilation_succeeded",
    LUA_COMPILATION_FAILED => "lua_compilation_failed",
    LUA_SHARED_LOAD_STARTED => "lua_shared_load_started",
    LUA_SHARED_LOAD_SUCCEEDED => "lua_shared_load_succeeded",
    LUA_SHARED_LOAD_FAILED => "lua_shared_load_failed",
    LUA_CHUNK_STARTED => "lua_chunk_started",
    LUA_CHUNK_SUCCEEDED => "lua_chunk_succeeded",
    LUA_CHUNK_FAILED => "lua_chunk_failed",
    LUA_TEARDOWN_STARTED => "lua_teardown_started",
    LUA_TEARDOWN_SUCCEEDED => "lua_teardown_succeeded",
    STORE_WRITE_SUCCEEDED => "store_write_succeeded",
    STORE_WRITE_FAILED => "store_write_failed",
    STORE_APPEND_SUCCEEDED => "store_append_succeeded",
    STORE_APPEND_FAILED => "store_append_failed",
    STORE_READ_SUCCEEDED => "store_read_succeeded",
    STORE_READ_FAILED => "store_read_failed",
    STORE_READ_NUMBERED_SUCCEEDED => "store_read_numbered_succeeded",
    STORE_READ_NUMBERED_FAILED => "store_read_numbered_failed",
    STORE_REPLACE_SUCCEEDED => "store_replace_succeeded",
    STORE_REPLACE_FAILED => "store_replace_failed",
    STORE_DELETE_SUCCEEDED => "store_delete_succeeded",
    STORE_DELETE_FAILED => "store_delete_failed",
    STORE_GLOB_SUCCEEDED => "store_glob_succeeded",
    STORE_GLOB_FAILED => "store_glob_failed",
}

/// The `kind` label an event serializes under.
fn kind(event: &Event) -> String {
    serde_json::to_value(event)
        .ok()
        .and_then(|value| value.get("kind")?.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// Folds one event to the record a suite compares.
pub(crate) fn observation(event: &Event) -> Observation {
    if let Event::Lua { message, .. } = event {
        return Observation::Lua(message.clone());
    }
    let kind = kind(event);
    KINDS
        .iter()
        .find(|known| **known == kind)
        .map_or(Observation::Other(kind), |known| {
            Observation::Lifecycle(known)
        })
}

/// One recorded content report: the tool result's turn, call id, alias,
/// content, and trusted flag, field for field.
pub(crate) type ToolResultRecord = (u32, String, String, String, bool);

/// An emitter over a private sink, with the events it produced read back
/// as records: the suites' recording observer.
#[derive(Debug)]
pub(crate) struct Recorder {
    sink: EventSink,
    emitter: Emitter,
    /// Every event drained so far, so `records` is cumulative across calls.
    seen: Mutex<Vec<Event>>,
}

impl Default for Recorder {
    fn default() -> Self {
        Self::for_execution("lua-test")
    }
}

impl Recorder {
    /// A recorder whose emitter reports under `execution`.
    pub(crate) fn for_execution(execution: &str) -> Self {
        let sink = EventSink::default();
        let emitter = Emitter::root(sink.clone(), execution, DebugMode::Off);
        Self {
            sink,
            emitter,
            seen: Mutex::new(Vec::new()),
        }
    }

    /// The emitter the seams under test report through.
    pub(crate) fn emitter(&self) -> &Emitter {
        &self.emitter
    }

    /// A second emitter over the same sink reporting under another
    /// execution id, for a test that interleaves runs.
    pub(crate) fn emitter_for(&self, execution: &str) -> Emitter {
        Emitter::root(self.sink.clone(), execution, DebugMode::Off)
    }

    /// Every event reported so far, in order.
    pub(crate) fn events(&self) -> Vec<Event> {
        let mut seen = self
            .seen
            .lock()
            .expect("the recorder mutex must not be poisoned");
        seen.extend(self.sink.take());
        seen.clone()
    }

    /// Every report so far as `(execution, section, observation)`.
    pub(crate) fn records(&self) -> Vec<(String, String, Observation)> {
        self.events()
            .iter()
            .map(|event| {
                (
                    event.execution().to_owned(),
                    event.section().to_owned(),
                    observation(event),
                )
            })
            .collect()
    }

    /// Every report so far as `(section, observation)`.
    pub(crate) fn observations(&self) -> Vec<(String, Observation)> {
        self.events()
            .iter()
            .map(|event| (event.section().to_owned(), observation(event)))
            .collect()
    }

    /// The payload-free boundaries and checkpoints alone, in order.
    pub(crate) fn kinds(&self) -> Vec<Observation> {
        self.events()
            .iter()
            .filter(|event| !matches!(event, Event::ToolResult { .. }))
            .map(observation)
            .collect()
    }

    /// The `ToolResult` content reports alone, in order.
    pub(crate) fn tool_results(&self) -> Vec<ToolResultRecord> {
        self.events()
            .iter()
            .filter_map(|event| match event {
                Event::ToolResult {
                    turn,
                    tool_call_id,
                    alias,
                    content,
                    trusted,
                    ..
                } => Some((
                    *turn,
                    tool_call_id.clone(),
                    alias.clone(),
                    content.clone(),
                    *trusted,
                )),
                _ => None,
            })
            .collect()
    }
}

/// An emitter whose events nobody reads: the silent stand-in a test passes
/// where it has nothing to assert about the boundaries.
pub(crate) fn null_emitter() -> Emitter {
    Emitter::root(EventSink::default(), "lua-test", DebugMode::Off)
}
