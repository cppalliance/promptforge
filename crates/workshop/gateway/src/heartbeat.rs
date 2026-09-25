//! The gateway heartbeat: a background task polling the gateway's
//! `GET /health` endpoint and publishing reachability to the rest of the
//! server.
//!
//! One task is spawned with the server ([`spawn`]): while the gateway
//! answers, it probes through [`GatewayClient::health`](crate::client::GatewayClient::health) on the fixed
//! [`HEARTBEAT_INTERVAL`] and publishes the outcome to the shared
//! [`GatewayHealth`] flag the gateway-dependent routes read; while the
//! gateway is unreachable, the next probe instead waits out a delay
//! drawn from the shared [`ReconnectBackoff`] - jittered, escalating,
//! and reset only by useful work elsewhere (a delivered token or a
//! successful completion), never by a probe that merely connects, so a
//! gateway that flaps without delivering keeps escalating. When the
//! backoff's total-delay budget exhausts, the loop reports the give-up
//! on the status bus and stops probing for the life of the process.
//! Status subscribers hear about transitions only - the first probe reports
//! the initial state ("Connected to gateway" or "Gateway unreachable"),
//! and after that a status update fires when the answer changes, so a
//! steady state never spams the status bar. Every transition also feeds
//! the Model menu's reachability (so `chat_ready` flips with the
//! gateway), and a transition to reachable (boot's first probe included)
//! refreshes the gateway's profile state and model catalog into their
//! buses. If simultaneous startup leaves either source empty, later healthy
//! ticks retry each source independently until the profile and a selectable
//! model are both ready, then restore the selection exactly once.
//!
//! The task stops through its [`Heartbeat`] handle: the signal wins the
//! loop's selects, so shutdown never waits out a tick or an in-flight
//! probe. The server runs the shutdown inside its graceful-shutdown future.

use std::time::Duration;

use tokio::sync::{oneshot, watch};

use workshop_protocol::{Activity, Severity, StatusBarUpdate};
use workshop_registry::Push;
use workshop_support::ReconnectBackoff;

use crate::binding::{GatewayBinding, GatewaySnapshot};
use crate::progress::{Ended, Signals, until};
use crate::refresh::{refresh_catalog, refresh_profiles};

/// The status line announcing that the gateway answers its health probe.
pub(crate) const CONNECTED_LABEL: &str = "Connected to gateway";
/// The status line announcing that the gateway does not answer.
pub(crate) const UNREACHABLE_LABEL: &str = "Gateway unreachable";
/// The description sent with the unreachable announcement.
pub(crate) const UNREACHABLE_DESCRIPTION: &str = "the gateway does not answer its health probe";

/// The status frame a joining session hears first: the bus's retained
/// frame, unless that frame is one of the heartbeat's transition
/// announcements. A transition describes a past moment, not the current
/// state - the boot-time "Connected to gateway" outlives itself within
/// seconds - so the line is recomputed from the current probe. A retained
/// frame that reports real work (a download's progress, a chat's activity)
/// replays as-is.
#[must_use]
pub fn join_status(
    retained: Option<StatusBarUpdate>,
    health: &GatewayHealth,
) -> Option<StatusBarUpdate> {
    let update = retained?;
    if update.label != CONNECTED_LABEL && update.label != UNREACHABLE_LABEL {
        return Some(update);
    }
    let reachable = health.is_reachable();
    Some(StatusBarUpdate {
        label: if reachable {
            "Ready"
        } else {
            UNREACHABLE_LABEL
        }
        .to_owned(),
        description: if reachable {
            "idle".to_owned()
        } else {
            UNREACHABLE_DESCRIPTION.to_owned()
        },
        busy: false,
        severity: Severity::Info,
        activity: Activity::General,
    })
}

/// How often the heartbeat probes a reachable gateway. Hardcoded for
/// now; a configuration knob may follow once someone needs one. Probes
/// of an unreachable gateway follow the [`ReconnectBackoff`] instead.
pub(crate) const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Shared gateway reachability, written by the heartbeat and read by the
/// gateway-dependent routes.
///
/// The flag starts optimistic (`true`): until the first probe lands, a
/// request flows to the gateway and fails or succeeds on its own merits,
/// which keeps a server running without a heartbeat (every router-only
/// test) behaving as it did before the heartbeat existed.
#[derive(Debug, Clone)]
pub struct GatewayHealth {
    reachable: watch::Sender<bool>,
}

impl GatewayHealth {
    /// Starts the flag optimistic; see the type docs for why.
    #[must_use]
    pub fn new() -> Self {
        Self {
            reachable: watch::channel(true).0,
        }
    }

    /// Whether the gateway is currently believed reachable.
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        *self.reachable.borrow()
    }

    /// Subscribes to reachability changes. The current value is visible
    /// immediately through the receiver; each later publish that flips the
    /// flag notifies. The progress subscriber waits on this to hold its
    /// subscription only while the gateway answers.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.reachable.subscribe()
    }

    /// Publishes one probe outcome. The heartbeat is the only production
    /// writer; tests publish directly to pin the degraded paths.
    pub fn publish(&self, reachable: bool) {
        self.reachable.send_if_modified(|current| {
            let changed = *current != reachable;
            *current = reachable;
            changed
        });
    }
}

impl Default for GatewayHealth {
    fn default() -> Self {
        Self::new()
    }
}

/// A running heartbeat task.
///
/// [`Heartbeat::shutdown`] signals the loop to stop and awaits the task.
/// Dropping the handle without shutting down still stops the task at its
/// next select point, because the closed channel resolves the stop branch.
#[derive(Debug)]
pub struct Heartbeat {
    stop: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Heartbeat {
    /// Signals the heartbeat to stop and waits for its task to finish.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

/// Spawns the heartbeat loop against `client`, reporting transitions
/// through `push` and publishing reachability to `health` and to the
/// menu behind `push`, which recomputes `chat_ready` from it. A
/// transition to reachable - boot's first probe included - refreshes the
/// gateway's profile state and model catalog through the same handle,
/// then restores a model selection when none is applied. Healthy ticks
/// repeat each incomplete refresh independently, covering a gateway whose
/// health endpoint becomes ready before its catalog or profile state. The first
/// probe runs immediately; later probes follow `interval` while the
/// gateway answers and draw from `backoff` while it does not, ending the
/// loop when the backoff's budget exhausts.
#[must_use]
pub fn spawn(
    gateway: GatewayBinding,
    push: Push,
    health: GatewayHealth,
    interval: Duration,
    backoff: ReconnectBackoff,
) -> Heartbeat {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&gateway, &push, &health, interval, &backoff, &mut stopped).await;
    });
    Heartbeat {
        stop: Some(stop),
        task: Some(task),
    }
}

/// Which menu sources have converged since the gateway last became
/// reachable, and whether the selection restore has run.
#[derive(Default)]
struct RefreshState {
    profiles_ready: bool,
    catalog_ready: bool,
    selection_restored: bool,
}

/// What the probe loop has learned about the current binding: the last
/// probe verdict and which menu sources have converged. A replaced
/// binding starts it over.
#[derive(Default)]
struct BindingState {
    last: Option<bool>,
    refresh: RefreshState,
}

/// The probe loop: repeats [`cycle`] until the stop signal or an
/// exhausted backoff budget ends it. A replaced binding restarts the
/// cycle from the top with fresh state.
async fn run(
    gateway: &GatewayBinding,
    push: &Push,
    health: &GatewayHealth,
    interval: Duration,
    backoff: &ReconnectBackoff,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut gateway_changed = gateway.subscribe();
    let mut signals = Signals {
        stop,
        reachable: None,
        gateway_changed: &mut gateway_changed,
    };
    let mut state = BindingState::default();
    loop {
        match cycle(
            gateway,
            push,
            health,
            interval,
            backoff,
            &mut signals,
            &mut state,
        )
        .await
        {
            Ok(()) => {}
            Err(Ended::Stop) => break,
            // With no reachability watch armed, `Lost` never arrives.
            Err(Ended::Rebind | Ended::Lost) => state = BindingState::default(),
        }
    }
}

/// One pass of the probe loop, in named phases: wait out the probe
/// interval, probe the health endpoint, announce a transition, refresh
/// stale menu sources, and restore the selection. Every wait goes through
/// [`until`], so the stop signal and a replaced binding end the pass
/// early.
async fn cycle(
    gateway: &GatewayBinding,
    push: &Push,
    health: &GatewayHealth,
    interval: Duration,
    backoff: &ReconnectBackoff,
    signals: &mut Signals<'_>,
    state: &mut BindingState,
) -> Result<(), Ended> {
    await_probe_interval(state.last, interval, backoff, push, signals).await?;
    let snapshot = gateway.snapshot();
    let generation = snapshot.generation();
    let reachable = until(snapshot.client().health(), signals).await?;
    if gateway.generation() != generation {
        return Err(Ended::Rebind);
    }
    announce_transition(push, health, reachable, &mut state.last);
    let refresh = &mut state.refresh;
    if !reachable {
        *refresh = RefreshState::default();
        return Ok(());
    }
    if !refresh.profiles_ready || !refresh.catalog_ready {
        until(
            refresh_incomplete_sources(&snapshot, push, refresh),
            signals,
        )
        .await?;
    }
    if refresh.profiles_ready && refresh.catalog_ready && !refresh.selection_restored {
        // A fresh boot has no selection, so restore the remembered
        // model for the now-known active profile (else the first
        // catalog model); a reconnect whose selection survived the
        // outage is a no-op. This branch runs exactly once per reachable
        // convergence because both readiness facts remain true.
        push.menu().restore_selection();
        refresh.selection_restored = true;
    }
    Ok(())
}

/// The wait before a probe: skipped on the first, `interval` while the
/// gateway answered last time (measured from the previous probe's
/// completion, so a slow probe never bunches into a catch-up burst), and
/// the backoff's next delay while it did not, ending the loop when the
/// backoff's budget exhausts.
async fn await_probe_interval(
    last: Option<bool>,
    interval: Duration,
    backoff: &ReconnectBackoff,
    push: &Push,
    signals: &mut Signals<'_>,
) -> Result<(), Ended> {
    let Some(reachable) = last else {
        return Ok(());
    };
    let wait = if reachable {
        interval
    } else if let Some(delay) = backoff.next_delay() {
        delay
    } else {
        push.push_failure(
            "Gateway reconnect stopped",
            "the reconnect budget is exhausted; restart the workshop to retry",
            Activity::General,
        );
        return Err(Ended::Stop);
    };
    until(tokio::time::sleep(wait), signals).await
}

/// Publishes the probe outcome and, when it differs from the last
/// verdict, reports the transition to the menu and the status bar.
fn announce_transition(
    push: &Push,
    health: &GatewayHealth,
    reachable: bool,
    last: &mut Option<bool>,
) {
    health.publish(reachable);
    let transitioned = *last != Some(reachable);
    *last = Some(reachable);
    if transitioned {
        // The menu recomputes chat_ready from reachability, so the
        // verdict feeds it before any slower refresh work below.
        push.menu().set_gateway_reachable(reachable);
        if reachable {
            push.push_status_update(
                CONNECTED_LABEL,
                "the gateway answers its health probe",
                Activity::General,
            );
        } else {
            push.push_status_update(
                UNREACHABLE_LABEL,
                UNREACHABLE_DESCRIPTION,
                Activity::General,
            );
        }
    }
}

/// Refreshes only the gateway-owned menu sources that have not converged.
/// All menu state is server-owned and reaches the UI via socket pushes -
/// the UI fetches nothing on boot - so every transition into reachable,
/// boot's first probe included, (re)populates the profile state and the
/// model catalog. Healthy ticks independently repeat either refresh until
/// both sources are populated, because health and one ready source do not
/// imply the other source is ready; the probe interval bounds retries and
/// keeps this from becoming a busy loop.
async fn refresh_incomplete_sources(
    snapshot: &GatewaySnapshot,
    push: &Push,
    refresh: &mut RefreshState,
) {
    match (refresh.profiles_ready, refresh.catalog_ready) {
        (false, false) => {
            (refresh.profiles_ready, refresh.catalog_ready) = tokio::join!(
                refresh_profiles(snapshot.client(), push),
                refresh_catalog(snapshot.client(), push)
            );
        }
        (false, true) => refresh.profiles_ready = refresh_profiles(snapshot.client(), push).await,
        (true, false) => refresh.catalog_ready = refresh_catalog(snapshot.client(), push).await,
        (true, true) => {}
    }
}
#[cfg(test)]
#[path = "heartbeat-tests.rs"]
mod tests;
