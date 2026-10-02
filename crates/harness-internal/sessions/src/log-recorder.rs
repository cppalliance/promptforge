//! The run recorder over the Harness's own run log: a thin `RunRecorder`
//! that takes the log's lock for each call and hands the write to
//! `RunLog`. The sessions read their transcripts from the same log behind
//! the same lock.

use harness_runner::recorder::{
    Record, RecorderError, RecorderFuture, RunId, RunMeta, RunOutcome, RunRecorder,
};

use super::SharedLog;

/// A [`RunRecorder`] over a shared run log. A log failure is the source of
/// the [`RecorderError`] it returns.
pub(crate) struct LogRecorder {
    log: SharedLog,
}

impl LogRecorder {
    /// A recorder that writes through `log`.
    pub(crate) fn new(log: SharedLog) -> Self {
        Self { log }
    }
}

impl RunRecorder for LogRecorder {
    fn begin_run(&self, meta: RunMeta) -> RecorderFuture<'_, RunId> {
        Box::pin(async move {
            self.log
                .lock()
                .await
                .begin_run(meta)
                .await
                .map_err(RecorderError::new)
        })
    }

    fn append(&self, run: RunId, record: Record) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            self.log
                .lock()
                .await
                .append(run, record)
                .await
                .map(|_seq| ())
                .map_err(RecorderError::new)
        })
    }

    fn end_run(&self, run: RunId, outcome: RunOutcome) -> RecorderFuture<'_, ()> {
        Box::pin(async move {
            self.log
                .lock()
                .await
                .end_run(run, outcome)
                .await
                .map_err(RecorderError::new)
        })
    }
}

#[cfg(test)]
#[path = "log-recorder-tests.rs"]
mod tests;
