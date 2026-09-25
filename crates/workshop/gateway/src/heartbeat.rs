//! The gateway heartbeat: a background task polling the gateway's
//! `GET /health` endpoint and publishing reachability to the rest of the
//! server.
//!
//! One task is spawned with the server ([`spawn`]): while the gateway
//! answers, it probes through [`GatewayClient::health`](crate::gateway::GatewayClient::health) on the fixed
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

use crate::gateway_binding::{GatewayBinding, GatewaySnapshot};

#[path = "heartbeat-refresh.rs"]
mod refresh;
pub use refresh::{refresh_catalog, refresh_profiles};

/// The status line announcing that the gateway answers its health probe.
pub const CONNECTED_LABEL: &str = "Connected to gateway";
/// The status line announcing that the gateway does not answer.
pub const UNREACHABLE_LABEL: &str = "Gateway unreachable";
/// The description sent with the unreachable announcement.
pub const UNREACHABLE_DESCRIPTION: &str = "the gateway does not answer its health probe";

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
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

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

/// Why a phase of the probe loop ended before it completed.
enum Phase {
    /// The stop signal fired, or a control channel closed: end the loop.
    Stop,
    /// The gateway binding was replaced: reset iteration state and
    /// restart the loop from its top.
    Rebind,
}

/// The probe loop, in named phases: wait out the probe interval, probe
/// the health endpoint, announce a transition, refresh stale menu sources,
/// and restore the selection. The stop signal wins every select, and a
/// replaced binding restarts the iteration from the top.
async fn run(
    gateway: &GatewayBinding,
    push: &Push,
    health: &GatewayHealth,
    interval: Duration,
    backoff: &ReconnectBackoff,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut last: Option<bool> = None;
    let mut refresh = RefreshState::default();
    let mut gateway_changed = gateway.subscribe();
    loop {
        match await_probe_interval(last, interval, backoff, push, stop, &mut gateway_changed).await
        {
            Ok(()) => {}
            Err(Phase::Stop) => break,
            Err(Phase::Rebind) => {
                last = None;
                refresh = RefreshState::default();
                continue;
            }
        }
        let snapshot = gateway.snapshot();
        let generation = snapshot.generation();
        let reachable = match probe(snapshot.as_ref(), stop, &mut gateway_changed).await {
            Ok(reachable) => reachable,
            Err(Phase::Stop) => break,
            Err(Phase::Rebind) => {
                last = None;
                refresh = RefreshState::default();
                continue;
            }
        };
        if gateway.generation() != generation {
            last = None;
            refresh = RefreshState::default();
            continue;
        }
        announce_transition(push, health, reachable, &mut last);
        if !reachable {
            refresh = RefreshState::default();
            continue;
        }
        if !refresh.profiles_ready || !refresh.catalog_ready {
            match refresh_sources(
                snapshot.as_ref(),
                push,
                &mut refresh,
                stop,
                &mut gateway_changed,
            )
            .await
            {
                Ok(()) => {}
                Err(Phase::Stop) => break,
                Err(Phase::Rebind) => {
                    last = None;
                    refresh = RefreshState::default();
                    continue;
                }
            }
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
    }
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
    stop: &mut oneshot::Receiver<()>,
    gateway_changed: &mut watch::Receiver<u64>,
) -> Result<(), Phase> {
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
        return Err(Phase::Stop);
    };
    tokio::select! {
        _ = &mut *stop => Err(Phase::Stop),
        changed = gateway_changed.changed() => {
            if changed.is_err() {
                Err(Phase::Stop)
            } else {
                Err(Phase::Rebind)
            }
        }
        () = tokio::time::sleep(wait) => Ok(()),
    }
}

/// One bounded health probe against the current snapshot's client.
async fn probe(
    snapshot: &GatewaySnapshot,
    stop: &mut oneshot::Receiver<()>,
    gateway_changed: &mut watch::Receiver<u64>,
) -> Result<bool, Phase> {
    tokio::select! {
        _ = &mut *stop => Err(Phase::Stop),
        changed = gateway_changed.changed() => {
            if changed.is_err() {
                Err(Phase::Stop)
            } else {
                Err(Phase::Rebind)
            }
        }
        reachable = snapshot.client().health() => Ok(reachable),
    }
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

/// Refreshes the menu sources that have not converged, racing the stop
/// signal and a binding replacement against the refresh. All menu state
/// is server-owned and reaches the UI via socket pushes - the UI fetches
/// nothing on boot - so every transition into reachable, boot's first
/// probe included, (re)populates the profile state and the model catalog.
/// Healthy ticks independently repeat either refresh until both sources
/// are populated, because health and one ready source do not imply the
/// other source is ready; the probe interval bounds retries and keeps this
/// from becoming a busy loop.
async fn refresh_sources(
    snapshot: &GatewaySnapshot,
    push: &Push,
    refresh: &mut RefreshState,
    stop: &mut oneshot::Receiver<()>,
    gateway_changed: &mut watch::Receiver<u64>,
) -> Result<(), Phase> {
    tokio::select! {
        _ = &mut *stop => Err(Phase::Stop),
        changed = gateway_changed.changed() => {
            if changed.is_err() {
                Err(Phase::Stop)
            } else {
                Err(Phase::Rebind)
            }
        }
        () = refresh_incomplete_sources(snapshot, push, refresh) => Ok(()),
    }
}

/// Refreshes only the gateway-owned menu sources that have not converged.
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
