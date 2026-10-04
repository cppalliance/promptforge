//! The [`CommandQueue`]'s operations and the worker loop that drains it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Instant;

use gateway_progress::{Activity, ProgressHub};
use tokio::sync::oneshot;

#[cfg(test)]
use super::ExecutorOverride;
use super::{
    ActiveEntry, Command, CommandQueue, CommandStatus, CommandSummary, DebounceKey, Enqueued,
    Executor, Outcome, PendingEntry, QueueState, run_command,
};
use crate::AppState;
use crate::error::GatewayError;

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
