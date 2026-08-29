//! Report-only observation for a run in flight.
//!
//! [`Observer`] receives a borrowed `(execution, section)` pair and one typed
//! [`Observation`] at operational boundaries. Reports are synchronous and
//! never consulted for a decision. [`NullObserver`] provides silence without
//! a second execution path.
//!
//! The implementation lives in the `promptforge-core-support` crate and is
//! re-exported here unchanged, so existing `promptforge_core::observe::*`
//! paths keep working.

pub use promptforge_core_support::observe::{NullObserver, Observation, Observer};

pub(crate) use promptforge_core_support::observe::detail;

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A recorder that keeps every correlated `(execution, section, event)`
    /// record.
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

    impl Recorder {
        fn records(&self) -> Vec<(String, String, Observation)> {
            self.0
                .lock()
                .expect("recorder mutex must remain usable")
                .clone()
        }
    }

    #[test]
    fn parse_failure_pairs_started_with_failed_and_carries_author_labels() {
        // F7 (failure lifecycle pairing + sensitive labels), cross-module
        // through the parser: a failed parse emits `ParseStarted` first and
        // `ParseFailed` last, and the caller-chosen `execution` id (untrusted,
        // author-controlled metadata) is carried verbatim to the observer.
        use crate::parser::Prompt;

        let recorder = Recorder::default();
        let execution = "author/controlled:run id";
        let _ = Prompt::parse("no frontmatter here", execution, &recorder)
            .expect_err("a source without frontmatter must fail to parse");

        let records = recorder.records();
        assert_eq!(
            records.first().map(|(_, _, event)| event),
            Some(&Observation::ParseStarted),
            "the lifecycle must open with ParseStarted: {records:?}"
        );
        assert_eq!(
            records.last().map(|(_, _, event)| event),
            Some(&Observation::ParseFailed),
            "a failed parse must close with ParseFailed: {records:?}"
        );
        assert!(
            records
                .iter()
                .all(|(seen_execution, _, _)| seen_execution == execution),
            "the author-controlled execution id must be carried verbatim: {records:?}"
        );

        // The success lifecycle pairs Started with Succeeded instead.
        let recorder = Recorder::default();
        let source =
            "---\nname: greeter\ndescription: d\npromptforge: 1\n---\n\n# T\n\n## S\n\nhi\n";
        Prompt::parse(source, execution, &recorder).expect("a well-formed source must parse");
        let events: Vec<Observation> = recorder
            .records()
            .into_iter()
            .map(|(_, _, event)| event)
            .collect();
        assert_eq!(events.first(), Some(&Observation::ParseStarted));
        assert_eq!(events.last(), Some(&Observation::ParseSucceeded));
    }
}
