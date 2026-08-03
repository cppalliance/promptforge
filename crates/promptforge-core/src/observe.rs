//! Progress reporting for a run in flight.
//!
//! A run is otherwise silent until it returns, which for a multi-section prompt
//! is minutes. [`Observer`] is the seam a caller hooks to watch one: the
//! executor hands it an [`Event`] at each point a reader would want to see,
//! and the caller forwards, records, or discards it. [`NullObserver`] is the
//! discarding implementation, and is what a caller that wants no progress
//! passes.
//!
//! The events are the contract, not the executor's internals: they name what
//! happened in the prompt's own vocabulary (sections, turns, tools), so a
//! consumer renders them without knowing how a section is run.
//!
//! Each [`Event`] serializes externally tagged, meaning one JSON object whose
//! single key is the variant name and whose value holds the fields:
//!
//! ```json
//! {"SectionStarted": {"completed": 1, "name": "Gather"}}
//! ```
//!
//! [`crate::execute::run`] emits through this seam: it takes a
//! [`crate::execute::RunOptions`] carrying the observer, and reports the run's
//! start and end, each section boundary, each model turn, and each tool call.

/// A sink for [`Event`]s produced by a run.
///
/// The executor calls [`on_event`] from whatever task is driving the run,
/// so an implementation must be `Send + Sync`. It must also be cheap:
/// `on_event` is synchronous and sits on the run's own path, so an
/// implementation that forwards elsewhere hands the event to a channel and
/// returns rather than blocking, awaiting, or performing I/O.
///
/// An event is a report, never a decision. Dropping one, or all of them, must
/// leave the run's result unchanged - which is what lets [`NullObserver`] be
/// the default.
///
/// [`on_event`]: Observer::on_event
///
/// # Examples
/// ```
/// use std::sync::atomic::{AtomicUsize, Ordering};
///
/// use promptforge_core::observe::{Event, Observer};
///
/// #[derive(Default)]
/// struct Counter(AtomicUsize);
///
/// impl Observer for Counter {
///     fn on_event(&self, _ev: &Event) {
///         self.0.fetch_add(1, Ordering::Relaxed);
///     }
/// }
///
/// let counter = Counter::default();
/// counter.on_event(&Event::SectionFinished { name: "Gather".to_string() });
/// assert_eq!(counter.0.load(Ordering::Relaxed), 1);
/// ```
pub trait Observer: Send + Sync {
    /// Reports that `ev` just happened.
    ///
    /// Called synchronously on the run's own path. An implementation must not
    /// block, await, or panic; see the trait documentation.
    fn on_event(&self, ev: &Event);
}

/// Something worth reporting that happened during a run.
///
/// Marked `#[non_exhaustive]` because the executor will grow points worth
/// reporting; a consumer therefore matches with a catch-all arm.
///
/// # Examples
/// ```
/// use promptforge_core::observe::Event;
///
/// let ev = Event::RunStarted { prompt: "greet".to_string(), sections: 2 };
/// assert_eq!(
///     serde_json::to_value(&ev)?,
///     serde_json::json!({"RunStarted": {"prompt": "greet", "sections": 2}}),
/// );
/// # Ok::<(), serde_json::Error>(())
/// ```
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[non_exhaustive]
pub enum Event {
    /// The run began.
    RunStarted {
        /// The prompt's frontmatter name.
        prompt: String,
        /// How many top-level sections the prompt declares.
        ///
        /// This is what the prompt contains, not what the run will visit: an
        /// early return or a jump means fewer, so it bounds rather than
        /// predicts.
        sections: usize,
    },

    /// A section began.
    SectionStarted {
        /// How many sections have been entered, including this one, so it is
        /// 1 for the first. Never decreases across a run; a repeated section
        /// repeats a value rather than going backwards.
        completed: u32,
        /// The section's heading text.
        name: String,
    },

    /// A section finished.
    SectionFinished {
        /// The section's heading text.
        name: String,
    },

    /// The model produced a reply within a section.
    ModelTurn {
        /// The heading text of the section the turn belongs to.
        section: String,
        /// Which turn this is within the section, counting from 1.
        turn: u32,
    },

    /// A tool the model asked for has been dispatched and has answered.
    ToolCalled {
        /// The heading text of the section the call belongs to.
        section: String,
        /// The tool's wire name.
        tool: String,
        /// Whether the call answered rather than failing.
        ok: bool,
    },

    /// The run ended, whether by returning a value or by failing.
    RunFinished {
        /// How many model turns the whole run took.
        turns: u32,
        /// Wall-clock duration of the run in milliseconds.
        elapsed_ms: u64,
        /// Whether the run produced a value rather than an error.
        ok: bool,
    },
}

/// An [`Observer`] that discards every event.
///
/// This is what a caller wanting no progress passes, so the executor never
/// needs an `Option<&dyn Observer>` and never branches on one.
///
/// # Examples
/// ```
/// use promptforge_core::observe::{Event, NullObserver, Observer};
///
/// let observer = NullObserver;
/// observer.on_event(&Event::RunFinished { turns: 3, elapsed_ms: 120, ok: true });
/// ```
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NullObserver;

impl Observer for NullObserver {
    fn on_event(&self, _ev: &Event) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How many variants [`Event`] has; [`variant_index`] is what keeps this
    /// honest.
    const VARIANT_COUNT: usize = 6;

    /// Returns a distinct index for each [`Event`] variant.
    ///
    /// The match has one arm per variant and no catch-all. `#[non_exhaustive]`
    /// only binds other crates, so this match is still exhaustiveness-checked
    /// here: a variant added to [`Event`] stops this file compiling until it
    /// is given an index, and `every_variant_is_represented` then fails until
    /// [`every_variant`] builds one.
    fn variant_index(ev: &Event) -> usize {
        match ev {
            Event::RunStarted { .. } => 0,
            Event::SectionStarted { .. } => 1,
            Event::SectionFinished { .. } => 2,
            Event::ModelTurn { .. } => 3,
            Event::ToolCalled { .. } => 4,
            Event::RunFinished { .. } => 5,
        }
    }

    /// One of each variant, so a test that must cover the enum cannot quietly
    /// miss one added later.
    fn every_variant() -> Vec<Event> {
        vec![
            Event::RunStarted {
                prompt: "greet".to_string(),
                sections: 2,
            },
            Event::SectionStarted {
                completed: 1,
                name: "Gather".to_string(),
            },
            Event::SectionFinished {
                name: "Gather".to_string(),
            },
            Event::ModelTurn {
                section: "Gather".to_string(),
                turn: 2,
            },
            Event::ToolCalled {
                section: "Gather".to_string(),
                tool: "web_search".to_string(),
                ok: true,
            },
            Event::RunFinished {
                turns: 3,
                elapsed_ms: 1250,
                ok: false,
            },
        ]
    }

    #[test]
    fn every_variant_is_represented() {
        let mut seen = [false; VARIANT_COUNT];
        for ev in every_variant() {
            seen[variant_index(&ev)] = true;
        }
        assert!(
            seen.iter().all(|found| *found),
            "every_variant omits a variant: {seen:?}"
        );
    }

    #[test]
    fn variants_serialize_to_expected_shape() {
        let expected = serde_json::json!([
            {"RunStarted": {"prompt": "greet", "sections": 2}},
            {"SectionStarted": {"completed": 1, "name": "Gather"}},
            {"SectionFinished": {"name": "Gather"}},
            {"ModelTurn": {"section": "Gather", "turn": 2}},
            {"ToolCalled": {"section": "Gather", "tool": "web_search", "ok": true}},
            {"RunFinished": {"turns": 3, "elapsed_ms": 1250, "ok": false}},
        ]);
        assert_eq!(
            serde_json::to_value(every_variant()).expect("serialize"),
            expected
        );
    }

    #[test]
    fn null_observer_accepts_every_variant() {
        let observer = NullObserver;
        for ev in every_variant() {
            observer.on_event(&ev);
        }
    }

    #[test]
    fn observer_is_dyn_compatible_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn Observer>();

        let observer: &dyn Observer = &NullObserver;
        observer.on_event(&Event::SectionFinished {
            name: "Gather".to_string(),
        });
    }

    #[test]
    fn events_are_cloneable_for_forwarding() {
        // A forwarding observer owns what it queues, so `Event` must clone.
        let ev = Event::SectionStarted {
            completed: 4,
            name: "Report".to_string(),
        };
        let queued = ev.clone();
        assert_eq!(ev, queued);
    }
}
