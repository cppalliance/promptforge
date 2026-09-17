use super::*;

use tokio::sync::broadcast;

use crate::StatusBus;
use workshop_protocol::{Progress, Severity, StatusBarUpdate};
use workshop_registry::{Registration, Registry};

/// A hub, a push handle whose status sink is a real bus, the status
/// receiver the renderer's frames land on, and the registration
/// keeping the sink alive.
fn wired() -> (
    Arc<ProgressHub>,
    Push,
    broadcast::Receiver<StatusBarUpdate>,
    Registration,
) {
    let hub = Arc::new(ProgressHub::new());
    let status = StatusBus::new();
    let rx = status.subscribe();
    let registry = Registry::new();
    let (_channel, sink, _state) = crate::register(&registry, &status);
    (hub, registry.push(), rx, sink)
}

/// Lets the renderer task run everything currently pending.
async fn settle() {
    for _ in 0..3 {
        tokio::task::yield_now().await;
    }
}

#[tokio::test(start_paused = true)]
async fn a_sub_second_operation_never_reaches_the_status_bar() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    settle().await;
    leaf.complete();
    drop(tree);
    settle().await;
    tokio::time::advance(SHOW_DELAY * 2).await;
    settle().await;
    assert!(
        rx.try_recv().is_err(),
        "a sub-second operation must never show the indicator"
    );
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_operation_outliving_the_show_delay_pushes_the_headline_and_aggregate() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let download = tree.register("download", 3.0);
    let verify = tree.register("verify", 1.0);
    download.set_fraction(1.0);
    verify.set_fraction(0.5);
    settle().await;
    tokio::time::advance(SHOW_DELAY.saturating_sub(Duration::from_millis(1))).await;
    settle().await;
    assert!(rx.try_recv().is_err(), "the bar waits out the show delay");
    tokio::time::advance(Duration::from_millis(2)).await;
    settle().await;
    let update = rx
        .try_recv()
        .expect("the bar appears once the delay lapses");
    assert_eq!(
        update.label, "verify",
        "the headline is the unfinished leaf"
    );
    assert_eq!(
        update.progress,
        Some(Progress {
            current: 875_000,
            total: PROGRESS_TOTAL,
        }),
        "the weighted aggregate: (3*1.0 + 1*0.5) / 4"
    );
    assert_eq!(update.severity, Severity::Info);
    assert_eq!(update.activity, Activity::General);
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn the_indicator_holds_for_the_minimum_visible_time_after_the_hub_drains() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    settle().await;
    tokio::time::advance(SHOW_DELAY).await;
    settle().await;
    let shown = rx
        .try_recv()
        .expect("the bar appears once the delay lapses");
    assert!(shown.progress.is_some());

    drop(tree);
    tokio::time::advance(DETACH_POLL).await;
    settle().await;
    assert!(
        rx.try_recv().is_err(),
        "the idle push waits out the minimum visible time"
    );
    tokio::time::advance(MIN_VISIBLE).await;
    settle().await;
    let idle = rx
        .try_recv()
        .expect("the bar clears once the minimum has passed");
    assert_eq!(idle.label, "Ready");
    assert_eq!(idle.progress, None);
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_lapsed_show_deadline_polls_for_the_detach_instead_of_rearming_the_past() {
    let live = Instant::now();
    let indicator = Indicator {
        live_since: Some(live),
        ..Indicator::default()
    };
    let now = live + SHOW_DELAY + Duration::from_secs(1);
    assert_eq!(
        indicator.next_wake(now),
        Some(now + DETACH_POLL),
        "a past deadline re-armed would resolve instantly and spin the loop"
    );
}

#[tokio::test(start_paused = true)]
async fn a_tree_finished_before_the_delay_and_held_past_it_never_reaches_the_status_bar() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.complete();
    settle().await;
    tokio::time::advance(SHOW_DELAY * 2).await;
    settle().await;
    assert!(
        rx.try_recv().is_err(),
        "a finished tree has no headline, so the bar never shows"
    );
    drop(tree);
    tokio::time::advance(DETACH_POLL * 2).await;
    settle().await;
    assert!(
        rx.try_recv().is_err(),
        "a bar that never showed pushes no idle frame"
    );
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_unchanged_sample_is_not_repushed_by_the_detach_poll() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    settle().await;
    tokio::time::advance(SHOW_DELAY).await;
    settle().await;
    let first = rx
        .try_recv()
        .expect("the bar appears once the delay lapses");
    assert!(first.progress.is_some());
    tokio::time::advance(DETACH_POLL * 5).await;
    settle().await;
    assert!(
        rx.try_recv().is_err(),
        "the detach poll re-samples without re-pushing an unchanged frame"
    );
    leaf.set_fraction(0.75);
    settle().await;
    let update = rx.try_recv().expect("a real change pushes");
    assert_eq!(
        update.progress,
        Some(Progress {
            current: 750_000,
            total: PROGRESS_TOTAL,
        })
    );
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_operation_attached_before_the_spawn_is_caught_by_the_first_sample() {
    let (hub, push, mut rx, _sink) = wired();
    // The tree attaches before the renderer subscribes: only the
    // initial sample catches it, since its Begun predates the
    // subscription.
    let tree = hub.operation();
    let leaf = tree.register("download", 1.0);
    leaf.set_fraction(0.5);
    let renderer = spawn(Arc::clone(&hub), push);
    settle().await;
    tokio::time::advance(SHOW_DELAY).await;
    settle().await;
    let update = rx
        .try_recv()
        .expect("the pre-attached operation still reaches the bar");
    assert_eq!(update.label, "download");
    assert_eq!(
        update.progress,
        Some(Progress {
            current: 500_000,
            total: PROGRESS_TOTAL,
        })
    );
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn a_lagged_event_receiver_resamples_from_the_snapshot() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    // Overflow the hub's 1024-event ring so the renderer's receiver
    // lags: the lag must fall through to a re-sample, not stall the
    // renderer.
    let tree = hub.operation();
    let _leaves: Vec<_> = (0..1100)
        .map(|index| tree.register(&format!("leaf-{index}"), 1.0))
        .collect();
    settle().await;
    tokio::time::advance(SHOW_DELAY).await;
    settle().await;
    let update = rx
        .try_recv()
        .expect("the bar still appears after the receiver lagged");
    assert!(
        update.progress.is_some(),
        "the re-sampled snapshot drives the bar"
    );
    renderer.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn an_operation_attaching_during_the_minimum_visible_hold_does_not_step_the_bar_backward() {
    let (hub, push, mut rx, _sink) = wired();
    let renderer = spawn(Arc::clone(&hub), push);
    let tree = hub.operation();
    let leaf = tree.register("first", 1.0);
    leaf.set_fraction(0.5);
    settle().await;
    tokio::time::advance(SHOW_DELAY).await;
    settle().await;
    let shown = rx
        .try_recv()
        .expect("the bar appears once the delay lapses");
    assert_eq!(
        shown.progress,
        Some(Progress {
            current: 500_000,
            total: PROGRESS_TOTAL,
        })
    );

    // The first operation drains while the bar is up; the minimum
    // visible hold keeps the bar showing.
    drop(tree);
    tokio::time::advance(DETACH_POLL).await;
    settle().await;

    // A new operation attaching during the hold continues from the
    // drained level: the meter reset on the idle sample, so without
    // the floor the visible bar would restart at zero.
    let second = hub.operation();
    let _leaf = second.register("second", 1.0);
    settle().await;
    let continued = rx
        .try_recv()
        .expect("the new operation pushes under the held bar");
    assert_eq!(continued.label, "second");
    assert_eq!(
        continued.progress,
        Some(Progress {
            current: 500_000,
            total: PROGRESS_TOTAL,
        }),
        "the bar never steps backward"
    );
    renderer.shutdown().await;
}
