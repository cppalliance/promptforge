//! The in-memory recorder: every run's records in a vector.

use std::sync::{Mutex, MutexGuard, PoisonError};

use super::{Record, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder};

/// A recorder that keeps every run in memory, for tests and doc examples.
///
/// It forgets everything when dropped. Run ids start at `1` and count up
/// in the order runs begin. A write to an unknown run, a write to a run
/// that has ended, and a second [`end_run`](RunRecorder::end_run) each
/// fail, so a test notices a Harness that breaks the call order.
#[derive(Debug, Default)]
pub struct MemoryRecorder {
    runs: Mutex<Vec<MemoryRun>>,
}

#[derive(Debug)]
struct MemoryRun {
    meta: RunMeta,
    records: Vec<Record>,
    outcome: Option<RunOutcome>,
}

#[derive(Debug, thiserror::Error)]
enum MemoryError {
    #[error("memory recorder: unknown run {0}")]
    UnknownRun(RunId),
    #[error("memory recorder: run {0} has ended")]
    RunEnded(RunId),
}

impl MemoryRecorder {
    /// An empty recorder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The run's records in the order they were appended; empty for a run
    /// this recorder never began.
    #[must_use]
    pub fn records(&self, run: RunId) -> Vec<Record> {
        self.lock()
            .get(slot(run))
            .map(|found| found.records.clone())
            .unwrap_or_default()
    }

    /// The meta the run began with; `None` for a run this recorder never
    /// began.
    #[must_use]
    pub fn meta(&self, run: RunId) -> Option<RunMeta> {
        self.lock().get(slot(run)).map(|found| found.meta.clone())
    }

    /// How the run ended; `None` while the run is open, and for a run this
    /// recorder never began.
    #[must_use]
    pub fn outcome(&self, run: RunId) -> Option<RunOutcome> {
        self.lock()
            .get(slot(run))
            .and_then(|found| found.outcome.clone())
    }

    fn lock(&self) -> MutexGuard<'_, Vec<MemoryRun>> {
        // Every writer leaves the vector whole between its pushes and
        // stores, so a poisoned lock still holds a usable value.
        self.runs.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Where run `run` sits in the vector: id `1` is the first run. An id the
/// recorder never issued maps past the end, so a lookup misses.
fn slot(run: RunId) -> usize {
    usize::try_from(run.get())
        .ok()
        .and_then(|id| id.checked_sub(1))
        .unwrap_or(usize::MAX)
}

/// The run `run`, if it exists and still takes writes.
fn open_run(runs: &mut [MemoryRun], run: RunId) -> Result<&mut MemoryRun, MemoryError> {
    let found = runs
        .get_mut(slot(run))
        .ok_or(MemoryError::UnknownRun(run))?;
    if found.outcome.is_some() {
        return Err(MemoryError::RunEnded(run));
    }
    Ok(found)
}

impl RunRecorder for MemoryRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            let mut runs = self.lock();
            let id = i64::try_from(runs.len() + 1).map_err(RecorderError::new)?;
            runs.push(MemoryRun {
                meta,
                records: Vec::new(),
                outcome: None,
            });
            Ok(RunId::from_raw(id))
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut runs = self.lock();
            open_run(&mut runs, run)
                .map(|open| open.records.push(record))
                .map_err(RecorderError::new)
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            let mut runs = self.lock();
            open_run(&mut runs, run)
                .map(|open| open.outcome = Some(outcome))
                .map_err(RecorderError::new)
        })
    }
}
