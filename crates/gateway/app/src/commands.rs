//! The command queue: serialized, debounced, cancellable gateway commands.
//!
//! Everything slow the gateway does - the boot-time profile load, config
//! applies, model provisioning and unloads - runs as a [`Command`] on one
//! worker task draining a shared pending deque FIFO, so downloads never
//! fight each other for bandwidth and the listener stays live while they
//! run.
//! The worker begins one [`Activity`] on the process hub per command it
//! runs, labelled with the command's name, and hands it to the body, which
//! writes its stages into the text; every command carries a
//! [`CancellationToken`] the worker honors at chunk and phase boundaries.
//! The in-process status ([`CommandQueue::active_command`] and
//! [`CommandQueue::pending_commands`]) feeds the tray and the admin routes.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use futures_util::future::BoxFuture;
use gateway_config::ProfileName;
use shared_progress::{Activity, ProgressHub};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

use crate::AppState;
use crate::config_apply::ApplySnapshot;
use crate::error::GatewayError;

/// The `ApplyConfig` command's display name: the status bar and tray show
/// it, and the apply's cancellation error names it.
pub(crate) const APPLY_CONFIG_LABEL: &str = "apply-config";

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
    /// this command does not carry; that arrives with the command's first
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

    /// The token the worker honors, when the command carries one.
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
/// text, not part of this readout.
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
    pub(crate) entry: u64,
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

impl CommandQueue {
    /// A queue over `hub`, with no worker yet; [`CommandQueue::spawn_worker`]
    /// starts the drain.
    pub(crate) fn new(hub: Arc<ProgressHub>) -> CommandQueue {
        CommandQueue {
            state: Arc::new(Mutex::new(QueueState::default())),
            notify: Arc::new(tokio::sync::Notify::new()),
            worker_taken: Arc::new(AtomicBool::new(false)),
            hub,
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        // A lock poisoned by a panicking peer recovers the value rather than
        // wedging the queue.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Enqueues `command`, applying the debounce: a `LoadProfile`,
    /// `ApplyConfig`, or `ProvisionModel` duplicating the pending or active
    /// command attaches to it and shares its outcome. Everything else
    /// queues FIFO; no command displaces or cancels another. `UnloadModel`
    /// is never debounced.
    pub(crate) fn enqueue(&self, command: Command) -> Enqueued {
        let (waiter_tx, waiter_rx) = oneshot::channel();
        let mut state = self.lock();
        if state.closed {
            // The queue is shut down: the command never runs, so settle its
            // waiter immediately under an entry id nothing else shares.
            let entry = state.next_id;
            state.next_id += 1;
            let _ = waiter_tx.send(Arc::new(Err(GatewayError::CommandCancelled(
                command.label(),
            ))));
            return Enqueued {
                entry,
                outcome: waiter_rx,
            };
        }
        let key = command.debounce_key();
        if let Some(key) = &key {
            // A duplicate of the active command attaches to it.
            if let Some(active) = &mut state.active
                && active.key.as_ref() == Some(key)
            {
                active.waiters.push(waiter_tx);
                return Enqueued {
                    entry: active.id,
                    outcome: waiter_rx,
                };
            }
            // A duplicate of a pending command attaches to it.
            if let Some(pending) = state
                .pending
                .iter_mut()
                .find(|entry| entry.key.as_ref() == Some(key))
            {
                let entry = pending.id;
                pending.waiters.push(waiter_tx);
                return Enqueued {
                    entry,
                    outcome: waiter_rx,
                };
            }
        }
        let id = state.next_id;
        state.next_id += 1;
        let label = command.label();
        state.pending.push_back(PendingEntry {
            id,
            command,
            key,
            label,
            queued_at: Instant::now(),
            waiters: vec![waiter_tx],
        });
        // Wake after releasing the lock: the permit is stored, so a worker
        // that re-checks the deque before sleeping cannot miss it.
        drop(state);
        self.notify.notify_one();
        Enqueued {
            entry: id,
            outcome: waiter_rx,
        }
    }

    /// The running command's status, or `None` when the worker is idle.
    pub(crate) fn active_command(&self) -> Option<CommandStatus> {
        let state = self.lock();
        let active = state.active.as_ref()?;
        Some(CommandStatus {
            name: active.label.clone(),
            started_at: active.started_at,
        })
    }

    /// The waiting commands, oldest first.
    pub(crate) fn pending_commands(&self) -> Vec<CommandSummary> {
        self.lock()
            .pending
            .iter()
            .map(|entry| CommandSummary {
                name: entry.label.clone(),
                queued_at: entry.queued_at,
            })
            .collect()
    }

    /// How many callers await the active command's outcome, for tests that
    /// must know a debounced duplicate has attached before they act.
    #[cfg(test)]
    pub(crate) fn active_waiters(&self) -> usize {
        self.lock()
            .active
            .as_ref()
            .map_or(0, |active| active.waiters.len())
    }

    /// Fires the active command's cancellation token. Returns whether a
    /// command was active. The command settles as cancelled once its body
    /// reaches the next chunk or phase boundary.
    pub(crate) fn cancel_active(&self) -> bool {
        let state = self.lock();
        let Some(active) = &state.active else {
            return false;
        };
        if let Some(token) = &active.token {
            token.cancel();
        }
        true
    }

    /// Cancels the `ApplyConfig` command, wherever it sits: fires the active
    /// one's token, or removes the pending one and settles its waiters as
    /// cancelled. Returns whether an apply existed. Revert calls this before
    /// deleting shadows, so an apply's deferred commit can never write its
    /// snapshot over files the user just reverted.
    pub(crate) fn cancel_apply(&self) -> bool {
        let (fired, removed) = {
            let mut state = self.lock();
            let fired = match &state.active {
                Some(active) if active.key == Some(DebounceKey::Apply) => {
                    if let Some(token) = &active.token {
                        token.cancel();
                    }
                    true
                }
                _ => false,
            };
            let position = state
                .pending
                .iter()
                .position(|entry| entry.key == Some(DebounceKey::Apply));
            (
                fired,
                position.and_then(|index| state.pending.remove(index)),
            )
        };
        let Some(entry) = removed else {
            return fired;
        };
        let label = entry.label.clone();
        entry.settle(Err(GatewayError::CommandCancelled(label)));
        true
    }

    /// Removes the waiting command at `index`, settling its waiters as
    /// cancelled. Returns whether an entry was removed. The deque is the
    /// sole owner, so the removed command never reaches the worker.
    pub(crate) fn cancel_pending(&self, index: usize) -> bool {
        let entry = self.lock().pending.remove(index);
        let Some(entry) = entry else {
            return false;
        };
        let label = entry.label.clone();
        entry.settle(Err(GatewayError::CommandCancelled(label)));
        true
    }

    /// Closes the queue: no new commands start, the active one is cancelled,
    /// every pending one settles as cancelled, and the worker wakes and exits
    /// once its current command settles.
    pub(crate) fn shutdown(&self) {
        let pending: Vec<PendingEntry> = {
            let mut state = self.lock();
            state.closed = true;
            if let Some(token) = state
                .active
                .as_ref()
                .and_then(|active| active.token.as_ref())
            {
                token.cancel();
            }
            state.pending.drain(..).collect()
        };
        for entry in pending {
            let label = entry.label.clone();
            entry.settle(Err(GatewayError::CommandCancelled(label)));
        }
        // The notify stores a permit, so a worker parked on the empty deque
        // wakes and observes `closed`.
        self.notify.notify_one();
    }

    /// Spawns the worker task draining the queue, running the production
    /// command bodies. Returns `None` when a worker was already taken.
    pub(crate) fn spawn_worker(&self, state: &AppState) -> Option<tokio::task::JoinHandle<()>> {
        #[cfg(test)]
        if let Some(executor) = self.lock().executor_override.as_ref() {
            return self.spawn_worker_with(state, Arc::clone(&executor.0));
        }
        self.spawn_worker_with(
            state,
            Arc::new(|state, command, activity| Box::pin(run_command(state, command, activity))),
        )
    }

    /// Test hook: workers spawned after this call run `executor` as the
    /// command body instead of the production commands.
    #[cfg(test)]
    pub(crate) fn override_executor(&self, executor: Arc<Executor>) {
        self.lock().executor_override = Some(ExecutorOverride(executor));
    }

    /// [`Self::spawn_worker`] with the command body injected, so a test can
    /// drive the worker over a stub.
    pub(crate) fn spawn_worker_with(
        &self,
        state: &AppState,
        executor: Arc<Executor>,
    ) -> Option<tokio::task::JoinHandle<()>> {
        if self.worker_taken.swap(true, Ordering::SeqCst) {
            return None;
        }
        let queue = self.clone();
        let state = state.clone();
        Some(tokio::spawn(worker_loop(queue, state, executor)))
    }

    /// Pops the next pending entry and installs it as the active command in
    /// one critical section. A shutdown landing between a separate pop and
    /// activate would drain a deque the entry already left and cancel only
    /// the previous active token, letting the popped command start
    /// uncancelled after close. Holding the lock across both means shutdown
    /// either runs first (the worker observes `closed` and exits) or after
    /// (it cancels this entry's token as the active command).
    fn begin_next(&self) -> BeginNext {
        let mut state = self.lock();
        if state.closed {
            // Shutdown drained the deque under this same lock, so a closed
            // queue has nothing left to run.
            return BeginNext::Exit;
        }
        let Some(entry) = state.pending.pop_front() else {
            return BeginNext::Wait;
        };
        let token = entry.command.token();
        let id = entry.id;
        // The command's activity begins here, labelled with its name, so
        // the hub is busy from the first instant the worker owns it; the
        // body refines the text and its return drops the guard.
        let activity = self.hub.begin(entry.label.clone());
        tracing::info!(command = %entry.label, "command started");
        state.active = Some(ActiveEntry {
            id,
            key: entry.key,
            label: entry.label,
            started_at: Instant::now(),
            token,
            waiters: entry.waiters,
        });
        BeginNext::Run(id, entry.command, activity)
    }

    /// Clears the active command and settles its waiters, logging the
    /// outcome: the boot load has no waiter, so the log is where its
    /// failure surfaces.
    fn finish(&self, id: u64, outcome: Outcome) {
        let active = {
            let mut state = self.lock();
            match state.active.as_ref() {
                Some(active) if active.id == id => state.active.take(),
                // The worker settles only what it began; a mismatch is a bug,
                // but dropping the outcome must not wedge the queue.
                _ => None,
            }
        };
        let Some(active) = active else {
            return;
        };
        let label = active.label;
        match &outcome {
            Ok(summary) => tracing::info!(command = %label, "command finished: {summary}"),
            Err(GatewayError::CommandCancelled(_)) => {
                tracing::info!(command = %label, "command cancelled");
            }
            Err(error) => tracing::error!(command = %label, "command failed: {error}"),
        }
        let outcome = Arc::new(outcome);
        for waiter in active.waiters {
            let _ = waiter.send(Arc::clone(&outcome));
        }
    }
}

/// What the worker does next, decided by [`CommandQueue::begin_next`] in
/// one critical section so a shutdown cannot slip between the pop and the
/// activation.
enum BeginNext {
    /// Runs this command under its activity; it is installed as the active
    /// entry.
    Run(u64, Command, Activity),
    /// The deque is empty; park until notified.
    Wait,
    /// The queue is closed; exit.
    Exit,
}

/// The worker loop: one command at a time, FIFO off the shared pending
/// deque, until the queue shuts down and the deque is empty.
async fn worker_loop(queue: CommandQueue, state: AppState, executor: Arc<Executor>) {
    loop {
        // Lost-wakeup-safe sleep: `enable` registers interest before the
        // deque is re-checked, so an enqueue or shutdown landing between
        // the check and the `await` still wakes this worker.
        let notified = queue.notify.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        let (id, command, activity) = match queue.begin_next() {
            BeginNext::Run(id, command, activity) => (id, command, activity),
            BeginNext::Wait => {
                notified.await;
                continue;
            }
            BeginNext::Exit => break,
        };
        let outcome = executor(state.clone(), command, activity).await;
        queue.finish(id, outcome);
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
            crate::config_apply::apply_config(&state, snapshot, token, activity).await
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
    let result = tokio::task::spawn_blocking(move || {
        let root = crate::local::resolve_cache_root(cache_dir.as_deref())?;
        let store = crate::local::artifacts::ArtifactStore::new(root)?;
        store.ensure_model_with_cancellation(&source, None, Some(&activity), Some(&worker_token))
    })
    .await;
    match result {
        Ok(Ok(_path)) => Ok(format!("provisioned {name}")),
        Ok(Err(_)) if token.is_cancelled() => Err(GatewayError::CommandCancelled(label)),
        Ok(Err(error)) => Err(GatewayError::cache(error)),
        Err(join) => Err(GatewayError::cache(join)),
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
    let result = tokio::task::spawn_blocking(move || model.endpoint.upstream.shutdown()).await;
    match result {
        Ok(Ok(())) => Ok(format!("unloaded {name}")),
        Ok(Err(error)) => Err(GatewayError::switch_failed("unload-model", error)),
        Err(join) => Err(GatewayError::switch_failed("unload-model", join)),
    }
}

/// The headless `UnloadModel` body: no local runtime exists to hold models.
#[cfg(not(feature = "local"))]
async fn unload_model(_state: &AppState, name: &str, _activity: Activity) -> Outcome {
    Err(GatewayError::UnknownModel(name.to_owned()))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use gateway_config::Config;

    use super::*;
    use crate::test_support::{app_state, parking_executor, wait_until};

    fn queue() -> CommandQueue {
        CommandQueue::new(Arc::new(ProgressHub::new()))
    }

    fn load_profile(name: &str) -> Command {
        Command::load_profile(
            ProfileName::parse(name).expect("profile name"),
            CancellationToken::new(),
        )
    }

    fn provision(name: &str) -> Command {
        Command::ProvisionModel {
            name: name.to_owned(),
            source: format!("/models/{name}.gguf"),
            token: CancellationToken::new(),
        }
    }

    fn unload(name: &str) -> Command {
        Command::UnloadModel {
            name: name.to_owned(),
        }
    }

    /// An `ApplyConfig` over a one-profile config with nothing captured; the
    /// stub executors never read the snapshot.
    fn apply() -> Command {
        let config = Config::from_toml_str(
            "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"t\"\n\
             [[profile]]\nname = \"alpha\"\nmodels = []\n",
        )
        .expect("config parses");
        Command::ApplyConfig {
            snapshot: ApplySnapshot {
                config: Box::new(config),
                files: Vec::new(),
                restart_required: false,
            },
            token: CancellationToken::new(),
        }
    }

    /// An `AppState` over a minimal config; the stub executors never read it.
    fn state() -> AppState {
        let config = Config::from_toml_str(
            "config-version = 0\n[server]\nbind = \"127.0.0.1:0\"\napi_key = \"t\"\n",
        )
        .expect("config parses");
        app_state(config, None)
    }

    /// Whether the active command is the one labelled `name`.
    fn active_is(queue: &CommandQueue, name: &str) -> bool {
        queue
            .active_command()
            .is_some_and(|status| status.name == name)
    }

    fn pending_names(queue: &CommandQueue) -> Vec<String> {
        queue
            .pending_commands()
            .into_iter()
            .map(|entry| entry.name)
            .collect()
    }

    #[test]
    fn a_duplicate_load_profile_for_the_same_profile_is_dropped() {
        let queue = queue();
        let first = queue.enqueue(load_profile("alpha"));
        let second = queue.enqueue(load_profile("alpha"));

        let pending = queue.pending_commands();
        assert_eq!(pending.len(), 1, "the duplicate never enters the queue");
        assert_eq!(
            first.entry, second.entry,
            "the duplicate attaches to the pending command's entry"
        );
        assert!(queue.active_command().is_none());
    }

    #[test]
    fn an_apply_attaches_to_a_pending_apply() {
        let queue = queue();
        let first = queue.enqueue(apply());
        let second = queue.enqueue(apply());

        assert_eq!(
            pending_names(&queue),
            ["apply-config"],
            "one apply is pending; the duplicate never enters the queue"
        );
        assert_eq!(
            first.entry, second.entry,
            "the duplicate attaches to the pending apply's entry"
        );
    }

    #[test]
    fn a_load_profile_queues_behind_a_pending_apply_without_replacing_it() {
        let queue = queue();
        let applied = queue.enqueue(apply());
        let _switch = queue.enqueue(load_profile("alpha"));

        assert_eq!(
            pending_names(&queue),
            ["apply-config", "load-profile: alpha"],
            "the switch queues FIFO behind the apply"
        );
        drop(applied);
    }

    /// An apply enqueued while the boot load is active queues behind it
    /// FIFO: the boot load keeps running, its waiter is not settled, and
    /// the apply starts only once the boot load has settled.
    #[tokio::test]
    async fn an_apply_during_the_active_boot_load_queues_behind_it_without_cancelling_it() {
        let state = state();
        let queue = state.commands.clone();
        let _worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");

        let mut boot = queue.enqueue(load_profile("alpha"));
        wait_until("the boot load to go active", || {
            active_is(&queue, "load-profile: alpha")
        })
        .await;
        let applied = queue.enqueue(apply());
        // Give the worker every chance to act on a cancellation that must
        // not have happened.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            active_is(&queue, "load-profile: alpha"),
            "the boot load keeps running under the queued apply"
        );
        assert_eq!(pending_names(&queue), ["apply-config"]);
        assert!(
            matches!(
                boot.outcome.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "the boot load's waiter is not settled by the apply"
        );

        assert!(queue.cancel_active(), "the boot load is cancelled by hand");
        let outcome = boot.outcome.await.expect("the boot load settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        wait_until("the apply to go active", || {
            active_is(&queue, "apply-config")
        })
        .await;
        assert!(queue.cancel_active());
        let outcome = applied.outcome.await.expect("the apply settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        queue.shutdown();
    }

    #[tokio::test]
    async fn a_load_profile_queues_behind_an_active_apply_without_cancelling_it() {
        let state = state();
        let queue = state.commands.clone();
        let _worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");

        let mut applied = queue.enqueue(apply());
        wait_until("the apply to go active", || {
            active_is(&queue, "apply-config")
        })
        .await;
        let switch = queue.enqueue(load_profile("alpha"));
        // Give the worker every chance to act on a cancellation that must
        // not have happened.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }

        assert!(
            active_is(&queue, "apply-config"),
            "the apply keeps running under the queued switch"
        );
        assert_eq!(pending_names(&queue), ["load-profile: alpha"]);
        assert!(
            matches!(
                applied.outcome.try_recv(),
                Err(oneshot::error::TryRecvError::Empty)
            ),
            "the apply's waiter is not settled by the switch"
        );

        assert!(queue.cancel_active());
        let outcome = applied.outcome.await.expect("the apply settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        wait_until("alpha to go active", || {
            active_is(&queue, "load-profile: alpha")
        })
        .await;
        assert!(queue.cancel_active());
        let outcome = switch.outcome.await.expect("alpha settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        queue.shutdown();
    }

    #[tokio::test]
    async fn cancel_apply_removes_a_pending_apply_and_fires_an_active_one() {
        // Pending: no worker, so the apply waits in the queue.
        let queue = queue();
        assert!(
            !queue.cancel_apply(),
            "an idle queue has no apply to cancel"
        );
        let _boot = queue.enqueue(load_profile("alpha"));
        assert!(
            !queue.cancel_apply(),
            "a pending boot load is not an apply and stays put"
        );
        assert_eq!(pending_names(&queue), ["load-profile: alpha"]);
        let pending = queue.enqueue(apply());
        assert!(queue.cancel_apply(), "the pending apply is removed");
        assert_eq!(
            pending_names(&queue),
            ["load-profile: alpha"],
            "only the apply leaves the queue"
        );
        let outcome = pending.outcome.await.expect("the removed apply settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));

        // Active: the parked apply observes its token.
        let state = state();
        let queue = state.commands.clone();
        let _worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        let active = queue.enqueue(apply());
        wait_until("the apply to go active", || {
            active_is(&queue, "apply-config")
        })
        .await;
        assert!(queue.cancel_apply(), "the active apply's token fires");
        let outcome = active.outcome.await.expect("the active apply settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        wait_until("the queue to go idle", || queue.active_command().is_none()).await;
        queue.shutdown();
    }

    #[test]
    fn provision_model_debounces_on_the_model_name() {
        let queue = queue();
        let first = queue.enqueue(provision("m"));
        let duplicate = queue.enqueue(provision("m"));
        let _other = queue.enqueue(provision("n"));

        assert_eq!(
            first.entry, duplicate.entry,
            "a same-model duplicate attaches to the pending command"
        );
        let pending = queue.pending_commands();
        assert_eq!(pending.len(), 2, "distinct models queue independently");
        assert!(
            pending
                .iter()
                .any(|entry| entry.name == "provision-model: m")
        );
        assert!(
            pending
                .iter()
                .any(|entry| entry.name == "provision-model: n")
        );
    }

    #[test]
    fn unload_model_is_never_debounced() {
        let queue = queue();
        let first = queue.enqueue(unload("m"));
        let second = queue.enqueue(unload("m"));

        assert_eq!(queue.pending_commands().len(), 2);
        assert_ne!(first.entry, second.entry, "each unload keeps its own entry");
    }

    #[tokio::test]
    async fn cancel_pending_removes_the_entry_and_settles_its_waiter() {
        let queue = queue();
        let _switch = queue.enqueue(load_profile("alpha"));
        let provisioned = queue.enqueue(provision("m"));

        assert!(!queue.cancel_pending(5), "out of range is a no-op");
        assert!(queue.cancel_pending(1), "the provision entry is removed");
        let pending = queue.pending_commands();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].name, "load-profile: alpha");
        let outcome = provisioned.outcome.await.expect("the waiter settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
    }

    #[tokio::test]
    async fn the_worker_drains_the_queue_in_fifo_order() {
        let state = state();
        let queue = state.commands.clone();
        let order = Arc::new(Mutex::new(Vec::new()));
        let executor: Arc<Executor> = Arc::new({
            let order = Arc::clone(&order);
            move |_state, command: Command, _activity| {
                let order = Arc::clone(&order);
                Box::pin(async move {
                    let label = command.label();
                    order
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(label.clone());
                    Ok(label)
                }) as BoxFuture<'static, Outcome>
            }
        });
        let worker = state
            .commands
            .spawn_worker_with(&state, executor)
            .expect("worker spawns");

        let a = queue.enqueue(unload("a"));
        let b = queue.enqueue(unload("b"));
        let c = queue.enqueue(unload("c"));
        for handle in [a, b, c] {
            let outcome = handle.outcome.await.expect("each command settles");
            assert!(outcome.is_ok(), "the stub body succeeds: {outcome:?}");
        }
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");

        assert_eq!(
            order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            ["unload-model: a", "unload-model: b", "unload-model: c"],
            "one worker drains the channel in FIFO order"
        );
        assert!(queue.active_command().is_none(), "the queue is idle");
    }

    #[tokio::test]
    async fn cancel_active_fires_the_active_commands_token() {
        let state = state();
        let queue = state.commands.clone();
        let _worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");

        assert!(!queue.cancel_active(), "no active command to cancel");
        let handle = queue.enqueue(load_profile("alpha"));
        wait_until("the command to go active", || {
            queue.active_command().is_some()
        })
        .await;
        assert_eq!(
            queue.active_command().expect("active").name,
            "load-profile: alpha"
        );

        assert!(queue.cancel_active());
        let outcome = handle.outcome.await.expect("the command settles");
        assert!(
            matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
            "the parked command observes its token: {outcome:?}"
        );
        wait_until("the queue to go idle", || queue.active_command().is_none()).await;
        queue.shutdown();
    }

    /// The worker begins the command's activity under its label, the body
    /// refines the text, and the hub falls idle once the body returns.
    #[tokio::test]
    async fn the_active_command_drives_the_hubs_busy_text() {
        let state = state();
        let queue = state.commands.clone();
        let hub = Arc::clone(&state.hub);
        assert!(!hub.current().busy, "a pending command is not yet busy");
        let _pending = queue.enqueue(load_profile("alpha"));
        assert!(
            !hub.current().busy,
            "a queued command that has not started reports nothing"
        );

        // The stub writes a stage into the activity, then parks until
        // cancelled.
        let executor: Arc<Executor> = Arc::new(|_state, command, activity| {
            Box::pin(async move {
                activity.set_text("Downloading qwen 45%");
                let label = command.label();
                let token = command.token().expect("a load command carries a token");
                token.cancelled().await;
                Err(GatewayError::CommandCancelled(label))
            }) as BoxFuture<'static, Outcome>
        });
        let _worker = state
            .commands
            .spawn_worker_with(&state, executor)
            .expect("worker spawns");

        wait_until("the command to write its stage", || {
            hub.current().text == "Downloading qwen 45%"
        })
        .await;
        assert!(hub.current().busy, "a running command is busy");
        queue.cancel_active();
        wait_until("the queue to go idle", || queue.active_command().is_none()).await;
        assert_eq!(
            hub.current(),
            gateway_api_types::Progress::default(),
            "the body's return drops the activity and the hub falls idle"
        );
        queue.shutdown();
    }

    #[tokio::test]
    async fn a_pre_cancelled_load_profile_settles_as_cancelled_without_switching() {
        let state = state();
        let token = CancellationToken::new();
        token.cancel();
        let activity = state.hub.begin("test");
        let outcome = run_command(
            state.clone(),
            Command::load_profile(ProfileName::parse("alpha").expect("profile name"), token),
            activity,
        )
        .await;
        assert!(
            matches!(outcome, Err(GatewayError::CommandCancelled(_))),
            "a fired token stops the load before any phase: {outcome:?}"
        );
        assert!(
            state.live.read().await.loading.is_empty(),
            "the cancelled load never touched the live state"
        );
    }

    #[tokio::test]
    async fn an_enqueue_on_a_closed_queue_settles_immediately() {
        let queue = queue();
        queue.shutdown();
        let handle = queue.enqueue(load_profile("alpha"));
        let outcome = handle.outcome.await.expect("settled at enqueue");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        assert!(queue.pending_commands().is_empty());
    }

    /// An executor that records each command's label in run order, then
    /// settles it Ok.
    fn recording_executor(order: &Arc<Mutex<Vec<String>>>) -> Arc<Executor> {
        Arc::new({
            let order = Arc::clone(order);
            move |_state, command: Command, _activity| {
                let order = Arc::clone(&order);
                Box::pin(async move {
                    let label = command.label();
                    order
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .push(label.clone());
                    Ok(label)
                }) as BoxFuture<'static, Outcome>
            }
        })
    }

    #[tokio::test]
    async fn the_worker_drains_more_than_thirty_two_commands_in_fifo_order() {
        let state = state();
        let queue = state.commands.clone();
        let order = Arc::new(Mutex::new(Vec::new()));
        // The pending deque is unbounded: every command waits for the one
        // worker, well past the old channel's capacity.
        let handles: Vec<Enqueued> = (0..40)
            .map(|index| queue.enqueue(unload(&format!("m{index}"))))
            .collect();
        let worker = state
            .commands
            .spawn_worker_with(&state, recording_executor(&order))
            .expect("worker spawns");

        for handle in handles {
            let outcome = handle.outcome.await.expect("each command settles");
            assert!(outcome.is_ok(), "no command is rejected: {outcome:?}");
        }
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");

        let expected: Vec<String> = (0..40)
            .map(|index| format!("unload-model: m{index}"))
            .collect();
        assert_eq!(
            order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            expected.as_slice(),
            "the deque holds every command and the worker drains them FIFO"
        );
    }

    #[tokio::test]
    async fn the_queue_spawns_at_most_one_worker() {
        let state = state();
        let queue = state.commands.clone();
        let first = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("the first worker spawns");
        assert!(
            state
                .commands
                .spawn_worker_with(&state, parking_executor())
                .is_none(),
            "a second worker is refused"
        );
        queue.shutdown();
        first.await.expect("the worker exits on shutdown");
    }

    #[tokio::test]
    async fn shutdown_on_an_idle_queue_stops_the_parked_worker() {
        let state = state();
        let queue = state.commands.clone();
        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        // Let the worker park on the empty deque.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        queue.shutdown();
        tokio::time::timeout(Duration::from_secs(10), worker)
            .await
            .expect("the parked worker wakes and exits")
            .expect("the worker task joins");
    }

    #[tokio::test]
    async fn an_enqueue_wakes_the_parked_worker() {
        let state = state();
        let queue = state.commands.clone();
        let order = Arc::new(Mutex::new(Vec::new()));
        let worker = state
            .commands
            .spawn_worker_with(&state, recording_executor(&order))
            .expect("worker spawns");
        // Let the worker park before the command lands.
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let handle = queue.enqueue(unload("m"));
        let outcome = handle.outcome.await.expect("the command settles");
        assert!(
            outcome.is_ok(),
            "the parked worker woke and ran it: {outcome:?}"
        );
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");
    }

    #[tokio::test]
    async fn an_enqueue_racing_shutdown_around_the_workers_sleep_still_settles() {
        let state = state();
        let queue = state.commands.clone();
        let worker = state
            .commands
            .spawn_worker_with(&state, parking_executor())
            .expect("worker spawns");
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
        let handle = queue.enqueue(load_profile("alpha"));
        queue.shutdown();
        let outcome = handle.outcome.await.expect("the command settles");
        assert!(
            matches!(&*outcome, Err(GatewayError::CommandCancelled(_))),
            "whether drained or started, shutdown settles it as cancelled: {outcome:?}"
        );
        worker.await.expect("the worker exits on shutdown");
    }

    #[tokio::test]
    async fn a_cancelled_pending_command_never_reaches_the_worker() {
        let state = state();
        let queue = state.commands.clone();
        let order = Arc::new(Mutex::new(Vec::new()));
        let cancelled = queue.enqueue(unload("a"));
        let kept = queue.enqueue(unload("b"));
        assert!(queue.cancel_pending(0), "the first entry leaves the deque");
        let worker = state
            .commands
            .spawn_worker_with(&state, recording_executor(&order))
            .expect("worker spawns");

        let outcome = cancelled
            .outcome
            .await
            .expect("the cancelled command settles");
        assert!(matches!(&*outcome, Err(GatewayError::CommandCancelled(_))));
        let outcome = kept.outcome.await.expect("the kept command settles");
        assert!(outcome.is_ok(), "the kept command runs: {outcome:?}");
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");
        assert_eq!(
            order
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_slice(),
            ["unload-model: b"],
            "the worker never saw the cancelled entry"
        );
    }

    #[tokio::test]
    async fn an_unload_of_a_model_the_runtime_does_not_hold_is_unknown_model() {
        let state = state();
        let activity = state.hub.begin("test");
        let outcome = run_command(state.clone(), unload("ghost"), activity).await;
        assert!(
            matches!(&outcome, Err(GatewayError::UnknownModel(name)) if name == "ghost"),
            "an unload miss is UnknownModel, not a queue error: {outcome:?}"
        );
    }

    /// A two-profile remote catalog on an endpoint nothing listens on:
    /// remote routing is static, so the boot load succeeds without network.
    #[cfg(feature = "stt")]
    fn speech_state() -> AppState {
        let catalog = Config::from_toml_str(
            "config-version = 0\n\
             [server]\nbind = \"127.0.0.1:0\"\napi_key = \"test-token\"\n\
             [[endpoint]]\nid = \"e\"\nprotocol = \"openai\"\nbase_url = \"http://127.0.0.1:9\"\napi_key = \"\"\n\
             [[model]]\nname = \"alpha-model\"\ndescription = \"a\"\ncontext = 1024\nupstream = \"a\"\nendpoints = [\"e\"]\n\
             [[model]]\nname = \"beta-model\"\ndescription = \"b\"\ncontext = 1024\nupstream = \"b\"\nendpoints = [\"e\"]\n\
             [[profile]]\nname = \"alpha\"\nmodels = []\n\
             [[profile]]\nname = \"beta\"\nmodels = []\n",
        )
        .expect("catalog parses");
        crate::test_support::boot_state(catalog)
    }

    /// The boot load runs its local half first, then makes the process's
    /// one guarded STT load attempt.
    #[cfg(feature = "stt")]
    #[tokio::test]
    async fn the_boot_command_loads_speech_after_its_local_half() {
        use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

        let mut state = speech_state();
        crate::test_support::arm_boot_speech(
            &mut state,
            ScriptedModelFactory::new(ScriptedDecoder::new()),
        );
        let activity = state.hub.begin("test");
        let outcome = run_command(state.clone(), load_profile("alpha"), activity).await;

        assert_eq!(
            outcome.as_deref().ok(),
            Some("alpha"),
            "the boot command settles with the profile: {outcome:?}"
        );
        assert!(
            state.live.read().await.routing.model("alpha-model").is_ok(),
            "the remote table the runner published keeps serving"
        );
        assert!(
            state.speech.status().ready(),
            "the boot command's guarded load published the speech runtime"
        );
        assert_eq!(
            state
                .speech
                .models()
                .iter()
                .map(gateway_stt::SpeechModelInfo::name)
                .collect::<Vec<_>>(),
            ["scripted-interim"]
        );
        state.speech.shutdown();
    }

    /// A duplicate attaching to the pending boot command shares its outcome;
    /// the single load attempt runs once for both waiters.
    #[cfg(feature = "stt")]
    #[tokio::test]
    async fn a_duplicate_attached_to_the_boot_command_shares_the_single_speech_load() {
        use gateway_stt::test_fixtures::{ScriptedDecoder, ScriptedModelFactory};

        let mut state = speech_state();
        crate::test_support::arm_boot_speech(
            &mut state,
            ScriptedModelFactory::new(ScriptedDecoder::new()),
        );
        let queue = state.commands.clone();
        // Both enqueue before the worker spawns, so the attach cannot race
        // the drain.
        let boot_handle = queue.enqueue(load_profile("alpha"));
        let attached = queue.enqueue(load_profile("alpha"));
        assert_eq!(
            boot_handle.entry, attached.entry,
            "the duplicate attaches to the boot command"
        );
        let worker = state.commands.spawn_worker(&state).expect("worker spawns");

        let outcome = boot_handle.outcome.await.expect("the boot command settles");
        assert!(outcome.is_ok(), "the boot command succeeds: {outcome:?}");
        let outcome = attached.outcome.await.expect("the attached waiter settles");
        assert!(
            outcome.is_ok(),
            "the attached duplicate shares the outcome: {outcome:?}"
        );
        assert!(
            state.speech.status().ready(),
            "the one guarded load published the runtime for both waiters"
        );
        queue.shutdown();
        worker.await.expect("the worker exits on shutdown");
        state.speech.shutdown();
    }
}
