//! Spawning, polling, and joining the single drain thread.

use std::io;
use std::sync::Arc;

use super::{LogWorker, SegmentedFile, Sink};
#[cfg(test)]
use super::{StallPoint, StalledSinkControl};
use crate::queue::LogQueue;

impl LogWorker {
    /// Spawns the single worker thread. It blocks on the queue, swaps up to
    /// a batch of records into local storage, and performs every write and
    /// flush outside the mutex.
    ///
    /// # Errors
    /// Returns the I/O failure from spawning the thread.
    pub(crate) fn spawn(queue: Arc<LogQueue>, file: SegmentedFile) -> io::Result<Self> {
        Self::spawn_with_sink(queue, Sink::Segmented(file))
    }

    fn spawn_with_sink(queue: Arc<LogQueue>, mut sink: Sink) -> io::Result<Self> {
        let handle = std::thread::Builder::new()
            .name("gateway-logging".to_string())
            .spawn(move || {
                loop {
                    let batch = queue.take_batch();
                    let records = batch.records.len();
                    let had_summary = batch.summary.is_some();
                    for record in &batch.records {
                        if queue.is_abandoned() {
                            return;
                        }
                        sink.write_line(&record.line);
                    }
                    if let Some(summary) = &batch.summary {
                        if queue.is_abandoned() {
                            return;
                        }
                        sink.write_line(summary);
                    }
                    if queue.is_abandoned() {
                        return;
                    }
                    sink.flush();
                    if queue.is_abandoned() {
                        return;
                    }
                    queue.complete_batch(records, had_summary, batch.summary_affected);
                    if batch.done {
                        break;
                    }
                }
            })?;
        Ok(Self { handle })
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub(crate) fn join(self) -> std::thread::Result<()> {
        self.handle.join()
    }

    #[cfg(test)]
    pub(crate) fn spawn_stalled(
        queue: Arc<LogQueue>,
        point: StallPoint,
    ) -> io::Result<(Self, StalledSinkControl)> {
        let (release, control) = crate::fault_injection::release_point();
        let worker = Self::spawn_with_sink(
            queue,
            Sink::Stalled {
                point,
                release: Some(release),
            },
        )?;
        Ok((worker, StalledSinkControl(control)))
    }
}

#[cfg(test)]
impl StalledSinkControl {
    pub(crate) fn wait_until_stalled(
        &self,
        timeout: std::time::Duration,
    ) -> Result<(), std::sync::mpsc::RecvTimeoutError> {
        self.0.wait_until_reached(timeout)
    }

    pub(crate) fn release(self) {
        self.0.release();
    }
}
