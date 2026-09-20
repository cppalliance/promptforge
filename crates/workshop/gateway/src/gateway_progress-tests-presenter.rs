//! The anti-flicker presenter pinned with explicit instants: a busy
//! snapshot shows only after the show delay, an idle snapshot before the
//! delay pushes nothing, an idle arriving within the minimum visible time
//! is deferred to it, text changes republish while shown, and a detach
//! rests a shown bar under the same hold.

use super::*;

use gateway_api_types::Progress;

/// A presenter under the production policy, its push, and the recorder.
fn wired() -> (Presenter, Recorder) {
    (Presenter::new(Policy::DEFAULT), recorder())
}

fn busy(text: &str) -> Progress {
    Progress {
        busy: true,
        text: text.to_owned(),
    }
}

fn idle() -> Progress {
    Progress::default()
}

#[tokio::test]
async fn a_busy_snapshot_pushes_only_once_the_show_delay_has_passed() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    assert!(
        recorder.pushed().is_empty(),
        "a fresh busy snapshot must not flash the bar"
    );
    assert_eq!(
        presenter.next_wake(),
        Some(start + SHOW_DELAY),
        "the presenter asks to wake exactly at the show deadline"
    );

    presenter.tick(
        start + SHOW_DELAY - Duration::from_millis(1),
        &recorder.push,
    );
    assert!(
        recorder.pushed().is_empty(),
        "one millisecond short of the delay is still too soon"
    );

    presenter.tick(start + SHOW_DELAY, &recorder.push);
    assert_eq!(
        recorder.pushed(),
        [("Downloading model".to_owned(), true)],
        "the delay lapsing pushes one busy frame carrying the gateway text"
    );
    assert_eq!(
        presenter.next_wake(),
        None,
        "a shown bar with live work has no deadline of its own"
    );
}

#[tokio::test]
async fn an_idle_snapshot_before_the_show_delay_pushes_nothing() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Loading profile"), start, &recorder.push);
    presenter.apply(idle(), start + Duration::from_millis(300), &recorder.push);
    presenter.tick(start + SHOW_DELAY * 2, &recorder.push);
    assert!(
        recorder.pushed().is_empty(),
        "sub-second work never disturbs the status bar"
    );
    assert_eq!(presenter.next_wake(), None);
}

#[tokio::test]
async fn an_idle_snapshot_within_the_minimum_visible_time_is_deferred_to_it() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    assert_eq!(recorder.pushed().len(), 1, "the bar is up");

    let ended = shown + Duration::from_millis(100);
    presenter.apply(idle(), ended, &recorder.push);
    assert_eq!(
        recorder.pushed().len(),
        1,
        "an idle arriving 100ms after showing must not flash the bar off"
    );
    assert_eq!(
        presenter.next_wake(),
        Some(shown + MIN_VISIBLE),
        "the presenter asks to wake when the minimum visible time lapses"
    );

    presenter.tick(
        shown + MIN_VISIBLE - Duration::from_millis(1),
        &recorder.push,
    );
    assert_eq!(recorder.pushed().len(), 1, "still inside the hold");

    presenter.tick(shown + MIN_VISIBLE, &recorder.push);
    assert_eq!(
        recorder.pushed(),
        [
            ("Downloading model".to_owned(), true),
            ("Ready".to_owned(), false),
        ],
        "the hold lapsing rests the bar"
    );
    assert_eq!(presenter.next_wake(), None);
}

#[tokio::test]
async fn an_idle_snapshot_after_the_minimum_visible_time_rests_the_bar_at_once() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    presenter.apply(idle(), shown + MIN_VISIBLE * 4, &recorder.push);
    assert_eq!(
        recorder.pushed().last(),
        Some(&("Ready".to_owned(), false)),
        "an idle past the hold needs no deferral"
    );
}

#[tokio::test]
async fn a_text_change_while_shown_republishes_and_an_unchanged_text_does_not() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model 10%"), start, &recorder.push);
    presenter.apply(
        busy("Downloading model 20%"),
        start + Duration::from_millis(500),
        &recorder.push,
    );
    assert!(
        recorder.pushed().is_empty(),
        "text changes before the delay stay pending"
    );
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    assert_eq!(
        recorder.pushed(),
        [("Downloading model 20%".to_owned(), true)],
        "the delay lapsing pushes the newest text, not the first"
    );

    presenter.apply(
        busy("Downloading model 20%"),
        shown + Duration::from_millis(10),
        &recorder.push,
    );
    assert_eq!(
        recorder.pushed().len(),
        1,
        "a snapshot repeating the shown text is not re-pushed"
    );

    presenter.apply(
        busy("Downloading model 30%"),
        shown + Duration::from_millis(20),
        &recorder.push,
    );
    assert_eq!(
        recorder.pushed().last(),
        Some(&("Downloading model 30%".to_owned(), true)),
        "a new text republishes while the bar is up"
    );
}

#[tokio::test]
async fn work_resuming_during_the_hold_keeps_the_bar_up_without_a_new_delay() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Selecting profile"), start, &recorder.push);
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    presenter.apply(idle(), shown + Duration::from_millis(100), &recorder.push);
    presenter.apply(
        busy("Starting models"),
        shown + Duration::from_millis(200),
        &recorder.push,
    );
    assert_eq!(
        recorder.pushed(),
        [
            ("Selecting profile".to_owned(), true),
            ("Starting models".to_owned(), true),
        ],
        "back-to-back work shares one visible run: no idle frame, no second delay"
    );
    assert_eq!(presenter.next_wake(), None);
}

#[tokio::test]
async fn a_detach_within_the_minimum_visible_time_is_deferred_to_it() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    presenter.detach(shown + Duration::from_millis(100), &recorder.push);
    assert_eq!(
        recorder.pushed().len(),
        1,
        "a subscription lost 100ms after showing must not flash the bar off"
    );
    assert_eq!(
        presenter.next_wake(),
        Some(shown + MIN_VISIBLE),
        "the presenter asks to wake when the minimum visible time lapses"
    );

    presenter.tick(shown + MIN_VISIBLE, &recorder.push);
    assert_eq!(
        recorder.pushed(),
        [
            ("Downloading model".to_owned(), true),
            ("Ready".to_owned(), false),
        ],
        "the hold lapsing rests the bar"
    );
    assert_eq!(presenter.next_wake(), None);
}

#[tokio::test]
async fn a_detach_after_the_minimum_visible_time_rests_the_bar_at_once() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    let shown = start + SHOW_DELAY;
    presenter.tick(shown, &recorder.push);
    presenter.detach(shown + MIN_VISIBLE, &recorder.push);
    assert_eq!(
        recorder.pushed().last(),
        Some(&("Ready".to_owned(), false)),
        "a lost subscription past the hold rests the bar at once"
    );
    assert_eq!(presenter.next_wake(), None);
}

#[tokio::test]
async fn a_detach_before_the_bar_showed_forgets_the_pending_work() {
    let (mut presenter, recorder) = wired();
    let start = Instant::now();
    presenter.apply(busy("Downloading model"), start, &recorder.push);
    presenter.detach(start + Duration::from_millis(300), &recorder.push);
    assert_eq!(presenter.next_wake(), None);
    presenter.tick(start + SHOW_DELAY * 2, &recorder.push);
    assert!(
        recorder.pushed().is_empty(),
        "a detach before the bar showed pushes nothing, then or later"
    );
}
