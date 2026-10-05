//! Draining batches to the worker, delivery accounting, shutdown, and state seams for tests.

use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::time::Instant;

use super::{BATCH, Batch, LogQueue, ShutdownLoss, State};

impl LogQueue {
    /// Blocks until records are available (or the queue is closed and
    /// drained), moves up to [`BATCH`] of them out in global sequence
    /// order, and attaches the pressure summary once both record and byte
    /// occupancy reach their half-capacity low-water marks. Every write
    /// happens on the caller's side, outside the mutex.
    pub(crate) fn take_batch(&self) -> Batch {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            self.merge_pending_rejections(&mut state);
            if self.is_abandoned() {
                return Batch {
                    records: Vec::new(),
                    summary: None,
                    summary_affected: 0,
                    done: true,
                };
            }
            if state.len == 0
                && state.loss.is_empty()
                && state.pending_summaries.is_empty()
                && (!state.closed || self.active_producers() > 0)
            {
                state = self
                    .work_available
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let mut records = Vec::with_capacity(BATCH.min(state.len));
            let pending_fence = state.pending_summary_fence();
            while records.len() < BATCH {
                if pending_fence.is_some_and(|fence| {
                    state
                        .oldest_sequence()
                        .is_none_or(|sequence| sequence >= fence)
                }) {
                    break;
                }
                let Some(record) = state.pop_oldest() else {
                    break;
                };
                records.push(record);
            }
            if self.limits.is_at_low_water(state.len, state.queued_bytes) {
                state.close_loss_episode();
            }
            let ready_summary = state.take_ready_summary();
            let summary_affected = ready_summary.as_ref().map_or(0, |summary| summary.affected);
            let summary = ready_summary.map(|summary| summary.text);
            state.in_flight_records += records.len();
            state.in_flight_summaries += usize::from(summary.is_some());
            state.in_flight_pressure_records = state
                .in_flight_pressure_records
                .saturating_add(summary_affected);
            let done = state.closed
                && state.len == 0
                && state.loss.is_empty()
                && state.pending_summaries.is_empty()
                && self.active_producers() == 0;
            drop(state);
            self.space_available.notify_all();
            return Batch {
                records,
                summary,
                summary_affected,
                done,
            };
        }
    }

    /// Whether the queue is empty. Test seam for the writer's
    /// drop-to-enqueue contract.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len
            == 0
    }

    #[cfg(test)]
    pub(crate) fn hold_lock_for_test(
        &self,
        entered: &std::sync::mpsc::SyncSender<()>,
        release: &std::sync::mpsc::Receiver<()>,
    ) {
        let _state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        entered.send(()).expect("report held queue mutex");
        release.recv().expect("release queue mutex");
    }

    #[cfg(test)]
    pub(super) fn accounting_for_test(&self) -> (usize, usize, usize) {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        (state.len, state.queued_bytes, state.peak_queued_bytes)
    }

    #[cfg(test)]
    pub(crate) fn shutdown_loss_for_test(&self) -> ShutdownLoss {
        ShutdownLoss {
            abandoned_records: self.shutdown_abandoned_records.load(Ordering::Acquire),
            abandoned_summaries: self.shutdown_abandoned_summaries.load(Ordering::Acquire),
            unreported_pressure_records: self
                .shutdown_unreported_pressure_records
                .load(Ordering::Acquire),
        }
    }

    /// Marks one flushed batch as delivered. Records remain in flight until
    /// flush returns because buffered writes alone are not final delivery.
    pub(crate) fn complete_batch(&self, records: usize, had_summary: bool, summary_affected: u64) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.in_flight_records = state.in_flight_records.saturating_sub(records);
        state.in_flight_summaries = state
            .in_flight_summaries
            .saturating_sub(usize::from(had_summary));
        state.in_flight_pressure_records = state
            .in_flight_pressure_records
            .saturating_sub(summary_affected);
        self.subtract_undelivered(u64::try_from(records).unwrap_or(u64::MAX));
        if had_summary {
            Self::saturating_sub(&self.outstanding_summaries, 1);
        }
        Self::saturating_sub(&self.unreported_pressure_records, summary_affected);
    }

    /// Whether shutdown has abandoned delivery after its finite wait.
    pub(crate) fn is_abandoned(&self) -> bool {
        self.abandoned.load(Ordering::Acquire)
    }

    /// Accounts everything not known to have reached the sink and prevents a
    /// later queue batch from beginning. The bounded queue storage stays with
    /// the detached worker rather than making timeout cleanup part of the
    /// caller's latency.
    pub(crate) fn abandon(&self) -> ShutdownLoss {
        self.close_admission();
        self.abandoned.store(true, Ordering::Release);
        let loss = ShutdownLoss {
            abandoned_records: self.outstanding_records(),
            abandoned_summaries: self.outstanding_summaries.swap(0, Ordering::AcqRel),
            unreported_pressure_records: self.unreported_pressure_records.swap(0, Ordering::AcqRel),
        };
        self.shutdown_abandoned_records
            .fetch_add(loss.abandoned_records, Ordering::AcqRel);
        self.shutdown_abandoned_summaries
            .fetch_add(loss.abandoned_summaries, Ordering::AcqRel);
        self.shutdown_unreported_pressure_records
            .fetch_add(loss.unreported_pressure_records, Ordering::AcqRel);
        self.space_available.notify_all();
        self.work_available.notify_all();
        loss
    }

    /// Closes admission and wakes every waiter: producers drop new
    /// records, blocked producers return, and the worker exits once the
    /// queue drains.
    #[cfg(test)]
    pub(crate) fn close(&self) {
        self.close_admission();
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        self.close_locked(&mut state);
        drop(state);
        self.work_available.notify_all();
        self.space_available.notify_all();
    }

    /// Closes admission within an existing shutdown budget. Failure means
    /// the mutex owner consumed that budget and the caller must abandon.
    pub(crate) fn close_until(&self, deadline: Instant) -> bool {
        self.close_admission();
        let Some(mut state) = self.lock_until(deadline) else {
            return false;
        };
        self.close_locked(&mut state);
        drop(state);
        self.work_available.notify_all();
        self.space_available.notify_all();
        true
    }

    fn close_locked(&self, state: &mut State) {
        self.merge_pending_rejections(state);
        if state.closed {
            return;
        }
        state.closed = true;
        self.record_rejections(state, state.blocked_producers);
        self.subtract_undelivered(state.blocked_producers);
    }
}
