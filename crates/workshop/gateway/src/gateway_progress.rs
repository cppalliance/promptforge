//! The gateway progress subscriber: a background task that reads the
//! gateway's `GET /admin/progress` snapshot stream and drives the status
//! bar's busy indicator through the registry's [`Push`] facade, so
//! gateway-side work (model downloads, profile switches) shows as a
//! barberpole plus the gateway's own text.
//!
//! The task follows the heartbeat's lifecycle posture: spawned with the
//! server, stopped through its [`Subscriber`] handle inside the same
//! graceful-shutdown signal, and driven by the shared [`GatewayHealth`]
//! verdict rather than by probes of its own. It subscribes while the
//! gateway reads reachable and idles while it does not; a reconnect
//! resubscribes. Each snapshot passes through the anti-flicker
//! `Presenter`, which decides when the bar shows and when it rests.
//! When the subscription drops - a lost connection or an unreachable
//! verdict - the bar returns to rest, because progress from a gateway
//! the workshop can no longer hear is stale; a bar that only just
//! appeared still waits out its minimum visible time first, so a
//! dropped stream cannot flash it.

#[path = "gateway_progress-presenter.rs"]
mod presenter;

use std::future::Future;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::{oneshot, watch};
use tokio::time::Instant;

use workshop_registry::Push;

use crate::gateway_binding::GatewayBinding;
use crate::heartbeat::GatewayHealth;

#[cfg(test)]
pub(crate) use presenter::{MIN_VISIBLE, SHOW_DELAY};
pub(crate) use presenter::{Policy, Presenter};

/// How long a resubscribe waits when the stream ended while the gateway
/// still reads reachable, so an endpoint that accepts and immediately
/// closes cannot spin the loop. A reachability flip restarts at once;
/// matched to the heartbeat's probe cadence.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

/// The subscriber's durations, injectable so tests can shorten them.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// Wait before resubscribing to a stream that ended while reachable.
    pub(crate) resubscribe_delay: Duration,
    /// The anti-flicker policy the presenter runs.
    pub(crate) policy: Policy,
}

impl Timing {
    /// The production timing.
    pub(crate) const DEFAULT: Self = Self {
        resubscribe_delay: RESUBSCRIBE_DELAY,
        policy: Policy::DEFAULT,
    };
}

/// A running subscriber task.
///
/// [`Subscriber::shutdown`] signals the task to stop and awaits it.
/// Dropping the handle without shutting down still stops the task at its
/// next select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub struct Subscriber {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Subscriber {
    /// Signals the subscriber to stop and waits for its task to finish.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the subscriber task against the gateway behind `gateway`,
/// pushing busy and idle frames through `push` while `health` reads
/// reachable.
#[must_use]
pub fn spawn(gateway: GatewayBinding, push: Push, health: GatewayHealth) -> Subscriber {
    spawn_with_timing(gateway, push, health, Timing::DEFAULT)
}

/// [`spawn`] with every duration injected, so tests can shorten them.
pub(crate) fn spawn_with_timing(
    gateway: GatewayBinding,
    push: Push,
    health: GatewayHealth,
    timing: Timing,
) -> Subscriber {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&gateway, &push, &health, timing, &mut stopped).await;
    });
    Subscriber {
        stop: Some(stop),
        task: Some(task),
    }
}

/// Why one wait ended early.
enum Ended {
    /// The stop signal fired, or a control channel closed: the task
    /// returns.
    Stop,
    /// The endpoint binding was replaced: subscribe to the new one now.
    Rebind,
    /// The gateway's reachability changed, or the stream closed: fall
    /// back to the reconnect loop.
    Lost,
}

/// The control signals every wait in the loop listens to beside its own
/// future.
struct Signals<'a> {
    stop: &'a mut oneshot::Receiver<()>,
    reachable: &'a mut watch::Receiver<bool>,
    gateway_changed: &'a mut watch::Receiver<u64>,
}

/// The subscription loop: idle while the gateway is unreachable, and while
/// reachable hold one subscription whose snapshots drive the presenter.
/// Every wait goes through [`until`], so the stop signal wins each select
/// and the presenter's deadlines keep ticking between subscriptions.
async fn run(
    gateway: &GatewayBinding,
    push: &Push,
    health: &GatewayHealth,
    timing: Timing,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut reachable = health.subscribe();
    let mut gateway_changed = gateway.subscribe();
    let mut signals = Signals {
        stop,
        reachable: &mut reachable,
        gateway_changed: &mut gateway_changed,
    };
    let mut presenter = Presenter::new(timing.policy);
    loop {
        while !*signals.reachable.borrow_and_update() {
            match until(
                std::future::pending::<()>(),
                &mut signals,
                &mut presenter,
                push,
            )
            .await
            {
                Err(Ended::Stop) => return,
                Err(Ended::Lost | Ended::Rebind) | Ok(()) => {}
            }
        }
        let snapshot = gateway.snapshot();
        let stream = match until(
            snapshot.client().subscribe_progress(),
            &mut signals,
            &mut presenter,
            push,
        )
        .await
        {
            Err(Ended::Stop) => return,
            Err(Ended::Lost | Ended::Rebind) => continue,
            Ok(Ok(stream)) => stream,
            Ok(Err(error)) => {
                tracing::warn!(%error, "gateway progress subscription failed");
                match until(
                    tokio::time::sleep(timing.resubscribe_delay),
                    &mut signals,
                    &mut presenter,
                    push,
                )
                .await
                {
                    Err(Ended::Stop) => return,
                    Err(Ended::Lost | Ended::Rebind) | Ok(()) => continue,
                }
            }
        };
        tokio::pin!(stream);
        let ended = loop {
            match until(stream.next(), &mut signals, &mut presenter, push).await {
                Err(ended) => break ended,
                Ok(Some(Ok(snapshot))) => presenter.apply(snapshot, Instant::now(), push),
                // One malformed snapshot or a terminal read failure; the
                // stream itself decides which by continuing or ending.
                Ok(Some(Err(error))) => {
                    tracing::warn!(%error, "gateway progress snapshot skipped");
                }
                Ok(None) => break Ended::Lost,
            }
        };
        match ended {
            Ended::Stop => return,
            // The subscription is gone, so its progress is stale: the bar
            // rests (after any minimum-visible hold) until the next
            // subscription reports work.
            Ended::Rebind => {
                presenter.detach(Instant::now(), push);
                continue;
            }
            Ended::Lost => presenter.detach(Instant::now(), push),
        }
        if *signals.reachable.borrow_and_update() {
            match until(
                tokio::time::sleep(timing.resubscribe_delay),
                &mut signals,
                &mut presenter,
                push,
            )
            .await
            {
                Err(Ended::Stop) => return,
                Err(Ended::Lost | Ended::Rebind) | Ok(()) => {}
            }
        }
    }
}

/// Awaits `future` with the control signals and the presenter's next
/// deadline armed beside it. A control signal ends the wait early with
/// its [`Ended`]; a presenter deadline ticks the presenter and keeps
/// waiting, so a minimum-visible hold lapses on time even while the loop
/// is between subscriptions. Both watch senders sit in `AppState` for
/// the process lifetime, so a closed watch means shutdown.
async fn until<F: Future>(
    future: F,
    signals: &mut Signals<'_>,
    presenter: &mut Presenter,
    push: &Push,
) -> Result<F::Output, Ended> {
    tokio::pin!(future);
    loop {
        tokio::select! {
            _ = &mut *signals.stop => return Err(Ended::Stop),
            changed = signals.reachable.changed() => {
                return Err(if changed.is_err() { Ended::Stop } else { Ended::Lost });
            }
            changed = signals.gateway_changed.changed() => {
                return Err(if changed.is_err() { Ended::Stop } else { Ended::Rebind });
            }
            () = wake_at(presenter.next_wake()) => presenter.tick(Instant::now(), push),
            output = &mut future => return Ok(output),
        }
    }
}

/// Waits for `at`, or forever when there is no pending deadline.
async fn wake_at(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
#[path = "gateway_progress-tests.rs"]
mod tests;
