//! The gateway progress subscriber: a background task that imports the
//! gateway's `GET /admin/progress` event stream into the workshop
//! [`ProgressHub`] as a [`RemoteOperation`], so gateway-side work (model
//! downloads, profile switches) renders on the status bar through the same
//! renderer task as local operations.
//!
//! The task follows the heartbeat's lifecycle posture: spawned with the
//! server, stopped through its [`Subscriber`] handle inside the same
//! graceful-shutdown signal, and driven by the shared [`GatewayHealth`]
//! verdict rather than by probes of its own. It subscribes while the
//! gateway reads reachable and idles while it does not; a reconnect
//! resubscribes, and each subscription tracks one import per upstream
//! operation id, so interleaved work stays separate and a finished operation
//! detaches without closing the long-lived event stream.
//! When the subscription drops - a lost connection or an unreachable
//! verdict - the import detaches with it, because progress from a gateway
//! the workshop can no longer hear is stale, not informative.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::oneshot;

use shared_progress::{EventState, OperationId, ProgressHub, RemoteOperation};

use crate::gateway_binding::GatewayBinding;
use crate::heartbeat::GatewayHealth;

/// How long a resubscribe waits when the stream ended while the gateway
/// still reads reachable, so an endpoint that accepts and immediately
/// closes cannot spin the loop. A reachability flip restarts at once;
/// matched to the heartbeat's probe cadence.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

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

/// Spawns the subscriber task against the gateway at `base_url`,
/// importing its progress events into `hub` while `health` reads
/// reachable.
#[must_use]
pub fn spawn(gateway: GatewayBinding, hub: Arc<ProgressHub>, health: GatewayHealth) -> Subscriber {
    spawn_with_delay(gateway, hub, health, RESUBSCRIBE_DELAY)
}

/// [`spawn`] with the resubscribe delay injected, so tests can shorten it.
fn spawn_with_delay(
    gateway: GatewayBinding,
    hub: Arc<ProgressHub>,
    health: GatewayHealth,
    resubscribe_delay: Duration,
) -> Subscriber {
    let (stop, mut stopped) = oneshot::channel();
    let task = tokio::spawn(async move {
        run(&gateway, &hub, &health, resubscribe_delay, &mut stopped).await;
    });
    Subscriber {
        stop: Some(stop),
        task: Some(task),
    }
}

/// The subscription loop: idle while the gateway is unreachable, and while
/// reachable hold one subscription whose events drive operation-id-keyed
/// [`RemoteOperation`] imports. An operation-level terminal event
/// detaches that import while the subscription remains open. The stop
/// signal wins every select, so shutdown never waits out a stream read, a
/// connect, or a resubscribe delay.
async fn run(
    gateway: &GatewayBinding,
    hub: &Arc<ProgressHub>,
    health: &GatewayHealth,
    resubscribe_delay: Duration,
    stop: &mut oneshot::Receiver<()>,
) {
    let mut reachable = health.subscribe();
    let mut gateway_changed = gateway.subscribe();
    'reconnect: loop {
        while !*reachable.borrow_and_update() {
            tokio::select! {
                _ = &mut *stop => return,
                changed = gateway_changed.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                changed = reachable.changed() => {
                    // The sender lives in AppState for the process
                    // lifetime, so a closed watch means shutdown.
                    if changed.is_err() {
                        return;
                    }
                }
            }
        }
        let snapshot = gateway.snapshot();
        let stream = tokio::select! {
            _ = &mut *stop => return,
            _ = reachable.changed() => continue,
            changed = gateway_changed.changed() => {
                if changed.is_err() {
                    return;
                }
                continue;
            }
            result = snapshot.client().subscribe_progress() => match result {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "gateway progress subscription failed");
                    tokio::select! {
                        _ = &mut *stop => return,
                        _ = reachable.changed() => {}
                        changed = gateway_changed.changed() => {
                            if changed.is_err() {
                                return;
                            }
                        }
                        () = tokio::time::sleep(resubscribe_delay) => {}
                    }
                    continue;
                }
            },
        };
        let mut remotes: HashMap<OperationId, RemoteOperation> = HashMap::new();
        tokio::pin!(stream);
        loop {
            tokio::select! {
                _ = &mut *stop => return,
                _ = reachable.changed() => break,
                changed = gateway_changed.changed() => {
                    if changed.is_err() {
                        return;
                    }
                    continue 'reconnect;
                }
                item = stream.next() => match item {
                    Some(Ok(event)) => {
                        let operation = event.operation;
                        if matches!(event.state, EventState::OperationFinished) {
                            remotes.remove(&operation);
                            continue;
                        }
                        remotes
                            .entry(operation)
                            .or_insert_with(|| RemoteOperation::attach(hub))
                            .apply(&event);
                    }
                    // One malformed event or a terminal read failure; the
                    // stream itself decides which by continuing or ending.
                    Some(Err(error)) => {
                        tracing::warn!(%error, "gateway progress event skipped");
                    }
                    None => break,
                }
            }
        }
        drop(remotes);
        if *reachable.borrow_and_update() {
            tokio::select! {
                _ = &mut *stop => return,
                _ = reachable.changed() => {}
                changed = gateway_changed.changed() => {
                    if changed.is_err() {
                        return;
                    }
                }
                () = tokio::time::sleep(resubscribe_delay) => {}
            }
        }
    }
}

#[cfg(test)]
#[path = "gateway_progress-tests.rs"]
mod tests;
