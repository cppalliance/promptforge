//! A test-only subscriber layer that records each log's fields and level.
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

/// One recorded event: its level and its fields, each rendered as its own
/// `name=value` string rather than joined into one line.
type Event = (Level, Vec<String>);

/// The level and the individually rendered fields of every event a subscriber
/// saw, in order.
///
/// Each event keeps its fields as a list of `name=value` strings rather than
/// one flattened line, so a match is tested against one field at a time and
/// cannot span the boundary between two of them.
#[derive(Clone, Default)]
pub(crate) struct Levels(Arc<Mutex<Vec<Event>>>);

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

    /// Whether some event at `level` recorded the field `needle` exactly.
    ///
    /// Every needle is matched exactly against one recorded field, whether it
    /// is a structured `name=value` or the event's own text. Each event's
    /// message is recorded as a `message=<text>` field, so a message assertion
    /// is stated in full as `message=the whole line` and a structured one as
    /// `run_id=r1`. Exactness is the whole contract: `run_id=r1` never matches
    /// a field whose value is `r10`, never matches a value that merely embeds
    /// it, and - since each field is kept on its own rather than flattened into
    /// one line - never spans the boundary between two fields. A caller that
    /// genuinely needs a substring - proving a secret is absent from every
    /// field, say - reaches for [`mentioned`](Self::mentioned) by name rather
    /// than relaxing this, so a loose test never hides behind the exact one.
    pub(crate) fn said(&self, level: Level, needle: &str) -> bool {
        self.lines().iter().any(|(seen, fields)| {
            *seen == level && fields.iter().any(|field| field.as_str() == needle)
        })
    }

    /// Whether some event at `level` recorded a field that contains `needle`.
    ///
    /// The explicit, clearly-named substring counterpart to [`said`](Self::said),
    /// used where partial matching is the point rather than an accident: chiefly
    /// a negative assertion that a payload appears in no field at all.
    pub(crate) fn mentioned(&self, level: Level, needle: &str) -> bool {
        self.lines().iter().any(|(seen, fields)| {
            *seen == level && fields.iter().any(|field| field.contains(needle))
        })
    }

    /// Everything recorded so far.
    fn lines(&self) -> Vec<Event> {
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
        let mut fields = Fields(Vec::new());
        event.record(&mut fields);
        self.0
            .lock()
            .expect("no test panics while holding the recorded lines")
            .push((*event.metadata().level(), fields.0));
    }
}

/// A recorder installed for the calling thread, serialized against every other
/// capture in the process.
///
/// `tracing` keeps one process-global table of each callsite's interest and
/// rebuilds it whenever a scoped default is installed or dropped. Two capture
/// tests running in parallel can recompute a callsite while the rebuilding
/// thread's own default is the no-op subscriber, which caches the callsite as
/// `never` and silently drops the other test's events. That is the whole of the
/// flake behind the backgrounded-run logging test: isolated it passes, under
/// parallel load its one expected event vanishes.
///
/// [`recording`] holds a single lock for the life of each capture, so no two
/// installs or teardowns overlap and the interest cache cannot flap underneath
/// a test that is mid-run.
#[cfg(test)]
pub(crate) struct Recording {
    /// Uninstalls the recorder when this value drops.
    _default: tracing::subscriber::DefaultGuard,
    /// Released after the recorder is uninstalled, because it is the field
    /// declared last: no other capture installs until this test's teardown is
    /// complete.
    _lock: std::sync::MutexGuard<'static, ()>,
}

/// Installs a fresh [`Levels`] recorder for the calling thread and returns it
/// with the guard that keeps the capture exclusive.
///
/// The guard must outlive every event the test means to record, including those
/// a spawned task emits on a current-thread runtime. Dropping it uninstalls the
/// recorder and only then releases the process-wide lock, so captures never
/// interleave.
#[cfg(test)]
pub(crate) fn recording() -> (Levels, Recording) {
    use std::sync::{Mutex, Once, PoisonError};

    use tracing_subscriber::layer::SubscriberExt;

    // Serializes every capture in the crate. A panic mid-capture poisons the
    // lock, but the guarded unit carries no state a panic could corrupt, so it
    // is recovered rather than failing every later capture.
    static LOCK: Mutex<()> = Mutex::new(());
    // Installs the global no-op default exactly once, described below.
    static GLOBAL: Once = Once::new();

    let lock = LOCK.lock().unwrap_or_else(PoisonError::into_inner);

    // A callsite first registered while the process has no global default is
    // cached `never` and never reconsidered, so an event a later scoped recorder
    // means to see is dropped before dispatch: the whole of the flake. Installing
    // one global no-op subscriber - a bare registry that enables every callsite
    // and records nothing - keeps registration from ever caching `never`, and
    // the rebuild below recomputes anything a non-capture thread already cached
    // that way before this ran. A scoped recorder then overrides it per thread.
    GLOBAL.call_once(|| {
        let _ = tracing::subscriber::set_global_default(tracing_subscriber::registry());
        tracing::callsite::rebuild_interest_cache();
    });

    let levels = Levels::default();
    let recorder = tracing_subscriber::registry().with(levels.clone());
    let default = tracing::subscriber::set_default(recorder);
    (
        levels,
        Recording {
            _default: default,
            _lock: lock,
        },
    )
}

/// The rendered fields of one event.
struct Fields(Vec<String>);

impl Visit for Fields {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.push(format!("{}={value}", field.name()));
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0.push(format!("{}={value:?}", field.name()));
    }
}

#[cfg(test)]
mod tests {
    use tracing::Level;

    use super::recording;

    #[test]
    fn a_needle_matches_its_field_exactly_and_never_a_longer_value() {
        let (levels, _recording) = recording();
        tracing::info!(run_id = "r10", "a run outlived its call");

        // The exact field is found; a shorter value it merely embeds is not,
        // which is the precise hazard a substring test would miss.
        assert!(levels.said(Level::INFO, "run_id=r10"));
        assert!(!levels.said(Level::INFO, "run_id=r1"));
        // Message text is a `message=` field asserted in full, not a substring.
        assert!(levels.said(Level::INFO, "message=a run outlived its call"));
        assert!(!levels.said(Level::INFO, "outlived its call"));
        assert!(!levels.said(Level::WARN, "run_id=r10"));
    }
}
