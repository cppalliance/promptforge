//! The runs this process has started: admission, the reply deadline, and
//! collecting a run that outlived the call which asked for it.
//!
//! Cursor's remote calls fail at about 300 seconds and no progress notification
//! extends that clock, so a prompt that runs longer than the client will wait
//! must not take its work down with the call. A call therefore blocks for at
//! most `reply_deadline`; past that the caller is handed a `running` result
//! naming its `run_id`, and `check_run` collects the outcome afterwards.
//!
//! A run does not belong to the call that started it. [`RunRegistry::launch`]
//! registers the run and, in the same step, puts it under a supervisor task
//! that owns its join handle for the whole of its life. The call that awaits
//! the run holds nothing but a receiver, so a call that is cancelled or
//! disconnects cannot detach the run: dropping its wait asks the registry to
//! cancel the run, the run observes the cancellation and stops, and the
//! supervisor records the terminal result either way. A backgrounded run
//! therefore always reaches a terminal record - a panic or an abort becomes a
//! `failed` result
//! saying the run did not finish, and a cancellation becomes the failure the
//! core reports - which is what keeps such a record collectable and, in time,
//! evictable rather than `running` for the life of the process.
//!
//! Registration is atomic and rejects a duplicate id, so a run id is never
//! observable without the supervisor that will drive it to a terminal state,
//! and a second run can never overwrite a live or retained record. The terminal
//! transition is first-write-wins: whatever records the run first stands, so a
//! late frame cannot rewrite an outcome already reported.
//!
//! Admission is separate and earlier. There are `max_concurrent_runs` slots and
//! a call waits up to `admission_timeout` for one, after which it is refused
//! with a message naming the wait. A refusal the calling model can retry is
//! better than a queue nobody bounded: every waiting call holds a client
//! connection, and a line long enough to outlast the reply deadline would turn
//! into a crowd of background runs the operator never sized for.
//!
//! A finished record stays readable for `retain_completed` and is then evicted;
//! a running record is never evicted, because the run it belongs to is still
//! live and its result has nowhere else to land. Eviction is a sweep taken on
//! each read and each write rather than a timer task: the map holds one small
//! record per run, and a stale record costs nothing until somebody looks.
//!
//! What the log carries is what an operator cannot otherwise see. A run handed
//! back as `running` is `info`, naming the same id the caller was given, and so
//! is that run reaching its terminal state later, which is the only place its
//! outcome is observable once the call has gone. Between them they are the
//! symptom of a `reply_deadline` set too short. A refusal is `warn`, because it
//! means `max_concurrent_runs` is biting and calls are being turned away.
//! Eviction is `debug`: a record ageing out is bookkeeping, and the run it
//! belonged to was reported when it ended.
//!
//! The map sits behind a [`std::sync::Mutex`], and every method that takes it is
//! synchronous, so no guard can be held across an `.await`. [`RunRegistry::admit`]
//! touches the semaphore and nothing else; [`RunRegistry::settle`] awaits only
//! the run's result channel, never the map.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::collections::hash_map::Entry as MapEntry;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use promptforge_core::CancelHandle;
use tokio::sync::oneshot;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;

use crate::config::ServerConfig;
use crate::result::{NO_TURNS, RunResult};

/// One of the `max_concurrent_runs` slots, held for as long as the run holds it.
///
/// Dropping it is what returns the slot, so a slot outlives the call that took
/// it exactly when the run does.
#[derive(Debug)]
#[must_use = "dropping the slot returns it, so a run that means to hold one must keep it"]
pub(crate) struct RunSlot {
    /// Held and never read: its `Drop` is the whole of its job.
    _permit: OwnedSemaphorePermit,
}

/// What a finished run left behind.
#[derive(Debug)]
struct Finished {
    /// When it finished, which is what the retention window is measured from.
    at: Instant,
    /// What it produced.
    result: RunResult,
}

/// One run, still going or recently finished.
#[derive(Debug)]
struct Record {
    /// The prompt's name, so a running run can be reported without the catalog.
    prompt: String,
    /// When the run started.
    started: Instant,
    /// The controlling clone of the run's cancellation handle, so a run can be
    /// stopped by whatever owns its record even after the call that started it
    /// has gone.
    cancel: CancelHandle,
    /// The outcome, absent while the run is still going.
    finished: Option<Finished>,
}

/// The run id offered to [`RunRegistry::launch`] was already registered.
///
/// A run id is 128 random bits, so a collision does not happen in one process's
/// life; this makes a duplicate representable rather than a silent overwrite of
/// a live or retained record.
#[derive(Debug)]
pub(crate) struct DuplicateRun;

/// Stops a run when the call awaiting it is abandoned.
///
/// The call that started a run holds this while it waits out the reply deadline.
/// If that wait is dropped - the client cancelled the request or disconnected -
/// the guard's `Drop` asks the registry to cancel the run, which signals the
/// controlling clone of its [`CancelHandle`] so the run observes the
/// cancellation and stops rather than being left to burn a slot for the life of
/// the process. Reaching the result or the deadline disarms it: a run that
/// finished needs no cancelling, and one handed back as `running` is meant to
/// keep going.
struct CancelOnDrop {
    registry: Arc<RunRegistry>,
    run_id: String,
    armed: bool,
}

impl CancelOnDrop {
    /// Disarms the guard, so dropping it no longer cancels the run.
    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.registry.cancel(&self.run_id);
        }
    }
}

impl Record {
    /// This run as a caller reads it.
    fn snapshot(&self, run_id: &str) -> RunResult {
        match &self.finished {
            Some(finished) => finished.result.clone(),
            None => RunResult::running(run_id.to_owned(), &self.prompt, elapsed_ms(self.started)),
        }
    }
}

/// Every run this process has started, and the limits that govern them.
///
/// One registry serves the whole server: it hands out the run slots, races each
/// run against the reply deadline, and holds the records `check_run` reads.
#[derive(Debug)]
pub(crate) struct RunRegistry {
    /// The run slots, one permit each.
    admission: Arc<Semaphore>,
    /// How long a call waits for a slot.
    admission_timeout: Duration,
    /// How long a call blocks before the run is left to finish in background.
    reply_deadline: Duration,
    /// How long a finished record stays collectable.
    retain_completed: Duration,
    /// The records, keyed by run id.
    records: Mutex<HashMap<String, Record>>,
}

impl RunRegistry {
    /// Builds a registry over the `[server]` limits.
    #[must_use]
    pub(crate) fn new(server: &ServerConfig) -> RunRegistry {
        RunRegistry {
            admission: Arc::new(Semaphore::new(server.max_concurrent_runs.get())),
            admission_timeout: server.admission_timeout,
            reply_deadline: server.reply_deadline,
            retain_completed: server.retain_completed,
            records: Mutex::new(HashMap::new()),
        }
    }

    /// How long a call waits for a run slot.
    #[must_use]
    pub(crate) fn admission_timeout(&self) -> Duration {
        self.admission_timeout
    }

    /// How long a finished run stays collectable.
    #[must_use]
    pub(crate) fn retain_completed(&self) -> Duration {
        self.retain_completed
    }

    /// Waits up to [`admission_timeout`](Self::admission_timeout) for a run
    /// slot, and answers `None` when none came free.
    ///
    /// A refusal is the caller's to report; the registry does not queue past the
    /// timeout, so a busy server tells its callers to retry rather than
    /// accumulating them.
    pub(crate) async fn admit(&self) -> Option<RunSlot> {
        let admission = Arc::clone(&self.admission);
        // The semaphore is never closed, so an `Err` here and a timeout are the
        // same answer to the caller: there is no slot.
        let waited = tokio::time::timeout(self.admission_timeout, admission.acquire_owned()).await;
        let Ok(Ok(permit)) = waited else {
            tracing::warn!("no run slot came free: refusing the call, which can be retried");
            return None;
        };
        Some(RunSlot { _permit: permit })
    }

    /// Registers a run and puts it under a supervisor that owns its whole life.
    ///
    /// Registration is atomic: the record and the supervisor that will drive it
    /// to a terminal state come into being together and under the same lock, so
    /// no run id is ever observable without an owner, and a run id already in
    /// use is refused with [`DuplicateRun`] rather than overwriting a live or
    /// retained record. `spawn_run` is called only once the record is in place,
    /// so the run cannot record a terminal result against a key that is not yet
    /// there; on a refusal it is dropped uncalled, which returns its admission
    /// slot.
    ///
    /// The supervisor - not this call, and not the caller that awaits the run -
    /// owns the run's join handle for its whole life. A cancellation of the
    /// awaiting call therefore cannot detach the run, and a run that panics or
    /// is aborted still reaches a terminal, evictable record instead of staying
    /// `running` for ever. The supervisor sends the terminal result to the
    /// awaiting call over `result_tx`; a call that has already given up leaves
    /// the result to land in the record alone, which is the point.
    pub(crate) fn launch<F>(
        self: &Arc<Self>,
        run_id: String,
        prompt: String,
        cancel: CancelHandle,
        result_tx: oneshot::Sender<RunResult>,
        spawn_run: F,
    ) -> Result<(), DuplicateRun>
    where
        F: FnOnce() -> JoinHandle<RunResult>,
    {
        let mut records = self.records();
        evict(&mut records, self.retain_completed);
        let task = match records.entry(run_id.clone()) {
            MapEntry::Occupied(_) => return Err(DuplicateRun),
            MapEntry::Vacant(vacant) => {
                vacant.insert(Record {
                    prompt: prompt.clone(),
                    started: Instant::now(),
                    cancel,
                    finished: None,
                });
                // Still under the lock: the record exists before the run task
                // does, so the two are never observed apart.
                spawn_run()
            }
        };
        drop(records);

        let registry = Arc::clone(self);
        // Detached deliberately: this task is the run's owner. It holds the only
        // join handle and records the terminal result however the run ends, so
        // there is nothing to hand it to and nobody who must await it.
        let _supervisor = tokio::spawn(async move {
            let terminal = match task.await {
                Ok(result) => result,
                Err(join) => {
                    // The run task panicked or was aborted and recorded nothing
                    // of its own, so its terminal result is built and logged
                    // here - the only place that can observe an abnormal end.
                    let result = registry.unfinished(&run_id, &prompt, &join);
                    tracing::info!(
                        run_id = %run_id,
                        prompt = %prompt,
                        status = ?result.status(),
                        turns = result.turns(),
                        elapsed_ms = result.elapsed_ms(),
                        "a backgrounded run reached its terminal state"
                    );
                    result
                }
            };
            registry.finished(&run_id, terminal.clone());
            let _ = result_tx.send(terminal);
        });
        Ok(())
    }

    /// Records what a run produced, which is what makes it collectable.
    ///
    /// First-write-wins: a run whose record already carries an outcome keeps it,
    /// so a late frame cannot rewrite a result already reported. A run whose
    /// record is gone was never registered through [`launch`](Self::launch), and
    /// its result is dropped rather than resurrecting a key nothing will read.
    pub(crate) fn finished(&self, run_id: &str, result: RunResult) {
        let mut records = self.records();
        evict(&mut records, self.retain_completed);
        if let Some(record) = records.get_mut(run_id)
            && record.finished.is_none()
        {
            record.finished = Some(Finished {
                at: Instant::now(),
                result,
            });
        }
    }

    /// The run `run_id` names, still going or finished inside its retention
    /// window, or `None` when no such run is known.
    #[must_use]
    pub(crate) fn check(&self, run_id: &str) -> Option<RunResult> {
        let mut records = self.records();
        evict(&mut records, self.retain_completed);
        records.get(run_id).map(|record| record.snapshot(run_id))
    }

    /// Signals the run `run_id` to stop, if it is still known.
    ///
    /// Fires the controlling clone of the run's cancellation handle, which every
    /// clone - including the one installed on the core run - observes, so the
    /// run stops cooperatively and its supervisor records the terminal result.
    /// A run that is unknown or already finished is left alone. The handle is
    /// idempotent, so cancelling twice is harmless.
    pub(crate) fn cancel(&self, run_id: &str) {
        if let Some(record) = self.records().get(run_id) {
            record.cancel.cancel();
        }
    }

    /// Waits up to `reply_deadline` for the run's result, and reports it either
    /// way.
    ///
    /// Inside the deadline the run's own result arrives on `result_rx` and is
    /// returned. Past it the run is left going - the supervisor owns it and
    /// records its outcome regardless - and the caller gets the `running`
    /// snapshot to collect by id later. A cancellation guard is held for the
    /// whole wait: dropping this future before either outcome (the client
    /// cancelled the call or disconnected) asks the registry to stop the run,
    /// while reaching the result or the deadline disarms the guard so a
    /// backgrounded run keeps going.
    pub(crate) async fn settle(
        self: &Arc<Self>,
        run_id: &str,
        prompt: &str,
        result_rx: oneshot::Receiver<RunResult>,
    ) -> RunResult {
        let mut guard = CancelOnDrop {
            registry: Arc::clone(self),
            run_id: run_id.to_owned(),
            armed: true,
        };
        match tokio::time::timeout(self.reply_deadline, result_rx).await {
            Ok(Ok(result)) => {
                guard.disarm();
                result
            }
            Ok(Err(_closed)) => {
                // The supervisor dropped the sender without a value, which it
                // does only when the run's result already reached the record.
                guard.disarm();
                self.running_snapshot(run_id, prompt)
            }
            Err(_elapsed) => {
                guard.disarm();
                tracing::info!(
                    run_id = %run_id,
                    prompt = %prompt,
                    "the run outlived its call and is collectable by run id"
                );
                self.running_snapshot(run_id, prompt)
            }
        }
    }

    /// The run as a later caller would read it, falling back to a fresh
    /// `running` snapshot for the compiler's benefit: a registered run has a
    /// record, and a running one is never evicted, so the fallback is not a path
    /// the program takes.
    fn running_snapshot(&self, run_id: &str, prompt: &str) -> RunResult {
        self.check(run_id)
            .unwrap_or_else(|| RunResult::running(run_id.to_owned(), prompt, 0))
    }

    /// The failure a run that ended without producing a result reports.
    ///
    /// `elapsed_ms` is taken from the record's own start, so a run that died is
    /// timed the same way a run that finished is.
    fn unfinished(&self, run_id: &str, prompt: &str, join: &JoinError) -> RunResult {
        let elapsed = self
            .records()
            .get(run_id)
            .map_or(0, |record| elapsed_ms(record.started));
        RunResult::failed(
            run_id.to_owned(),
            prompt,
            format!("the run did not finish: {join}"),
            NO_TURNS,
            elapsed,
        )
    }

    /// The records, recovering a poisoned lock.
    ///
    /// Poisoning means a panic while the map was held; the map is plain data
    /// that no panic can leave half-updated, so refusing every later run over it
    /// would cost more than it buys.
    fn records(&self) -> MutexGuard<'_, HashMap<String, Record>> {
        self.records.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Drops every record that finished longer than `retain` ago.
fn evict(records: &mut HashMap<String, Record>, retain: Duration) {
    let now = Instant::now();
    let held = records.len();
    records.retain(|_, record| match &record.finished {
        Some(finished) => now.saturating_duration_since(finished.at) < retain,
        None => true,
    });
    let evicted = held - records.len();
    if evicted > 0 {
        tracing::debug!(evicted, "evicted run record(s) past the retention window");
    }
}

/// Milliseconds since `started`, saturating rather than wrapping.
///
/// The clock is Tokio's, so a paused-clock test measures the time it advanced
/// rather than the time it spent.
pub(crate) fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}
