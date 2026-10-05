//! Producer admission: enqueueing formatted records and accounting rejected ones.

use std::sync::PoisonError;
use std::time::Instant;

use super::{FormatStatus, LogPriority, LogQueue};

impl LogQueue {
    /// Enqueues `line`, assigning its global sequence atomically with
    /// successful admission. Formatting and allocation still happen before
    /// the mutex. When either record or byte capacity is exhausted, the
    /// oldest eligible lower-priority records are evicted until the line
    /// fits; with none eligible the producer blocks on the condition
    /// variable until the worker frees space. After admission closes, new
    /// records are dropped.
    #[cfg(test)]
    pub(crate) fn enqueue(&self, priority: LogPriority, line: Box<str>) {
        self.enqueue_formatted(priority, line, FormatStatus::Complete);
    }

    /// Enqueues one bounded formatter result and accounts marked
    /// truncation in the same pressure episode as queue eviction.
    pub(crate) fn enqueue_formatted(
        &self,
        priority: LogPriority,
        line: Box<str>,
        status: FormatStatus,
    ) {
        self.enqueue_after(priority, line, status, || {});
    }

    /// Testable preparation boundary: `before_admission` runs after the
    /// owned record exists but before admission locks and assigns sequence.
    pub(super) fn enqueue_after(
        &self,
        priority: LogPriority,
        line: Box<str>,
        status: FormatStatus,
        before_admission: impl FnOnce(),
    ) {
        self.enqueue_around(priority, line, status, before_admission, || {});
    }

    pub(super) fn enqueue_around(
        &self,
        priority: LogPriority,
        line: Box<str>,
        status: FormatStatus,
        before_admission: impl FnOnce(),
        after_admission: impl FnOnce(),
    ) {
        if !self.begin_producer() {
            return;
        }
        let started = Instant::now();
        let deadline = started
            .checked_add(self.limits.producer_wait)
            .unwrap_or(started);
        let line_bytes = line.len();
        before_admission();
        let Some(mut state) = self.lock_until(deadline) else {
            if !self.is_abandoned() {
                self.record_rejection_without_lock();
            }
            self.finish_rejected_producer();
            self.work_available.notify_one();
            return;
        };
        self.merge_pending_rejections(&mut state);
        let mut close_accounted = false;
        loop {
            if state.closed {
                if close_accounted {
                    self.finish_admitted_producer();
                } else {
                    self.record_rejection(&mut state);
                    self.finish_rejected_producer();
                }
                drop(state);
                self.work_available.notify_one();
                return;
            }
            if line_bytes > self.limits.max_bytes {
                self.record_rejection(&mut state);
                self.finish_rejected_producer();
                drop(state);
                self.work_available.notify_one();
                return;
            }
            if state.can_admit(self.limits, line_bytes) {
                if status == FormatStatus::Truncated {
                    self.record_truncation(&mut state);
                }
                state.admit(priority, line);
                after_admission();
                self.finish_admitted_producer();
                drop(state);
                self.work_available.notify_one();
                return;
            }
            if let Some(evicted) = state.evict_for(priority) {
                self.record_eviction(&mut state, &evicted);
                continue;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                self.record_rejection(&mut state);
                self.finish_rejected_producer();
                drop(state);
                self.work_available.notify_one();
                return;
            }
            state.blocked_producers = state.blocked_producers.saturating_add(1);
            let (next, timeout) = self
                .space_available
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner);
            state = next;
            state.blocked_producers = state.blocked_producers.saturating_sub(1);
            close_accounted = state.closed;
            if timeout.timed_out() && !state.closed {
                self.record_rejection(&mut state);
                self.finish_rejected_producer();
                drop(state);
                self.work_available.notify_one();
                return;
            }
        }
    }

    /// Rejects invalid formatter bytes and wakes the worker so the loss is
    /// observable even when no queue record accompanies it.
    pub(crate) fn reject_formatted(&self) {
        if !self.begin_producer() {
            return;
        }
        let started = Instant::now();
        let deadline = started
            .checked_add(self.limits.producer_wait)
            .unwrap_or(started);
        let Some(mut state) = self.lock_until(deadline) else {
            if !self.is_abandoned() {
                self.record_rejection_without_lock();
            }
            self.finish_rejected_producer();
            self.work_available.notify_one();
            return;
        };
        self.merge_pending_rejections(&mut state);
        if state.closed {
            self.record_rejection(&mut state);
            self.finish_rejected_producer();
            drop(state);
            self.work_available.notify_one();
            return;
        }
        self.record_rejection(&mut state);
        self.finish_rejected_producer();
        drop(state);
        self.work_available.notify_one();
    }
}
