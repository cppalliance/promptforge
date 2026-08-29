use std::sync::Arc;

use tokio::sync::mpsc;

use super::arm::ArmFinalizer;
use super::*;
use crate::observe::{Observation, Observer, detail};

#[test]
fn resolve_sibling_finds_exact_match() {
    let sections = vec![sibling("Worker", 3), sibling("Topics", 3)];
    let found = resolve_sibling("### Worker", &sections).expect("must resolve");
    assert_eq!(found.name(), "Worker");
}

#[test]
fn resolve_sibling_missing_heading_lists_available() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### Missing", &sections).expect_err("missing heading must error");
    assert!(err.to_string().contains("### Worker"), "error was: {err}");
}

#[test]
fn resolve_sibling_bare_name_errors() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("Worker", &sections).expect_err("bare name without ### must error");
    assert!(err.to_string().contains("### markers"), "error was: {err}");
}

fn sibling(name: &str, level: u8) -> Section {
    crate::test_support::synthetic_section(
        name,
        level,
        vec![promptforge_parser::test_support::prose_block(
            String::new(),
            true,
        )],
        Vec::new(),
    )
}

#[test]
fn resolve_sibling_requires_whitespace_after_markers() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("###Worker", &sections)
        .expect_err("no whitespace after markers must error");
    assert!(err.to_string().contains("whitespace"), "error was: {err}");
}

#[test]
fn resolve_sibling_marker_only_heading_errors_as_nameless() {
    let sections = vec![sibling("Worker", 3)];
    let err = resolve_sibling("### ", &sections).expect_err("a marker-only heading must error");
    assert!(err.to_string().contains("has no name"), "error was: {err}");
}

#[test]
fn resolve_sibling_requires_exact_level() {
    let sections = vec![sibling("Worker", 3)];
    // Same name, wrong marker level, must not resolve.
    let err = resolve_sibling("## Worker", &sections)
        .expect_err("a level mismatch must not resolve by name alone");
    assert!(err.to_string().contains("not found"), "error was: {err}");
    // The exact address resolves.
    let ok = resolve_sibling("### Worker", &sections).expect("exact address resolves");
    assert_eq!(ok.name(), "Worker");
}

#[test]
fn resolve_sibling_rejects_more_than_one_match() {
    let sections = vec![sibling("Worker", 3), sibling("Worker", 3)];
    let err = resolve_sibling("### Worker", &sections)
        .expect_err("two identical siblings must be rejected as ambiguous");
    assert!(err.to_string().contains("ambiguous"), "error was: {err}");
}

/// Forwards each observation over the channel, so the finalizer test asserts
/// on arrival order.
struct ChannelObserver {
    tx: mpsc::Sender<(String, Observation)>,
}

impl Observer for ChannelObserver {
    fn observe(&self, _execution: &str, section: &str, event: Observation) {
        let _ = self.tx.try_send((section.to_owned(), event));
    }
}

#[test]
fn arm_finalizer_emits_cancelled_on_drop_unless_finished() {
    // FANOUT-004/006: the guard emits exactly one terminal event. Dropped
    // without finishing => cancelled; finished => only that event.
    let (tx, mut rx) = mpsc::channel::<(String, Observation)>(8);
    let observer: Arc<dyn Observer> = Arc::new(ChannelObserver { tx });

    drop(ArmFinalizer::new(
        Arc::clone(&observer),
        "exec".to_string(),
        "S".to_string(),
    ));
    let (_, event) = rx.try_recv().expect("a dropped finalizer emits an event");
    assert_eq!(event, detail::FANOUT_ARM_CANCELLED);
    assert!(rx.try_recv().is_err(), "exactly one terminal event on drop");

    let mut finalizer =
        ArmFinalizer::new(Arc::clone(&observer), "exec".to_string(), "S".to_string());
    finalizer.finish(detail::FANOUT_ARM_SUCCEEDED);
    drop(finalizer);
    let (_, event) = rx.try_recv().expect("finish emits its event");
    assert_eq!(event, detail::FANOUT_ARM_SUCCEEDED);
    assert!(
        rx.try_recv().is_err(),
        "a finished finalizer does not also emit cancelled on drop"
    );
}
