//! The hub-to-status-bar renderer: a task that samples the process
//! [`ProgressHub`] and drives the status bar's progress indicator through
//! [`Push`], so the anti-flicker policy lives here and the UI stays dumb.
//!
//! The indicator appears only once an operation has been live for
//! [`SHOW_DELAY`], stays up at least [`MIN_VISIBLE`] once shown, and
//! displays the monotonic aggregate from [`ProgressMeter`]: the bar never
//! flashes for sub-second work, never resets mid-operation, and never
//! steps backward. Status texts ("Voice ready", failures) stay explicit
//! push calls in the subsystems that own them; the trees own only
//! fractional progress. When the hub's last tree detaches, the renderer
//! returns the bar to rest with [`Push::push_idle`].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use tokio::time::Instant;

use promptforge_progress::{ProgressHub, ProgressMeter};

use crate::protocol::Activity;
use crate::push::Push;

/// How long an operation must be live before the indicator appears; work
/// shorter than this never disturbs the status bar.
pub(crate) const SHOW_DELAY: Duration = Duration::from_secs(1);

/// How long the indicator stays up once shown, so an operation that ends
/// just past [`SHOW_DELAY`] still reads as a completed bar, not a flash.
pub(crate) const MIN_VISIBLE: Duration = Duration::from_millis(500);

/// How often the renderer re-samples while the indicator is up: a tree's
/// detach emits no event, so only a poll notices the last tree leaving.
const DETACH_POLL: Duration = Duration::from_millis(100);

/// The `total` every pushed frame carries; fractions quantize to
/// millionths of it.
const PROGRESS_TOTAL: u64 = 1_000_000;

/// A running renderer task.
///
/// [`Renderer::shutdown`] signals the task to stop and awaits it. Dropping
/// the handle without shutting down still stops the task at its next
/// select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub(crate) struct Renderer {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Renderer {
    /// Signals the renderer to stop and waits for its task to finish.
    pub(crate) async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the renderer task against `hub`, pushing through `push`.
#[must_use]
pub(crate) fn spawn(hub: Arc<ProgressHub>, push: Push) -> Renderer {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&hub, &push, &mut stopped).await;
    });
    Renderer {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The task loop: re-sample on every hub event and on every anti-flicker
/// deadline. A lagged receiver simply re-samples - snapshots are the
/// ground truth and intermediate events are lossy by design.
async fn run(hub: &ProgressHub, push: &Push, stop: &mut oneshot::Receiver<()>) {
    let mut events = hub.subscribe();
    let mut indicator = Indicator::default();
    // Catches operations that attached before the subscription.
    indicator.update(hub, push);
    loop {
        tokio::select! {
            _ = &mut *stop => break,
            event = events.recv() => {
                // The hub lives in AppState for the process lifetime, so
                // Closed cannot occur in production; treat it as a stop.
                if matches!(event, Err(RecvError::Closed)) {
                    break;
                }
            }
            () = wake_at(indicator.next_wake(Instant::now())) => {}
        }
        indicator.update(hub, push);
    }
}

/// Waits for `at`, or forever when there is no pending deadline.
async fn wake_at(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// The anti-flicker state machine over the hub's snapshots.
#[derive(Debug, Default)]
struct Indicator {
    meter: ProgressMeter,
    /// When the current run of live operations began; back-to-back
    /// operations share one run, so the bar never flickers between them.
    live_since: Option<Instant>,
    /// When the indicator was shown; held until the idle push lands.
    shown_since: Option<Instant>,
    /// The last frame pushed, so the detach poll never re-pushes an
    /// unchanged sample.
    last_frame: Option<(String, u64)>,
}

impl Indicator {
    /// Samples the hub and pushes whatever transition the sample calls for.
    fn update(&mut self, hub: &ProgressHub, push: &Push) {
        let now = Instant::now();
        let Some(fraction) = self.meter.sample(hub) else {
            self.live_since = None;
            if let Some(shown) = self.shown_since
                && now.duration_since(shown) >= MIN_VISIBLE
            {
                self.shown_since = None;
                self.last_frame = None;
                push.push_idle();
            }
            return;
        };
        let live_since = *self.live_since.get_or_insert(now);
        if self.shown_since.is_none() && now.duration_since(live_since) < SHOW_DELAY {
            return;
        }
        // Every leaf finished but the tree still lives: the detach (and
        // the idle push) is imminent, so the last frame stands.
        let Some(label) = hub.headline() else {
            return;
        };
        self.shown_since.get_or_insert(now);
        let current = quantize(fraction);
        // The meter resets its high-water mark on an idle sample, so a new
        // operation attaching while the bar holds for MIN_VISIBLE after a
        // drain would restart the visible bar at the new operation's zero.
        // The last pushed frame is the floor until the new operation rises
        // past it: within a run the meter is already monotonic, so the floor
        // only ever bites across a drain.
        let current = self
            .last_frame
            .as_ref()
            .map_or(current, |(_, shown)| current.max(*shown));
        if self
            .last_frame
            .as_ref()
            .is_some_and(|(l, c)| l == &label && *c == current)
        {
            return;
        }
        self.last_frame = Some((label.clone(), current));
        push.push_progress(
            label.clone(),
            label,
            current,
            PROGRESS_TOTAL,
            Activity::General,
        );
    }

    /// The next moment `update` can change state without a hub event: the
    /// show deadline while an operation warms up, the detach poll while
    /// the indicator is up, or the earliest idle moment once the hub has
    /// drained under a still-visible bar.
    fn next_wake(&self, now: Instant) -> Option<Instant> {
        match (self.live_since, self.shown_since) {
            (Some(live), None) => {
                let deadline = live + SHOW_DELAY;
                // A lapsed deadline with the bar unshown means every leaf
                // finished but the tree still lives; poll for the detach
                // instead of re-arming a past instant, which would spin
                // the select loop.
                Some(if deadline > now {
                    deadline
                } else {
                    now + DETACH_POLL
                })
            }
            (Some(_), Some(_)) => Some(now + DETACH_POLL),
            (None, Some(shown)) => Some(shown + MIN_VISIBLE),
            (None, None) => None,
        }
    }
}

/// Quantizes a `0.0..=1.0` fraction to `current` of [`PROGRESS_TOTAL`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    reason = "the clamped fraction lands the cast in 0..=PROGRESS_TOTAL"
)]
fn quantize(fraction: f64) -> u64 {
    (fraction.clamp(0.0, 1.0) * PROGRESS_TOTAL as f64).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    use tokio::sync::broadcast;

    use crate::catalog::CatalogBus;
    use crate::menu::MenuBus;
    use crate::protocol::{Progress, Severity, StatusBarUpdate};
    use crate::status::StatusBus;

    /// A hub, a push handle over fresh buses, and the status receiver the
    /// renderer's frames land on (the push.rs wired() pattern).
    fn wired() -> (Arc<ProgressHub>, Push, broadcast::Receiver<StatusBarUpdate>) {
        let hub = Arc::new(ProgressHub::new());
        let status = StatusBus::new();
        let rx = status.subscribe();
        let catalog = CatalogBus::new();
        let menu = MenuBus::new(catalog.clone(), None);
        (hub, Push::new(status, catalog, menu), rx)
    }

    /// Lets the renderer task run everything currently pending.
    async fn settle() {
        for _ in 0..3 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_sub_second_operation_never_reaches_the_status_bar() {
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
        let (hub, push, mut rx) = wired();
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
    async fn an_operation_attaching_during_the_minimum_visible_hold_does_not_step_the_bar_backward()
    {
        let (hub, push, mut rx) = wired();
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
}
