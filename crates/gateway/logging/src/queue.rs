//! The bounded priority queue: one deque per level under one mutex, a fixed
//! total capacity, and eviction rules that protect Warn and Error records.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use crate::config::LOG_LIMITS;

mod admit;
mod drain;
mod records;
#[cfg(test)]
mod tests;

pub(crate) use records::{Batch, FormatStatus, LogPriority, LogRecord, ShutdownLoss};

const ADMISSION_CLOSED: u64 = 1 << 63;
const ACTIVE_PRODUCER_ONE: u64 = 1 << 32;
const ACTIVE_PRODUCERS: u64 = ((1 << 31) - 1) << 32;
const UNDELIVERED_RECORDS: u64 = (1 << 32) - 1;

/// Total records the queue holds before producers evict or block.
pub(crate) const CAPACITY: usize = 8192;

/// Records the worker moves to local storage per drain.
const BATCH: usize = 256;

/// The shared queue state producers and the single worker synchronize on.
#[derive(Debug)]
pub(crate) struct LogQueue {
    state: Mutex<State>,
    work_available: Condvar,
    space_available: Condvar,
    limits: QueueLimits,
    admission_gate: AtomicU64,
    abandoned: AtomicBool,
    pending_rejections: AtomicU64,
    outstanding_summaries: AtomicU64,
    unreported_pressure_records: AtomicU64,
    shutdown_abandoned_records: AtomicU64,
    shutdown_abandoned_summaries: AtomicU64,
    shutdown_unreported_pressure_records: AtomicU64,
}

/// Admission limits and their shared low-water definition. Pressure has
/// recovered only when both dimensions are at or below half capacity.
#[derive(Debug, Clone, Copy)]
struct QueueLimits {
    max_records: usize,
    max_bytes: usize,
    producer_wait: Duration,
}

impl QueueLimits {
    fn is_at_low_water(self, records: usize, bytes: usize) -> bool {
        records <= self.max_records / 2 && bytes <= self.max_bytes / 2
    }
}

#[derive(Debug)]
struct State {
    lanes: [VecDeque<LogRecord>; 5],
    len: usize,
    queued_bytes: usize,
    next_sequence: u64,
    closed: bool,
    loss: LossCounts,
    pending_summaries: VecDeque<PendingSummary>,
    pending_pressure_records: u64,
    in_flight_records: usize,
    in_flight_summaries: usize,
    in_flight_pressure_records: u64,
    blocked_producers: u64,
    #[cfg(test)]
    peak_queued_bytes: usize,
}

/// A closed pressure episode sequenced immediately after every record that
/// had already been admitted when occupancy recovered.
#[derive(Debug)]
struct PendingSummary {
    after_sequence: u64,
    text: Box<str>,
    affected: u64,
}

/// Counts every way record content is lost during one observable pressure
/// episode.
#[derive(Debug, Default)]
struct LossCounts {
    evicted: [u64; 5],
    truncated: u64,
    rejected: u64,
}

impl LossCounts {
    fn is_empty(&self) -> bool {
        self.evicted.iter().all(|&count| count == 0) && self.truncated == 0 && self.rejected == 0
    }

    fn evicted(&self) -> u64 {
        self.evicted
            .iter()
            .fold(0u64, |total, count| total.saturating_add(*count))
    }

    fn affected(&self) -> u64 {
        self.evicted()
            .saturating_add(self.truncated)
            .saturating_add(self.rejected)
    }

    fn take_summary(&mut self) -> Option<(Box<str>, u64)> {
        if self.is_empty() {
            return None;
        }
        let dropped = self.evicted().saturating_add(self.rejected);
        let affected = self.affected();
        let summary = format!(
            "log pressure affected {affected} record(s): dropped={dropped}, debug={}, trace={}, info={}, truncated={}, rejected={}\n",
            self.evicted[LogPriority::Debug.lane()],
            self.evicted[LogPriority::Trace.lane()],
            self.evicted[LogPriority::Info.lane()],
            self.truncated,
            self.rejected,
        )
        .into_boxed_str();
        *self = Self::default();
        Some((summary, affected))
    }
}

impl State {
    fn can_admit(&self, limits: QueueLimits, line_bytes: usize) -> bool {
        self.len < limits.max_records
            && line_bytes <= limits.max_bytes.saturating_sub(self.queued_bytes)
    }

    fn admit(&mut self, priority: LogPriority, line: Box<str>) {
        let line_bytes = line.len();
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.wrapping_add(1);
        self.lanes[priority.lane()].push_back(LogRecord {
            sequence,
            priority,
            line,
        });
        self.len += 1;
        self.queued_bytes += line_bytes;
        #[cfg(test)]
        {
            self.peak_queued_bytes = self.peak_queued_bytes.max(self.queued_bytes);
        }
    }

    /// Evicts the oldest record the incoming priority is allowed to
    /// displace.
    fn evict_for(&mut self, priority: LogPriority) -> Option<LogRecord> {
        for &lane_priority in priority.evictable() {
            if let Some(record) = self.lanes[lane_priority.lane()].pop_front() {
                self.len -= 1;
                self.queued_bytes -= record.line.len();
                return Some(record);
            }
        }
        None
    }

    fn oldest_lane(&self) -> Option<usize> {
        let mut oldest: Option<usize> = None;
        for (index, lane) in self.lanes.iter().enumerate() {
            let Some(front) = lane.front() else {
                continue;
            };
            match oldest {
                Some(current)
                    if front.sequence
                        >= self.lanes[current]
                            .front()
                            .map_or(u64::MAX, |head| head.sequence) => {}
                _ => oldest = Some(index),
            }
        }
        oldest
    }

    fn oldest_sequence(&self) -> Option<u64> {
        self.oldest_lane()
            .and_then(|index| self.lanes[index].front())
            .map(|record| record.sequence)
    }

    /// Pops the lane head with the smallest global sequence, so drained
    /// output stays chronological across lanes.
    fn pop_oldest(&mut self) -> Option<LogRecord> {
        let index = self.oldest_lane()?;
        let record = self.lanes[index].pop_front();
        if let Some(record) = &record {
            self.len -= 1;
            self.queued_bytes -= record.line.len();
        }
        record
    }

    fn close_loss_episode(&mut self) {
        let Some((text, affected)) = self.loss.take_summary() else {
            return;
        };
        self.pending_pressure_records = self.pending_pressure_records.saturating_add(affected);
        self.pending_summaries.push_back(PendingSummary {
            after_sequence: self.next_sequence,
            text,
            affected,
        });
    }

    fn pending_summary_fence(&self) -> Option<u64> {
        self.pending_summaries
            .front()
            .map(|summary| summary.after_sequence)
    }

    fn take_ready_summary(&mut self) -> Option<PendingSummary> {
        let fence = self.pending_summary_fence()?;
        if self
            .oldest_sequence()
            .is_some_and(|sequence| sequence < fence)
        {
            return None;
        }
        let summary = self.pending_summaries.pop_front()?;
        self.pending_pressure_records = self
            .pending_pressure_records
            .saturating_sub(summary.affected);
        Some(summary)
    }
}

impl LogQueue {
    pub(crate) fn new() -> Self {
        Self::with_limits(
            CAPACITY,
            LOG_LIMITS.max_queued_bytes,
            LOG_LIMITS.producer_wait,
        )
    }

    fn with_limits(max_records: usize, max_bytes: usize, producer_wait: Duration) -> Self {
        assert!(max_records > 0, "a queue needs record capacity");
        assert!(max_bytes > 0, "a queue needs byte capacity");
        Self {
            state: Mutex::new(State {
                lanes: std::array::from_fn(|_| VecDeque::new()),
                len: 0,
                queued_bytes: 0,
                next_sequence: 0,
                closed: false,
                loss: LossCounts::default(),
                pending_summaries: VecDeque::with_capacity(
                    max_records.div_ceil(BATCH).saturating_add(1),
                ),
                pending_pressure_records: 0,
                in_flight_records: 0,
                in_flight_summaries: 0,
                in_flight_pressure_records: 0,
                blocked_producers: 0,
                #[cfg(test)]
                peak_queued_bytes: 0,
            }),
            work_available: Condvar::new(),
            space_available: Condvar::new(),
            limits: QueueLimits {
                max_records,
                max_bytes,
                producer_wait,
            },
            admission_gate: AtomicU64::new(0),
            abandoned: AtomicBool::new(false),
            pending_rejections: AtomicU64::new(0),
            outstanding_summaries: AtomicU64::new(0),
            unreported_pressure_records: AtomicU64::new(0),
            shutdown_abandoned_records: AtomicU64::new(0),
            shutdown_abandoned_summaries: AtomicU64::new(0),
            shutdown_unreported_pressure_records: AtomicU64::new(0),
        }
    }

    fn lock_until(&self, deadline: Instant) -> Option<MutexGuard<'_, State>> {
        loop {
            match self.state.try_lock() {
                Ok(state) => return Some(state),
                Err(TryLockError::Poisoned(error)) => return Some(error.into_inner()),
                Err(TryLockError::WouldBlock) => {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return None;
                    }
                    std::thread::yield_now();
                }
            }
        }
    }

    fn begin_producer(&self) -> bool {
        let mut gate = self.admission_gate.load(Ordering::Acquire);
        loop {
            if gate & ADMISSION_CLOSED != 0
                || gate & ACTIVE_PRODUCERS == ACTIVE_PRODUCERS
                || gate & UNDELIVERED_RECORDS == UNDELIVERED_RECORDS
            {
                return false;
            }
            match self.admission_gate.compare_exchange_weak(
                gate,
                gate + ACTIVE_PRODUCER_ONE + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(current) => gate = current,
            }
        }
    }

    fn finish_admitted_producer(&self) {
        let previous = self
            .admission_gate
            .fetch_sub(ACTIVE_PRODUCER_ONE, Ordering::AcqRel);
        debug_assert!(previous & ACTIVE_PRODUCERS > 0);
    }

    fn finish_rejected_producer(&self) {
        let previous = self
            .admission_gate
            .fetch_sub(ACTIVE_PRODUCER_ONE + 1, Ordering::AcqRel);
        debug_assert!(previous & ACTIVE_PRODUCERS > 0);
        debug_assert!(previous & UNDELIVERED_RECORDS > 0);
    }

    fn close_admission(&self) {
        self.admission_gate
            .fetch_or(ADMISSION_CLOSED, Ordering::AcqRel);
    }

    fn active_producers(&self) -> u64 {
        (self.admission_gate.load(Ordering::Acquire) & ACTIVE_PRODUCERS) >> 32
    }

    fn outstanding_records(&self) -> u64 {
        self.admission_gate.load(Ordering::Acquire) & UNDELIVERED_RECORDS
    }

    fn subtract_undelivered(&self, amount: u64) {
        let _ = self
            .admission_gate
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                let records = current & UNDELIVERED_RECORDS;
                Some(current.saturating_sub(records.min(amount)))
            });
    }

    fn saturating_sub(counter: &AtomicU64, amount: u64) {
        let _ = counter.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            Some(current.saturating_sub(amount))
        });
    }

    fn begin_loss(&self, state: &State) {
        if state.loss.is_empty() {
            self.outstanding_summaries.fetch_add(1, Ordering::AcqRel);
        }
    }

    fn record_rejection(&self, state: &mut State) {
        self.begin_loss(state);
        state.loss.rejected = state.loss.rejected.saturating_add(1);
        self.unreported_pressure_records
            .fetch_add(1, Ordering::AcqRel);
    }

    fn record_rejections(&self, state: &mut State, count: u64) {
        if count == 0 {
            return;
        }
        self.begin_loss(state);
        state.loss.rejected = state.loss.rejected.saturating_add(count);
        self.unreported_pressure_records
            .fetch_add(count, Ordering::AcqRel);
    }

    fn record_rejection_without_lock(&self) {
        if self.pending_rejections.fetch_add(1, Ordering::AcqRel) == 0 {
            self.outstanding_summaries.fetch_add(1, Ordering::AcqRel);
        }
        self.unreported_pressure_records
            .fetch_add(1, Ordering::AcqRel);
    }

    fn merge_pending_rejections(&self, state: &mut State) {
        let pending = self.pending_rejections.swap(0, Ordering::AcqRel);
        if pending == 0 {
            return;
        }
        if !state.loss.is_empty() {
            Self::saturating_sub(&self.outstanding_summaries, 1);
        }
        state.loss.rejected = state.loss.rejected.saturating_add(pending);
    }

    fn record_truncation(&self, state: &mut State) {
        self.begin_loss(state);
        state.loss.truncated = state.loss.truncated.saturating_add(1);
        self.unreported_pressure_records
            .fetch_add(1, Ordering::AcqRel);
    }

    fn record_eviction(&self, state: &mut State, record: &LogRecord) {
        self.begin_loss(state);
        state.loss.evicted[record.priority.lane()] =
            state.loss.evicted[record.priority.lane()].saturating_add(1);
        self.subtract_undelivered(1);
        self.unreported_pressure_records
            .fetch_add(1, Ordering::AcqRel);
    }

    #[cfg(test)]
    fn new_for_test(max_records: usize, max_bytes: usize) -> Self {
        Self::with_limits(max_records, max_bytes, LOG_LIMITS.producer_wait)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_wait(
        max_records: usize,
        max_bytes: usize,
        producer_wait: Duration,
    ) -> Self {
        Self::with_limits(max_records, max_bytes, producer_wait)
    }
}
