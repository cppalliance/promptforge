//! The bounded priority queue: one deque per level under one mutex, a fixed
//! total capacity, and eviction rules that protect Warn and Error records.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError, TryLockError};
use std::time::{Duration, Instant};

use crate::config::LOG_LIMITS;

const ADMISSION_CLOSED: u64 = 1 << 63;
const ACTIVE_PRODUCER_ONE: u64 = 1 << 32;
const ACTIVE_PRODUCERS: u64 = ((1 << 31) - 1) << 32;
const UNDELIVERED_RECORDS: u64 = (1 << 32) - 1;

/// Total records the queue holds before producers evict or block.
pub(crate) const CAPACITY: usize = 8192;

/// Records the worker moves to local storage per drain.
pub(crate) const BATCH: usize = 256;

/// The priority lanes, from most to least protected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogPriority {
    Error,
    Warn,
    Info,
    Trace,
    Debug,
}

impl LogPriority {
    /// Maps a tracing level onto its lane.
    pub(crate) fn from_level(level: tracing::Level) -> Self {
        if level == tracing::Level::ERROR {
            Self::Error
        } else if level == tracing::Level::WARN {
            Self::Warn
        } else if level == tracing::Level::INFO {
            Self::Info
        } else if level == tracing::Level::DEBUG {
            Self::Debug
        } else {
            Self::Trace
        }
    }

    /// The deque index: Error is lane 0, Debug lane 4.
    fn lane(self) -> usize {
        match self {
            Self::Error => 0,
            Self::Warn => 1,
            Self::Info => 2,
            Self::Trace => 3,
            Self::Debug => 4,
        }
    }

    /// The lanes an incoming record at this priority may evict from, in
    /// eviction order. A record never evicts a more important one: Debug
    /// evicts only Debug, Trace adds Trace, and anything at Info or above
    /// may evict any of the three lowest lanes. Warn and Error records are
    /// never eviction targets.
    fn evictable(self) -> &'static [LogPriority] {
        match self {
            Self::Debug => &[Self::Debug],
            Self::Trace => &[Self::Debug, Self::Trace],
            Self::Error | Self::Warn | Self::Info => &[Self::Debug, Self::Trace, Self::Info],
        }
    }
}

/// One formatted event: its global sequence, its lane, and the owned line.
#[derive(Debug)]
pub(crate) struct LogRecord {
    pub(crate) sequence: u64,
    pub(crate) priority: LogPriority,
    pub(crate) line: Box<str>,
}

/// What one worker drain produced: the records in global sequence order,
/// the pressure summary once the queue empties after evictions, and whether
/// a closed queue has nothing left.
#[derive(Debug)]
pub(crate) struct Batch {
    pub(crate) records: Vec<LogRecord>,
    pub(crate) summary: Option<Box<str>>,
    pub(crate) summary_affected: u64,
    pub(crate) done: bool,
}

/// Whether bounded formatting retained a whole record or a marked prefix.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FormatStatus {
    Complete,
    Truncated,
}

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

/// Records and already-built summaries that could not be delivered before
/// the shutdown budget expired. The counters live in queue state from
/// construction, so recording a timeout never needs to allocate.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShutdownLoss {
    pub(crate) abandoned_records: u64,
    pub(crate) abandoned_summaries: u64,
    pub(crate) unreported_pressure_records: u64,
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
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                let records = current & UNDELIVERED_RECORDS;
                Some(current.saturating_sub(records.min(amount)))
            });
    }

    fn saturating_sub(counter: &AtomicU64, amount: u64) {
        let _ = counter.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
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
    fn enqueue_after(
        &self,
        priority: LogPriority,
        line: Box<str>,
        status: FormatStatus,
        before_admission: impl FnOnce(),
    ) {
        self.enqueue_around(priority, line, status, before_admission, || {});
    }

    fn enqueue_around(
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

    /// Whether the queue holds no records. Test seam for the writer's
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
    pub(crate) fn new_for_test(max_records: usize, max_bytes: usize) -> Self {
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
    fn accounting_for_test(&self) -> (usize, usize, usize) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::mpsc;
    use std::time::Duration;

    fn line(text: &str) -> Box<str> {
        Box::from(text)
    }

    fn drain_all(queue: &LogQueue) -> Vec<LogRecord> {
        let mut all = Vec::new();
        loop {
            let batch = queue.take_batch();
            all.extend(batch.records);
            if batch.done {
                return all;
            }
        }
    }

    #[test]
    fn a_full_queue_evicts_oldest_debug_then_trace_then_info() {
        let queue = LogQueue::new();
        queue.enqueue(LogPriority::Debug, line("debug-oldest"));
        queue.enqueue(LogPriority::Trace, line("trace-oldest"));
        queue.enqueue(LogPriority::Info, line("info-oldest"));
        for index in 0..(CAPACITY - 3) {
            queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
        }
        // Each Error evicts the oldest record of the least protected
        // eligible lane: Debug first, then Trace, then Info.
        queue.enqueue(LogPriority::Error, line("error-1"));
        queue.enqueue(LogPriority::Error, line("error-2"));
        queue.enqueue(LogPriority::Error, line("error-3"));
        queue.close();

        let records = drain_all(&queue);
        let lines: Vec<&str> = records.iter().map(|record| &*record.line).collect();
        assert!(
            !lines.contains(&"debug-oldest"),
            "the oldest Debug is the first eviction victim"
        );
        assert!(
            !lines.contains(&"trace-oldest"),
            "with the Debug lane empty, the oldest Trace goes next"
        );
        assert!(
            !lines.contains(&"info-oldest"),
            "with Debug and Trace empty, the oldest Info goes last"
        );
        for wanted in ["error-1", "error-2", "error-3"] {
            assert!(lines.contains(&wanted), "the evicting record is retained");
        }
        assert_eq!(
            records.len(),
            CAPACITY,
            "eviction keeps the queue at capacity"
        );
    }

    #[test]
    fn eviction_never_displaces_a_more_important_record() {
        let queue = LogQueue::new();
        queue.enqueue(LogPriority::Trace, line("trace-kept"));
        queue.enqueue(LogPriority::Info, line("info-kept"));
        for index in 0..(CAPACITY - 2) {
            queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
        }
        // A Trace may evict a Trace but never an Info or a Warn.
        queue.enqueue(LogPriority::Trace, line("trace-new"));
        queue.close();

        let records = drain_all(&queue);
        let lines: Vec<&str> = records.iter().map(|record| &*record.line).collect();
        assert!(
            !lines.contains(&"trace-kept"),
            "Trace evicts the oldest Trace"
        );
        assert!(
            lines.contains(&"info-kept"),
            "Trace never evicts Info: no priority inversion"
        );
        assert!(
            lines.contains(&"trace-new"),
            "the evicting record is retained"
        );
        assert!(
            (0..(CAPACITY - 2)).all(|index| lines.contains(&format!("warn-{index}").as_str())),
            "Warn records are never eviction victims"
        );
    }

    #[test]
    fn a_producer_with_no_eligible_record_blocks_until_space_opens() {
        let queue = Arc::new(LogQueue::new_for_test_with_wait(
            CAPACITY,
            LOG_LIMITS.max_queued_bytes,
            Duration::from_secs(1),
        ));
        for index in 0..CAPACITY {
            queue.enqueue(LogPriority::Warn, line(&format!("warn-{index}")));
        }
        // A Debug record may evict only Debug, and the queue holds none:
        // the producer blocks instead of dropping or inverting priority.
        let producer_queue = Arc::clone(&queue);
        let (done_tx, done_rx) = mpsc::channel();
        let producer = std::thread::spawn(move || {
            producer_queue.enqueue(LogPriority::Debug, line("debug-blocked"));
            done_tx.send(()).expect("report enqueue");
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(200)).is_err(),
            "a full queue of Warn records blocks a Debug producer"
        );

        let batch = queue.take_batch();
        assert_eq!(batch.records.len(), BATCH, "the drain frees space");
        done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the blocked producer wakes once space opens");
        producer.join().expect("the producer joins");
        queue.close();

        let records = drain_all(&queue);
        assert!(
            records
                .iter()
                .any(|record| &*record.line == "debug-blocked"),
            "the blocked record lands once space opens"
        );
        assert_eq!(
            records
                .iter()
                .filter(|record| record.line.starts_with("warn-"))
                .count(),
            CAPACITY - BATCH,
            "no Warn record was evicted to make room"
        );
    }

    #[test]
    fn a_protected_producer_times_out_into_preallocated_loss_accounting() {
        let wait = Duration::from_millis(30);
        let queue = LogQueue::new_for_test_with_wait(2, 32, wait);
        queue.enqueue(LogPriority::Warn, line("warn-one"));
        queue.enqueue(LogPriority::Error, line("error-two"));

        let started = std::time::Instant::now();
        queue.enqueue(LogPriority::Error, line("error-timeout"));
        let elapsed = started.elapsed();
        assert!(
            elapsed >= wait,
            "the protected producer waits for its configured budget: {elapsed:?}"
        );
        assert!(
            elapsed < wait + Duration::from_millis(75),
            "the protected producer stays near its configured upper bound: {elapsed:?}"
        );

        queue.close();
        let batch = queue.take_batch();
        assert_eq!(
            batch
                .records
                .iter()
                .map(|record| record.line.as_ref())
                .collect::<Vec<_>>(),
            ["warn-one", "error-two"],
            "the timed-out record was never admitted"
        );
        assert_eq!(
            batch.summary.as_deref(),
            Some(
                "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
            ),
            "timeout loss uses the existing pressure summary storage"
        );
    }

    #[test]
    fn producer_deadline_includes_waiting_to_acquire_the_mutex() {
        let wait = Duration::from_millis(40);
        let queue = Arc::new(LogQueue::new_for_test_with_wait(2, 32, wait));
        queue.enqueue(LogPriority::Warn, line("warn-one"));
        queue.enqueue(LogPriority::Error, line("error-two"));

        let lock_queue = Arc::clone(&queue);
        let (locked_tx, locked_rx) = mpsc::sync_channel(0);
        let (release_tx, release_rx) = mpsc::channel();
        let lock_holder = std::thread::spawn(move || {
            lock_queue.hold_lock_for_test(&locked_tx, &release_rx);
        });
        locked_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the queue mutex is held");

        let producer_queue = Arc::clone(&queue);
        let (done_tx, done_rx) = mpsc::sync_channel(0);
        let producer = std::thread::spawn(move || {
            producer_queue.enqueue(LogPriority::Error, line("mutex-timeout"));
            done_tx.send(()).expect("report bounded producer");
        });
        let bounded = done_rx.recv_timeout(wait + Duration::from_millis(75));
        release_tx.send(()).expect("release queue mutex");
        lock_holder.join().expect("the lock holder joins");
        producer.join().expect("the producer joins");
        assert!(
            bounded.is_ok(),
            "mutex acquisition is part of the producer's {wait:?} budget"
        );

        queue.close();
        let batch = queue.take_batch();
        assert_eq!(
            batch.summary.as_deref(),
            Some(
                "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
            ),
            "a mutex-budget loss remains explicitly observable"
        );
    }

    #[test]
    fn a_batch_drains_256_records_in_global_sequence_order() {
        let queue = LogQueue::new();
        let priorities = [
            LogPriority::Error,
            LogPriority::Debug,
            LogPriority::Info,
            LogPriority::Warn,
            LogPriority::Trace,
        ];
        for index in 0..(BATCH * 2) {
            queue.enqueue(
                priorities[index % priorities.len()],
                line(&format!("record-{index}")),
            );
        }

        let batch = queue.take_batch();
        assert_eq!(
            batch.records.len(),
            BATCH,
            "one drain swaps a bounded batch"
        );
        assert!(!batch.done, "an open queue with records left is not done");
        for (position, record) in batch.records.iter().enumerate() {
            assert_eq!(
                record.sequence, position as u64,
                "lane heads are merged by smallest global sequence"
            );
            assert_eq!(
                &*record.line,
                format!("record-{position}"),
                "chronological order is retained across lanes"
            );
        }
        queue.close();
        let rest = drain_all(&queue);
        assert_eq!(rest.len(), BATCH, "the remainder drains after close");
    }

    #[test]
    fn sequence_follows_admission_when_a_prepared_producer_is_paused() {
        let queue = Arc::new(LogQueue::new_for_test(8, 128));
        let (pause, control) = crate::fault_injection::release_point();
        let paused_queue = Arc::clone(&queue);
        let paused = std::thread::spawn(move || {
            paused_queue.enqueue_after(
                LogPriority::Info,
                line("prepared-first"),
                FormatStatus::Complete,
                || pause.wait(),
            );
        });
        control
            .wait_until_reached(Duration::from_secs(5))
            .expect("the first producer pauses before admission");

        queue.enqueue(LogPriority::Warn, line("admitted-first"));
        control.release();
        paused.join().expect("the paused producer joins");
        queue.close();

        let records = drain_all(&queue);
        assert_eq!(
            records
                .iter()
                .map(|record| (record.sequence, record.line.as_ref()))
                .collect::<Vec<_>>(),
            [(0, "admitted-first"), (1, "prepared-first")],
            "sequence is assigned atomically with successful admission"
        );
    }

    #[test]
    fn abandonment_counts_admission_boundary_record_exactly_once() {
        let queue = Arc::new(LogQueue::new_for_test(8, 128));
        let producer_queue = Arc::clone(&queue);
        let (pause, control) = crate::fault_injection::release_point();
        let producer = std::thread::spawn(move || {
            producer_queue.enqueue_around(
                LogPriority::Warn,
                line("admitted-before-timeout"),
                FormatStatus::Complete,
                || {},
                || pause.wait(),
            );
        });
        control
            .wait_until_reached(Duration::from_secs(5))
            .expect("the producer pauses after admission");

        let loss = queue.abandon();
        assert_eq!(
            loss,
            ShutdownLoss {
                abandoned_records: 1,
                abandoned_summaries: 0,
                unreported_pressure_records: 0,
            },
            "one admitting producer is one undelivered record, not an active-plus-admitted double count"
        );
        control.release();
        producer.join().expect("the producer joins");
        assert_eq!(
            queue.shutdown_loss_for_test(),
            loss,
            "the persisted shutdown accounting keeps the same exact snapshot"
        );
    }

    #[test]
    fn variable_size_pressure_obeys_record_and_byte_peaks() {
        let count_queue = LogQueue::new_for_test(3, 100);
        for text in ["aaaa", "bb", "ccc"] {
            count_queue.enqueue(LogPriority::Debug, line(text));
        }
        count_queue.enqueue(LogPriority::Error, line("e"));
        assert_eq!(
            count_queue.accounting_for_test(),
            (3, 6, 9),
            "record capacity evicts one old Debug while byte accounting stays exact"
        );

        let byte_queue = LogQueue::new_for_test(8, 10);
        for text in ["aaaa", "bb", "ccc"] {
            byte_queue.enqueue(LogPriority::Debug, line(text));
        }
        byte_queue.enqueue(LogPriority::Error, line("1234567"));
        assert_eq!(
            byte_queue.accounting_for_test(),
            (2, 10, 10),
            "variable-size eviction admits only after enough exact bytes are freed"
        );
        byte_queue.close();
        let records = drain_all(&byte_queue);
        assert_eq!(
            records
                .iter()
                .map(|record| record.line.as_ref())
                .collect::<Vec<_>>(),
            ["ccc", "1234567"],
            "the oldest eligible records are evicted until the byte bound fits"
        );
    }

    #[test]
    fn pressure_summary_follows_the_admitted_tail_and_resets_for_a_second_episode() {
        let queue = LogQueue::new_for_test(600, 600);
        for _ in 0..600 {
            queue.enqueue(LogPriority::Debug, line("d"));
        }
        queue.enqueue_formatted(LogPriority::Error, line("e"), FormatStatus::Truncated);
        for _ in 0..4 {
            queue.enqueue(LogPriority::Error, line("e"));
        }

        let above_low_water = queue.take_batch();
        assert_eq!(above_low_water.records.len(), BATCH);
        assert!(
            above_low_water.summary.is_none(),
            "the episode remains open above both half-capacity thresholds"
        );
        let recovered = queue.take_batch();
        assert_eq!(recovered.records.len(), BATCH);
        assert!(
            recovered.summary.is_none(),
            "the summary waits behind records admitted before recovery"
        );
        assert_eq!(
            queue.accounting_for_test().0,
            600 - (BATCH * 2),
            "crossing low water leaves an admitted tail"
        );

        queue.enqueue(LogPriority::Info, line("admitted-after-recovery"));
        let admitted_tail = queue.take_batch();
        assert_eq!(
            admitted_tail.records.len(),
            600 - (BATCH * 2),
            "only records older than the pending summary drain"
        );
        assert!(
            admitted_tail
                .records
                .iter()
                .all(|record| record.line.as_ref() != "admitted-after-recovery"),
            "a later admission cannot move ahead of the pending summary"
        );
        assert_eq!(
            admitted_tail.summary.as_deref(),
            Some(
                "log pressure affected 6 record(s): dropped=5, debug=5, trace=0, info=0, truncated=1, rejected=0\n"
            ),
            "the first episode closes immediately after its admitted tail"
        );

        queue.enqueue(LogPriority::Error, line(&"x".repeat(601)));
        queue.close();
        let second_episode = queue.take_batch();
        assert_eq!(
            second_episode
                .records
                .iter()
                .map(|record| record.line.as_ref())
                .collect::<Vec<_>>(),
            ["admitted-after-recovery"],
            "the later admission follows the first summary"
        );
        assert_eq!(
            second_episode.summary.as_deref(),
            Some(
                "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
            ),
            "the oversized protected record starts a clean second episode"
        );
        assert!(second_episode.done);
    }

    #[test]
    fn a_byte_blocked_producer_wakes_after_drain() {
        let queue = Arc::new(LogQueue::new_for_test_with_wait(
            4,
            4,
            Duration::from_secs(1),
        ));
        queue.enqueue(LogPriority::Warn, line("wwww"));

        let drain_queue = Arc::clone(&queue);
        let (drain_prepared_tx, drain_prepared_rx) = mpsc::channel();
        let (drain_release_tx, drain_release_rx) = mpsc::channel();
        let (drain_done_tx, drain_done_rx) = mpsc::channel();
        let drain_waiter = std::thread::spawn(move || {
            drain_queue.enqueue_after(
                LogPriority::Debug,
                line("d"),
                FormatStatus::Complete,
                || {
                    drain_prepared_tx
                        .send(())
                        .expect("report prepared producer");
                    drain_release_rx.recv().expect("release prepared producer");
                },
            );
            drain_done_tx.send(()).expect("report drain wakeup");
        });
        drain_prepared_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the byte-blocked producer reaches admission");
        drain_release_tx
            .send(())
            .expect("release the byte-blocked producer");
        assert!(
            drain_done_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "free record slots do not bypass the aggregate byte bound"
        );

        let freed = queue.take_batch();
        assert_eq!(
            freed
                .records
                .iter()
                .map(|record| record.line.as_ref())
                .collect::<Vec<_>>(),
            ["wwww"]
        );
        drain_done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("draining bytes wakes the producer");
        drain_waiter.join().expect("the drain waiter joins");
        let admitted = queue.take_batch();
        assert_eq!(admitted.records[0].line.as_ref(), "d");
    }

    #[test]
    fn close_preaccounts_a_byte_blocked_protected_record() {
        let queue = Arc::new(LogQueue::new_for_test_with_wait(
            4,
            4,
            Duration::from_secs(1),
        ));
        queue.enqueue(LogPriority::Warn, line("wwww"));
        let close_queue = Arc::clone(&queue);
        let (close_prepared_tx, close_prepared_rx) = mpsc::channel();
        let (close_release_tx, close_release_rx) = mpsc::channel();
        let (close_done_tx, close_done_rx) = mpsc::channel();
        let close_waiter = std::thread::spawn(move || {
            close_queue.enqueue_after(
                LogPriority::Error,
                line("z"),
                FormatStatus::Complete,
                || {
                    close_prepared_tx
                        .send(())
                        .expect("report prepared producer");
                    close_release_rx.recv().expect("release prepared producer");
                },
            );
            close_done_tx.send(()).expect("report close wakeup");
        });
        close_prepared_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the closing producer reaches admission");
        close_release_tx
            .send(())
            .expect("release the closing producer");
        assert!(
            close_done_rx
                .recv_timeout(Duration::from_millis(200))
                .is_err(),
            "the producer blocks on bytes before close"
        );

        queue.close();
        close_done_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("close wakes the byte-blocked producer");
        close_waiter.join().expect("the close waiter joins");
        let remaining = queue.take_batch();
        assert_eq!(
            remaining
                .records
                .iter()
                .map(|record| record.line.as_ref())
                .collect::<Vec<_>>(),
            ["wwww"],
            "close wakes the producer without admitting its record"
        );
        assert_eq!(
            remaining.summary.as_deref(),
            Some(
                "log pressure affected 1 record(s): dropped=1, debug=0, trace=0, info=0, truncated=0, rejected=1\n"
            ),
            "close pre-accounts the protected record before waking its producer"
        );
        assert!(remaining.done);
    }

    #[test]
    fn pressure_emits_one_summary_after_the_queue_empties() {
        let queue = LogQueue::new();
        for index in 0..CAPACITY {
            queue.enqueue(LogPriority::Debug, line(&format!("debug-{index}")));
        }
        for index in 0..10 {
            queue.enqueue(LogPriority::Error, line(&format!("error-{index}")));
        }
        queue.close();

        let mut summaries = Vec::new();
        let mut total_records = 0;
        loop {
            let batch = queue.take_batch();
            total_records += batch.records.len();
            if let Some(summary) = batch.summary {
                summaries.push(summary);
            }
            if batch.done {
                break;
            }
        }
        assert_eq!(total_records, CAPACITY, "eviction keeps the queue full");
        assert_eq!(
            summaries.len(),
            1,
            "one synthetic summary per pressure episode: {summaries:?}"
        );
        assert!(
            summaries[0].contains("10 record(s)") && summaries[0].contains("debug=10"),
            "the summary counts evictions by level: {}",
            summaries[0]
        );
    }

    #[test]
    fn a_queue_without_evictions_emits_no_summary() {
        let queue = LogQueue::new();
        queue.enqueue(LogPriority::Info, line("only"));
        queue.close();
        let batch = queue.take_batch();
        assert!(batch.summary.is_none(), "no pressure, no summary");
        assert!(batch.done);
    }

    #[test]
    fn a_closed_queue_drops_new_records() {
        let queue = LogQueue::new();
        queue.enqueue(LogPriority::Info, line("before-close"));
        queue.close();
        queue.enqueue(LogPriority::Error, line("after-close"));
        let records = drain_all(&queue);
        assert_eq!(records.len(), 1, "admission is closed");
        assert_eq!(&*records[0].line, "before-close");
    }

    #[test]
    fn saturation_under_load_never_evicts_or_duplicates_warn_or_error() {
        const PRODUCERS: u64 = 4;
        const PER_PRODUCER: u64 = 10000;
        let queue = Arc::new(LogQueue::new_for_test_with_wait(
            CAPACITY,
            LOG_LIMITS.max_queued_bytes,
            Duration::from_secs(1),
        ));
        let (drained_tx, drained_rx) = mpsc::channel();
        let worker_queue = Arc::clone(&queue);
        let worker = std::thread::spawn(move || {
            loop {
                let batch = worker_queue.take_batch();
                for record in batch.records {
                    drained_tx
                        .send(record.line)
                        .expect("report the drained line");
                }
                if batch.done {
                    break;
                }
                // A slow sink: the producers outpace the drain, so the queue
                // fills and the eviction and blocking paths fire. The worker
                // never stops draining, so a blocked producer always wakes.
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        // Four producers push five times the capacity across every lane.
        let producers: Vec<_> = (0..PRODUCERS)
            .map(|id| {
                let queue = Arc::clone(&queue);
                std::thread::spawn(move || {
                    for index in 0..PER_PRODUCER {
                        let priority = match index % 5 {
                            0 => LogPriority::Debug,
                            1 => LogPriority::Trace,
                            2 => LogPriority::Info,
                            3 => LogPriority::Warn,
                            _ => LogPriority::Error,
                        };
                        queue.enqueue(priority, line(&format!("p{id}-{priority:?}-{index}")));
                    }
                })
            })
            .collect();
        for producer in producers {
            producer.join().expect("the producer joins");
        }
        queue.close();
        worker.join().expect("the worker joins");

        let drained: Vec<String> = drained_rx.iter().map(|line| line.to_string()).collect();
        let unique: std::collections::HashSet<&String> = drained.iter().collect();
        assert_eq!(
            drained.len(),
            unique.len(),
            "no record is written twice under saturation"
        );
        assert!(
            (drained.len() as u64) < PRODUCERS * PER_PRODUCER,
            "saturation really evicted: {} of {} records retained",
            drained.len(),
            PRODUCERS * PER_PRODUCER
        );
        for id in 0..PRODUCERS {
            for index in (3..PER_PRODUCER).step_by(5) {
                let warn = format!("p{id}-Warn-{index}");
                let error = format!("p{id}-Error-{}", index + 1);
                assert!(
                    unique.contains(&warn),
                    "Warn survives saturation: missing {warn}"
                );
                assert!(
                    unique.contains(&error),
                    "Error survives saturation: missing {error}"
                );
            }
        }
    }
}
