//! The command queue: serialized, debounced, cancellable gateway commands.
//!
//! Everything slow the gateway does - the boot-time profile load, config
//! applies, model provisioning and unloads - runs as a [`Command`] on one
//! worker task draining a shared pending deque FIFO, so downloads never
//! fight each other for bandwidth and the listener stays live while they
//! run.
//! The worker begins one [`Activity`] on the process hub per command it
//! runs, labelled with the command's name, and hands it to the body, which
//! writes its stages into the text; every command holds a
//! [`CancellationToken`] the worker honors at chunk and phase boundaries.
//! The in-process status ([`CommandQueue::active_command`] and
//! [`CommandQueue::pending_commands`]) feeds the tray and the admin routes.

use std::collections::VecDeque;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use futures_util::future::BoxFuture;
use gateway_config::ProfileName;
use gateway_progress::{Activity, ProgressHub};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::error::GatewayError;
// Only the local-inference command bodies leave the async executor.
#[cfg(feature = "local")]
use crate::error::blocking;

pub(crate) mod apply;
mod queue;

use self::apply::ApplySnapshot;

/// The `ApplyConfig` command's display name: the status bar and tray show
/// it, and the apply's cancellation error names it.
const APPLY_CONFIG_LABEL: &str = "apply-config";

/// What a settled command produced: the profile name for `LoadProfile`, a
/// summary line for the rest.
pub(crate) type Outcome = Result<String, GatewayError>;

/// The outcome as shared with every waiter on one command.
pub(crate) type SharedOutcome = Arc<Outcome>;

/// A command the queue worker can run.
#[derive(Debug)]
pub(crate) enum Command {
    /// The runner's boot load: provision and spawn the selected profile's
    /// local models into the live routing table, then make the process's
    /// one guarded STT load. The runner enqueues it once, right after the
    /// bind, and only when a profile is selected; nothing else produces
    /// it. The selection it loads is the boot selection, so a command-line
    /// or environment override stays ephemeral.
    LoadProfile {
        /// The profile to load.
        name: ProfileName,
        /// Cancellation token, checked at chunk and phase boundaries.
        token: CancellationToken,
    },
    /// Applies the staged configuration: promotes the captured shadows and
    /// swaps the remote routing table to the snapshot's config in one live
    /// write, leaving the local runtime as it is. Nothing touches a real
    /// file before that commit, so a failed or cancelled apply leaves every
    /// shadow staged for a retry.
    ///
    /// Commands run FIFO: an `ApplyConfig` enqueued while the boot
    /// `LoadProfile` is pending or active queues behind it and never
    /// cancels it, so the reload merges the children the boot load
    /// published. A second `ApplyConfig` attaches to the first and shares
    /// its outcome.
    ApplyConfig {
        /// The pending config and shadow contents the route captured under
        /// the apply lock.
        snapshot: ApplySnapshot,
        /// Cancellation token, checked at phase boundaries and again under
        /// the apply lock at the commit.
        token: CancellationToken,
    },
    /// Downloads and verifies one model into the artifact store. Spawning it
    /// into the routing table needs the model's full configuration, which
    /// this command does not include; that arrives with the command's first
    /// producer.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no producer exists yet; the config UI's model download wires it in a later step"
        )
    )]
    ProvisionModel {
        /// The model name, for status display and debounce.
        name: String,
        /// The source URL or path.
        source: String,
        /// Cancellation token, checked at chunk and phase boundaries.
        token: CancellationToken,
    },
    /// Stops one local model's `llama-server` child and drops it from the
    /// routing table. Not debounced: unloads are fast and order-independent.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "no producer exists yet; the admin queue routes wire it in a later step"
        )
    )]
    UnloadModel {
        /// The model to stop.
        name: String,
    },
}

impl Command {
    /// The runner's boot load for `name`.
    pub(crate) fn load_profile(name: ProfileName, token: CancellationToken) -> Command {
        Command::LoadProfile { name, token }
    }

    /// The command's display name, for status readouts and log lines.
    pub(crate) fn label(&self) -> String {
        match self {
            Command::LoadProfile { name, .. } => format!("load-profile: {name}"),
            Command::ApplyConfig { .. } => APPLY_CONFIG_LABEL.to_owned(),
            Command::ProvisionModel { name, .. } => format!("provision-model: {name}"),
            Command::UnloadModel { name } => format!("unload-model: {name}"),
        }
    }

    /// The token the worker honors, when the command has one.
    pub(crate) fn token(&self) -> Option<CancellationToken> {
        match self {
            Command::LoadProfile { token, .. }
            | Command::ApplyConfig { token, .. }
            | Command::ProvisionModel { token, .. } => Some(token.clone()),
            Command::UnloadModel { .. } => None,
        }
    }

    /// The identity debounce compares on, or `None` for commands that are
    /// never debounced.
    fn debounce_key(&self) -> Option<DebounceKey> {
        match self {
            Command::LoadProfile { name, .. } => Some(DebounceKey::Profile(name.to_string())),
            Command::ApplyConfig { .. } => Some(DebounceKey::Apply),
            Command::ProvisionModel { name, .. } => Some(DebounceKey::Model(name.clone())),
            Command::UnloadModel { .. } => None,
        }
    }
}

/// The identity a command debounces on: profile name for `LoadProfile`,
/// model name for `ProvisionModel`, and one shared slot for `ApplyConfig`,
/// so at most one apply is ever pending or active.
#[derive(Debug, Clone, PartialEq, Eq)]
enum DebounceKey {
    Profile(String),
    Apply,
    Model(String),
}

/// One waiting command's queue-side record. The deque owns the command
/// itself: there is no separate transport, so a cancelled entry is gone
/// completely rather than skipped when it surfaces.
#[derive(Debug)]
struct PendingEntry {
    id: u64,
    command: Command,
    key: Option<DebounceKey>,
    label: String,
    queued_at: Instant,
    waiters: Vec<oneshot::Sender<SharedOutcome>>,
}

impl PendingEntry {
    /// Settles every waiter of a command that never started.
    fn settle(self, outcome: Outcome) {
        let outcome = Arc::new(outcome);
        for waiter in self.waiters {
            let _ = waiter.send(Arc::clone(&outcome));
        }
    }
}

/// The running command's queue-side record.
#[derive(Debug)]
struct ActiveEntry {
    id: u64,
    key: Option<DebounceKey>,
    label: String,
    started_at: Instant,
    token: Option<CancellationToken>,
    waiters: Vec<oneshot::Sender<SharedOutcome>>,
}

/// The queue's shared state: the running command, the waiting commands, and
/// the lifecycle flags. A plain mutex, so the tray's synchronous status tick
/// can read it; never held across an `.await`.
#[derive(Debug, Default)]
struct QueueState {
    active: Option<ActiveEntry>,
    pending: VecDeque<PendingEntry>,
    next_id: u64,
    closed: bool,
    /// Test hook replacing the command body a spawned worker runs, so a
    /// `serve` test can park the worker on a command that ignores its
    /// cancellation token.
    #[cfg(test)]
    executor_override: Option<ExecutorOverride>,
}

/// The test hook's held executor; a `dyn Fn` has no `Debug`, so the
/// wrapper prints a placeholder.
#[cfg(test)]
struct ExecutorOverride(Arc<Executor>);

#[cfg(test)]
impl std::fmt::Debug for ExecutorOverride {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExecutorOverride(..)")
    }
}

/// A point-in-time readout of the running command. What the command is
/// doing right now is the hub's [`Progress`](gateway_api_types::Progress)
/// text.
#[derive(Debug, Clone)]
pub(crate) struct CommandStatus {
    /// The command's display name, for example `load-profile: main`.
    pub(crate) name: String,
    /// When the worker started the command.
    pub(crate) started_at: Instant,
}

/// A point-in-time readout of one waiting command.
#[derive(Debug, Clone)]
pub(crate) struct CommandSummary {
    /// The command's display name.
    pub(crate) name: String,
    /// When the command entered the queue.
    pub(crate) queued_at: Instant,
}

/// What an enqueue returns: the queue entry the command runs as, and the
/// receiver for its settled outcome. A command dropped by the debounce
/// attaches both to the command it duplicated.
#[derive(Debug)]
pub(crate) struct Enqueued {
    /// The queue entry the command runs as; a debounced duplicate shares
    /// the entry it attached to.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "read only by tests, which pin the debounce attaching a duplicate to the entry it duplicated"
        )
    )]
    entry: u64,
    /// Resolves when the command settles.
    pub(crate) outcome: oneshot::Receiver<SharedOutcome>,
}

/// The body the worker runs for one command; swappable in tests. The
/// activity is the command's own, labelled with its name and live until the
/// body returns: the body writes its stages into the text or begins nested
/// activities on the hub.
pub(crate) type Executor =
    dyn Fn(AppState, Command, Activity) -> BoxFuture<'static, Outcome> + Send + Sync;

/// The gateway's command queue: one shared pending deque, one worker task,
/// and the in-process status the tray and routes read.
#[derive(Debug)]
pub(crate) struct CommandQueue {
    state: Arc<Mutex<QueueState>>,
    /// Wakes the parked worker when a command lands or the queue closes.
    /// Payload-free: the worker re-reads the pending deque on each wake, so
    /// one shared `Notify` serves both enqueue and shutdown.
    notify: Arc<tokio::sync::Notify>,
    /// Set once the one worker has been spawned; clones share it, so only
    /// the first [`CommandQueue::spawn_worker`] call across every clone
    /// starts a worker.
    worker_taken: Arc<AtomicBool>,
    hub: Arc<ProgressHub>,
}

impl Clone for CommandQueue {
    fn clone(&self) -> CommandQueue {
        CommandQueue {
            state: Arc::clone(&self.state),
            notify: Arc::clone(&self.notify),
            worker_taken: Arc::clone(&self.worker_taken),
            hub: Arc::clone(&self.hub),
        }
    }
}

/// Runs one command to its end under its activity, which drops with the
/// body's return on every path.
async fn run_command(state: AppState, command: Command, activity: Activity) -> Outcome {
    match command {
        Command::LoadProfile { name, token } => {
            crate::boot_load::run(&state, name, activity, &token).await
        }
        Command::ApplyConfig { snapshot, token } => {
            apply::apply_config(&state, snapshot, token, activity).await
        }
        Command::ProvisionModel {
            name,
            source,
            token,
        } => provision_model(&state, &name, &source, token, activity).await,
        Command::UnloadModel { name } => unload_model(&state, &name, activity).await,
    }
}

/// The `ProvisionModel` body: download and verify one model into the
/// artifact store, off the async executor. The activity moves into the
/// blocking task, which writes the download and verify stages into it and
/// drops it when the store returns.
#[cfg(feature = "local")]
async fn provision_model(
    state: &AppState,
    name: &str,
    source: &str,
    token: CancellationToken,
    activity: Activity,
) -> Outcome {
    let cache_dir = state.cache_dir().await;
    let source = source.to_owned();
    let label = format!("provision-model: {name}");
    let worker_token = token.clone();
    let result = blocking(move || {
        let root = crate::local::resolve_cache_root(cache_dir.as_deref())?;
        let store = crate::local::artifacts::ArtifactStore::new(root)?;
        store.ensure_model_with_cancellation(&source, None, Some(&activity), Some(&worker_token))
    })
    .await?;
    match result {
        Ok(_path) => Ok(format!("provisioned {name}")),
        Err(_) if token.is_cancelled() => Err(GatewayError::CommandCancelled(label)),
        Err(error) => Err(GatewayError::cache(error)),
    }
}

/// The headless `ProvisionModel` body: local inference is compiled out.
#[cfg(not(feature = "local"))]
async fn provision_model(
    _state: &AppState,
    _name: &str,
    _source: &str,
    _token: CancellationToken,
    _activity: Activity,
) -> Outcome {
    Err(GatewayError::switch_failed(
        "provision-model",
        std::io::Error::other(crate::LOCAL_MODELS_UNSUPPORTED),
    ))
}

/// The `UnloadModel` body: drop the model from the routing table, then tear
/// down its child off the async executor.
#[cfg(feature = "local")]
async fn unload_model(state: &AppState, name: &str, activity: Activity) -> Outcome {
    // In-flight requests holding the old table entry keep their connection;
    // the teardown below ends the child under them, which is what the
    // caller asked for.
    let model = {
        let mut live = state.live.write().await;
        let Some(model) = live.local.unload_model(name) else {
            return Err(GatewayError::UnknownModel(name.to_owned()));
        };
        live.routing = Arc::new(live.routing.without(name));
        model
    };
    activity.set_text(format!("Stopping {name}"));
    blocking(move || model.endpoint.upstream.shutdown())
        .await?
        .map_err(|error| GatewayError::switch_failed("unload-model", error))?;
    Ok(format!("unloaded {name}"))
}

/// The headless `UnloadModel` body: no local runtime exists to hold models.
#[cfg(not(feature = "local"))]
async fn unload_model(_state: &AppState, name: &str, _activity: Activity) -> Outcome {
    Err(GatewayError::UnknownModel(name.to_owned()))
}

#[cfg(test)]
mod tests;
