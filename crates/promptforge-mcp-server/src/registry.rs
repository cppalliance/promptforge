//! The runs this process has started: admission, the reply deadline, and
//! collecting a run that outlived the call which asked for it.
//!
//! Cursor's remote calls fail at about 300 seconds and no progress notification
//! extends that clock, so a prompt that runs longer than the client will wait
//! must not take its work down with the call. A call therefore blocks for at
//! most `reply_deadline`; past that the caller is handed a `running` result
//! naming its `run_id`, and `check_run` collects the outcome afterwards. The run
//! is never cancelled: the deadline hands the still-live join handle to a
//! supervisor task rather than dropping it, so a run that ends abnormally is
//! still recorded. A backgrounded run therefore always reaches a terminal
//! record - a panic or an abort becomes a `failed` result saying the run did not
//! finish - which is what keeps such a record collectable and, in time,
//! evictable rather than `running` for the life of the process.
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
//! synchronous, so no guard can be held across an `.await`. The one asynchronous
//! method, [`RunRegistry::admit`], touches the semaphore and nothing else.

#[cfg(test)]
mod tests;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

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
pub struct RunSlot {
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
    /// The prompt's frontmatter contract version.
    version: u32,
    /// When the run started.
    started: Instant,
    /// The outcome, absent while the run is still going.
    finished: Option<Finished>,
}

impl Record {
    /// This run as a caller reads it.
    fn snapshot(&self, run_id: &str) -> RunResult {
        match &self.finished {
            Some(finished) => finished.result.clone(),
            None => RunResult::running(
                run_id.to_owned(),
                &self.prompt,
                self.version,
                elapsed_ms(self.started),
            ),
        }
    }
}

/// Every run this process has started, and the limits that govern them.
///
/// One registry serves the whole server: it hands out the run slots, races each
/// run against the reply deadline, and holds the records `check_run` reads.
#[derive(Debug)]
pub struct RunRegistry {
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
    pub fn new(server: &ServerConfig) -> RunRegistry {
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
    pub fn admission_timeout(&self) -> Duration {
        self.admission_timeout
    }

    /// How long a finished run stays collectable.
    #[must_use]
    pub fn retain_completed(&self) -> Duration {
        self.retain_completed
    }

    /// Waits up to [`admission_timeout`](Self::admission_timeout) for a run
    /// slot, and answers `None` when none came free.
    ///
    /// A refusal is the caller's to report; the registry does not queue past the
    /// timeout, so a busy server tells its callers to retry rather than
    /// accumulating them.
    pub async fn admit(&self) -> Option<RunSlot> {
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

    /// Records a run as started. Called before the task that runs it is spawned,
    /// so a `check_run` racing the deadline finds it.
    pub(crate) fn started(&self, run_id: &str, prompt: &str, version: u32) {
        let record = Record {
            prompt: prompt.to_owned(),
            version,
            started: Instant::now(),
            finished: None,
        };
        let _replaced = self.records().insert(run_id.to_owned(), record);
    }

    /// Records what a run produced, which is what makes it collectable.
    ///
    /// A run whose record is gone was never started through
    /// [`started`](Self::started), and its result is dropped rather than
    /// resurrecting a key nothing will read.
    pub(crate) fn finished(&self, run_id: &str, result: RunResult) {
        let mut records = self.records();
        evict(&mut records, self.retain_completed);
        if let Some(record) = records.get_mut(run_id) {
            record.finished = Some(Finished {
                at: Instant::now(),
                result,
            });
        }
    }

    /// The run `run_id` names, still going or finished inside its retention
    /// window, or `None` when no such run is known.
    #[must_use]
    pub fn check(&self, run_id: &str) -> Option<RunResult> {
        let mut records = self.records();
        evict(&mut records, self.retain_completed);
        records.get(run_id).map(|record| record.snapshot(run_id))
    }

    /// Waits up to `reply_deadline` for `task`, and reports the run either way.
    ///
    /// Inside the deadline the task's own result is returned. Past it the run is
    /// left going - the handle passes to a supervisor task, which detaches the
    /// run from this call without abandoning its outcome - and the caller gets
    /// the `running` snapshot to collect by id later.
    pub(crate) async fn settle(
        self: &Arc<Self>,
        run_id: &str,
        prompt: &str,
        version: u32,
        mut task: JoinHandle<RunResult>,
    ) -> RunResult {
        match tokio::time::timeout(self.reply_deadline, &mut task).await {
            Ok(Ok(result)) => result,
            Ok(Err(join)) => {
                // A task that panicked or was aborted wrote no record of its
                // own, so the failure is recorded here and reported once.
                let result = self.unfinished(run_id, prompt, version, &join);
                self.finished(run_id, result.clone());
                result
            }
            Err(_elapsed) => {
                tracing::info!(
                    run_id = %run_id,
                    prompt = %prompt,
                    "the run outlived its call and is collectable by run id"
                );
                self.supervise(run_id.to_owned(), prompt.to_owned(), version, task);
                // The record was written before the task was spawned and a
                // running one is never evicted, so the fallback is the
                // compiler's path and not the program's.
                self.check(run_id)
                    .unwrap_or_else(|| RunResult::running(run_id.to_owned(), prompt, version, 0))
            }
        }
    }

    /// Owns the join handle of a run that outlived its call, and records the
    /// terminal result of one that ends without producing its own.
    ///
    /// A run that finishes normally has already recorded itself, so the
    /// supervisor writes no record for it. A run that panics or is aborted
    /// records nothing, and without this task its record would stay `running`
    /// for the life of the process - never answerable by `check_run`, and never
    /// evicted, since eviction only reaches a record that finished.
    ///
    /// A normal outcome is logged by the runner that constructs it. An abnormal
    /// join is logged here because this task is the only place that can build
    /// and observe its terminal result.
    fn supervise(
        self: &Arc<Self>,
        run_id: String,
        prompt: String,
        version: u32,
        task: JoinHandle<RunResult>,
    ) {
        let registry = Arc::clone(self);
        // Detached deliberately: the handle is the last thing that could observe
        // this run, and there is no later caller to hand it to.
        let _supervisor = tokio::spawn(async move {
            match task.await {
                Ok(_result) => {}
                Err(join) => {
                    let result = registry.unfinished(&run_id, &prompt, version, &join);
                    registry.finished(&run_id, result.clone());
                    tracing::info!(
                        run_id = %run_id,
                        prompt = %prompt,
                        status = ?result.status,
                        turns = result.turns,
                        elapsed_ms = result.elapsed_ms,
                        "a backgrounded run reached its terminal state"
                    );
                }
            }
        });
    }

    /// The failure a run that ended without producing a result reports.
    ///
    /// `elapsed_ms` is taken from the record's own start, so a run that died is
    /// timed the same way a run that finished is.
    fn unfinished(&self, run_id: &str, prompt: &str, version: u32, join: &JoinError) -> RunResult {
        let elapsed = self
            .records()
            .get(run_id)
            .map_or(0, |record| elapsed_ms(record.started));
        RunResult::failed(
            run_id.to_owned(),
            prompt,
            version,
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
