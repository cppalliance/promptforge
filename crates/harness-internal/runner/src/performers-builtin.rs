//! The performers the runner supplies itself: the timer, the store, and
//! the task-events read. Each is machinery the runner already holds -
//! tokio's timer wheel, the Engine's store operation over the effect's
//! own access, and the run log the loop writes - so none needs a crate of
//! its own. The chat and tool performers reach outward (a gateway and the
//! activated capabilities) and live with what they reach.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use harness_log::RunId;
use promptforge::event::Event;
use promptforge::ids::TaskId;
use promptforge::vfs::Access;
use promptforge::vfs::{StoreOp, StoreOutcome, VfsError, perform_store_op};

use super::{BoxFuture, StorePerformer, TaskEventsPerformer, TimerPerformer};
use crate::effect_loop::SharedLog;

/// Sleeps on tokio's timer wheel.
///
/// The wheel multiplexes every pending sleep, so the timer machinery is
/// tokio's; a `Timer` effect is one `tokio::time::sleep`, and the loop's
/// abort of the performer task tears the sleep down.
#[derive(Clone, Copy, Debug, Default)]
pub struct TokioTimer;

impl TimerPerformer for TokioTimer {
    fn sleep(&self, seconds: f64) -> BoxFuture<()> {
        // The protocol bounds `seconds` to a non-negative, finite value
        // within `Duration`'s range before the effect is issued; anything
        // outside that fires at once rather than never, as the Engine's
        // own tokio test driver does.
        let duration = Duration::try_from_secs_f64(seconds).unwrap_or(Duration::ZERO);
        Box::pin(tokio::time::sleep(duration))
    }
}

/// Performs a store operation through the Engine's store facade over the
/// store view the effect carries.
///
/// Synchronous: the loop runs it on the blocking pool. When the access
/// drops never affects correctness: claims follow happens-before within
/// the run's scope, and the run ends that scope at `Done`.
#[derive(Clone, Copy, Debug, Default)]
pub struct VfsStore;

impl StorePerformer for VfsStore {
    fn perform(&self, access: &Access, op: StoreOp) -> Result<StoreOutcome, VfsError> {
        perform_store_op(access, op)
    }
}

/// Answers a `TaskEvents` read from the run log.
///
/// The loop commits a step's events before it issues the step's effects,
/// so a read issued in a step sees everything reported before it. The
/// events come back in the task's sequence order, narrowed to those after
/// `last` as the Engine's `tasks.events` promises (`last` is the highest
/// sequence number the caller has already seen).
///
/// A log that refuses the read, or a stored payload that no longer parses
/// as an event, is the Harness's fault, not the task's: it is reported
/// through `tracing` and the read answers with what it could recover (an
/// empty slice for a refused read), since the answer's shape has no room
/// for an error.
#[derive(Clone)]
pub struct LogTaskEvents {
    log: SharedLog,
    run_id: RunId,
}

impl LogTaskEvents {
    /// A reader over `run_id`'s records in `log`: the same log the loop
    /// driving that run writes.
    #[must_use]
    pub fn new(log: SharedLog, run_id: RunId) -> Self {
        Self { log, run_id }
    }
}

impl fmt::Debug for LogTaskEvents {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LogTaskEvents")
            .field("run_id", &self.run_id)
            .finish_non_exhaustive()
    }
}

impl TaskEventsPerformer for LogTaskEvents {
    fn events(&self, task: TaskId, last: Option<u32>) -> BoxFuture<Vec<Event>> {
        let log = Arc::clone(&self.log);
        let run_id = self.run_id;
        Box::pin(async move {
            let task_path = task.to_string();
            // The log's own `last` is a different `last`: it keeps the final
            // `n` records, while the effect's is the highest seq the caller
            // has already seen. The narrowing by seq happens below, so the
            // log reads the whole task.
            let read = log
                .lock()
                .await
                .events_for_task(run_id, &task_path, None)
                .await;
            let payloads = match read {
                Ok(payloads) => payloads,
                Err(error) => {
                    tracing::error!(
                        task = %task_path,
                        error = %error,
                        "the run log refused a task-events read; the task reads as empty"
                    );
                    return Vec::new();
                }
            };
            payloads
                .into_iter()
                .filter_map(|payload| match serde_json::from_value::<Event>(payload) {
                    Ok(event) => Some(event),
                    Err(error) => {
                        tracing::error!(
                            task = %task_path,
                            error = %error,
                            "a stored event payload does not parse as an event; it is skipped"
                        );
                        None
                    }
                })
                .filter(|event| last.is_none_or(|last| event.provenance().seq > last))
                .collect()
        })
    }
}
