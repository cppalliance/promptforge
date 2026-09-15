//! The hub-to-status-bar renderer: a task that samples the process
//! [`ProgressHub`] and drives the status bar's progress indicator through
//! [`Push`], so the anti-flicker policy lives here and the UI stays dumb.
//!
//! The indicator appears only once an operation has been live for
//! `SHOW_DELAY`, stays up at least `MIN_VISIBLE` once shown, and
//! displays the monotonic aggregate from [`ProgressMeter`]: the bar never
//! flashes for sub-second work, never resets mid-operation, and never
//! steps backward. Status texts ("Listening...", failures) stay explicit
//! push calls in the subsystems that own them; the trees own only
//! fractional progress. When the hub's last tree detaches, the renderer
//! returns the bar to rest with [`Push::push_idle`].

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::oneshot;
use tokio::time::Instant;

use shared_progress::{ProgressHub, ProgressMeter};

use workshop_protocol::Activity;
use workshop_registry::Push;

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
pub struct Renderer {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Renderer {
    /// Signals the renderer to stop and waits for its task to finish.
    pub async fn shutdown(mut self) {
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
pub fn spawn(hub: Arc<ProgressHub>, push: Push) -> Renderer {
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
#[path = "progress-tests.rs"]
mod tests;
