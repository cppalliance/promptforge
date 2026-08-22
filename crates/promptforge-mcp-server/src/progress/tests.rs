//! Unit tests for the progress observer and its framing.

use std::fmt::Write;

use std::sync::Arc;

use super::{Frame, McpObserver, ProgressPump, Receiver};
use crate::levels::recording;
use promptforge_core::execute::{self, ResolutionContext, RunConfig};
use promptforge_core::model::ModelCatalog;
use promptforge_core::observe::{NullObserver, Observation, Observer};
use promptforge_core::parser::Prompt;
use promptforge_core::store::StoreRef;
use promptforge_core::tools::ToolCatalog;
use promptforge_tool_picker::{Catalog, Config, ToolPicker};
use tracing::Level;

/// Every frame the queue is holding.
fn drain(frames: &mut Receiver<Frame>) -> Vec<Frame> {
    let mut drained = Vec::new();
    while let Ok(frame) = frames.try_recv() {
        drained.push(frame);
    }
    drained
}

/// The frame a caption and a count would produce.
fn frame(progress: u32, message: &str) -> Frame {
    Frame {
        progress,
        message: message.to_owned(),
    }
}

/// A prompt of `sections` Lua-only sections, the last of which returns, so
/// the whole run happens offline and emits one frame per section.
fn long_prompt(sections: usize) -> String {
    let mut source = String::from(
        "---\nname: long\ndescription: Many sections\npromptforge: 1\n---\n\n# Test prompt\n",
    );
    for section in 1..sections {
        let _written = write!(
            source,
            "\n## S{section}\n\n```lua\nvar.step = {section}\n```\n"
        );
    }
    let _written = write!(
        source,
        "\n## S{sections}\n\n```lua\nreturn 'long done'\n```\n"
    );
    source
}

/// The observations a three-section run emits, one section of which takes a
/// model turn and a tool call.
fn three_section_run() -> Vec<(&'static str, Observation)> {
    vec![
        ("Trio", Observation::RunStarted),
        ("First", Observation::SectionStarted),
        ("First", Observation::ModelTurnCompleted),
        ("First", Observation::ToolCallSucceeded),
        ("First", Observation::SectionFinished),
        ("Second", Observation::SectionStarted),
        ("Second", Observation::SectionFinished),
        ("Third", Observation::SectionStarted),
        ("Third", Observation::SectionFinished),
        ("Trio", Observation::RunSucceeded),
    ]
}

#[test]
fn a_run_frames_its_start_and_each_section_and_nothing_else() {
    let (observer, mut frames) = McpObserver::queued();
    for (section, report) in three_section_run() {
        observer.observe("test-run", section, report);
    }
    assert_eq!(
        drain(&mut frames),
        vec![
            frame(0, "Trio"),
            frame(1, "First"),
            frame(2, "Second"),
            frame(3, "Third"),
        ]
    );
    assert_eq!(observer.turns(), 1, "the run's own total is what is kept");
    assert_eq!(observer.dropped(), 0);
}

#[test]
fn progress_counts_recognized_section_starts() {
    let (observer, mut frames) = McpObserver::queued();
    for section in ["one", "two", "three"] {
        observer.observe("test-run", section, Observation::SectionStarted);
    }
    let progress: Vec<u32> = drain(&mut frames).iter().map(|f| f.progress).collect();
    assert_eq!(progress, vec![1, 2, 3]);
}

#[tokio::test]
async fn a_pump_that_never_drains_still_lets_the_run_finish() {
    // `frames` is held and never read, which is a pump whose peer has
    // stopped accepting: the queue fills, and the run must not notice.
    let sections = super::CAPACITY + 16;
    let source = long_prompt(sections);
    let prompt = Prompt::parse(&source, "test-run", &NullObserver::default())
        .expect("the fixture prompt parses");
    let (observer, _frames) = McpObserver::queued();
    let observer = Arc::new(observer);

    let store = StoreRef::memory();
    let models = ModelCatalog::empty();
    let picker =
        ToolPicker::build(Catalog::default(), Config::default()).expect("empty picker must build");
    let value = execute::run(
        &prompt,
        "",
        ResolutionContext::new(&picker, &models, &ToolCatalog::default()),
        &store,
        RunConfig::new("test-run").observer(Arc::clone(&observer) as Arc<dyn Observer>),
    )
    .await
    .expect("a Lua-only run reaches no model and finishes");

    assert_eq!(value, "long done");
    assert!(
        observer.dropped() > 0,
        "a run past the queue's capacity drops frames rather than stalling"
    );
}

#[tokio::test(start_paused = true)]
async fn a_pump_the_peer_never_accepts_is_abandoned() {
    // A send that never resolves is what a peer holding its stream open
    // produces, and the reply must not wait on it.
    let pump = ProgressPump::from_task(tokio::spawn(std::future::pending()));
    let started = tokio::time::Instant::now();
    pump.finish().await;
    assert_eq!(
        started.elapsed(),
        super::pump::FLUSH_GRACE,
        "the flush waits its grace and no longer"
    );
}

#[test]
fn the_run_start_and_both_within_run_failures_reach_the_default_level() {
    let (levels, _recording) = recording();
    let observer = McpObserver::silent();
    for (section, report) in three_section_run() {
        observer.observe("test-run", section, report);
    }
    // Both within-run failures are promoted from debug to warn, so an
    // operator watching the default level sees a failed tool call and a
    // failed model turn alike, and nothing else from inside the run.
    observer.observe("test-run", "First", Observation::ToolCallFailed);
    observer.observe("test-run", "First", Observation::ModelTurnFailed);
    assert_eq!(
        levels.operator_visible(),
        vec![Level::INFO, Level::WARN, Level::WARN],
        "the run start, then the failed tool call and the failed model turn"
    );
}

#[test]
fn a_closed_queue_counts_a_disconnect_apart_from_a_full_drop() {
    // A dropped receiver is a pump that has gone away entirely, which is a
    // different loss from a full queue and is counted on its own.
    let (observer, frames) = McpObserver::queued();
    drop(frames);
    observer.observe("test-run", "First", Observation::SectionStarted);
    assert_eq!(observer.disconnected(), 1, "a closed queue is a disconnect");
    assert_eq!(
        observer.dropped(),
        0,
        "and never folded into the full-queue drop count"
    );
}

#[test]
fn a_silent_observer_counts_turns_without_a_queue() {
    let observer = McpObserver::silent();
    for (section, report) in three_section_run() {
        observer.observe("test-run", section, report);
    }
    assert_eq!(observer.turns(), 1);
    assert_eq!(observer.dropped(), 0, "there is nothing to drop into");
}

#[test]
fn unknown_details_are_tolerated_without_frames_or_counters() {
    let (observer, mut frames) = McpObserver::queued();
    for report in [
        Observation::ToolScopeValidationStarted,
        Observation::ToolScopeValidationSucceeded,
        Observation::ToolScopeValidationFailed,
        Observation::Other("A future detail".to_owned()),
    ] {
        observer.observe("test-run", "First", report);
    }

    assert!(drain(&mut frames).is_empty());
    assert_eq!(observer.turns(), 0);
    assert_eq!(observer.dropped(), 0);
}
