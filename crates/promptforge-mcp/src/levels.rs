//! A test-only subscriber layer that records what the log said and at what
//! level.
//!
//! Two things in this crate are a level rather than a message: a run boundary is
//! `info` because an operator watching the default level must see that a run
//! happened, and everything inside a run is `debug` because a section's tool
//! calls would bury it. A level is invisible to every other kind of test, so
//! [`Levels`] is what pins one, by collecting each event and filtering to what
//! the default level would have shown.

use std::fmt;
use std::sync::{Arc, Mutex};

use tracing::field::{Field, Visit};
use tracing::{Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer};

/// The level and message of every event a subscriber saw, in order.
#[derive(Clone, Default)]
pub(crate) struct Levels(Arc<Mutex<Vec<(Level, String)>>>);

impl Levels {
    /// The levels an operator at the default level would have read, meaning
    /// `info` and above.
    pub(crate) fn operator_visible(&self) -> Vec<Level> {
        self.lines()
            .iter()
            // `Level` orders `ERROR` lowest, so this keeps info and above.
            .filter(|(level, _)| *level <= Level::INFO)
            .map(|(level, _)| *level)
            .collect()
    }

    /// Whether some event at `level` carried `needle` in its message.
    pub(crate) fn said(&self, level: Level, needle: &str) -> bool {
        self.lines()
            .iter()
            .any(|(seen, message)| *seen == level && message.contains(needle))
    }

    /// Everything recorded so far.
    fn lines(&self) -> Vec<(Level, String)> {
        self.0
            .lock()
            .expect("no test panics while holding the recorded lines")
            .clone()
    }
}

impl fmt::Debug for Levels {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Levels").field(&self.lines()).finish()
    }
}

impl<S: Subscriber> Layer<S> for Levels {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        let mut message = Message(String::new());
        event.record(&mut message);
        self.0
            .lock()
            .expect("no test panics while holding the recorded lines")
            .push((*event.metadata().level(), message.0));
    }
}

/// The `message` field of one event, which is the static text the macro was
/// written with.
struct Message(String);

impl Visit for Message {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.0 = value.to_owned();
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.0 = format!("{value:?}");
        }
    }
}
