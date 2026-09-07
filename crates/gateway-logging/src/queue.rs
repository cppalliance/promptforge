//! The bounded priority queue: one deque per level under one mutex, a fixed
//! total capacity, and eviction rules that protect Warn and Error records.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};

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
    next_sequence: AtomicU64,
}

#[derive(Debug)]
struct State {
    lanes: [VecDeque<LogRecord>; 5],
    len: usize,
    closed: bool,
    loss: LossCounts,
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

    fn take_summary(&mut self) -> Option<Box<str>> {
        if self.is_empty() {
            return None;
        }
        let dropped: u64 = self.evicted.iter().sum::<u64>() + self.rejected;
        let affected = dropped + self.truncated;
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
        Some(summary)
    }
}

impl State {
    fn push(&mut self, record: LogRecord) {
        self.lanes[record.priority.lane()].push_back(record);
        self.len += 1;
    }

    /// Evicts the oldest record the incoming priority is allowed to
    /// displace, counting the eviction by the evicted record's level.
    fn evict_for(&mut self, priority: LogPriority) -> Option<LogRecord> {
        for &lane_priority in priority.evictable() {
            if let Some(record) = self.lanes[lane_priority.lane()].pop_front() {
                self.loss.evicted[record.priority.lane()] += 1;
                self.len -= 1;
                return Some(record);
            }
        }
        None
    }

    /// Pops the lane head with the smallest global sequence, so drained
    /// output stays chronological across lanes.
    fn pop_oldest(&mut self) -> Option<LogRecord> {
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
        let index = oldest?;
        let record = self.lanes[index].pop_front();
        self.len -= 1;
        record
    }

    /// Builds the one synthetic summary of a pressure episode and resets
    /// the counters; `None` when no content was lost since the last
    /// summary.
    fn take_summary(&mut self) -> Option<Box<str>> {
        self.loss.take_summary()
    }
}

impl LogQueue {
    pub(crate) fn new() -> Self {
        Self {
            state: Mutex::new(State {
                lanes: std::array::from_fn(|_| VecDeque::new()),
                len: 0,
                closed: false,
                loss: LossCounts::default(),
            }),
            work_available: Condvar::new(),
            space_available: Condvar::new(),
            next_sequence: AtomicU64::new(0),
        }
    }

    /// Enqueues `line`, assigning its global sequence before the lock is
    /// taken so formatting and allocation never happen under the mutex. On
    /// a full queue the oldest eligible lower-priority record is evicted;
    /// with none eligible the producer blocks on the condition variable
    /// until the worker frees space. After [`close`](Self::close) new
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
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if state.closed {
                return;
            }
            if state.len < CAPACITY {
                if status == FormatStatus::Truncated {
                    state.loss.truncated += 1;
                }
                state.push(LogRecord {
                    sequence,
                    priority,
                    line,
                });
                drop(state);
                self.work_available.notify_one();
                return;
            }
            if state.evict_for(priority).is_some() {
                if status == FormatStatus::Truncated {
                    state.loss.truncated += 1;
                }
                state.push(LogRecord {
                    sequence,
                    priority,
                    line,
                });
                drop(state);
                self.work_available.notify_one();
                return;
            }
            state = self
                .space_available
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// Rejects invalid formatter bytes and wakes the worker so the loss is
    /// observable even when no queue record accompanies it.
    pub(crate) fn reject_formatted(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed {
            return;
        }
        state.loss.rejected += 1;
        drop(state);
        self.work_available.notify_one();
    }

    /// Blocks until records are available (or the queue is closed and
    /// drained), moves up to [`BATCH`] of them out in global sequence
    /// order, and attaches the pressure summary when the queue empties
    /// after evictions. Every write happens on the caller's side, outside
    /// the mutex.
    pub(crate) fn take_batch(&self) -> Batch {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if state.len == 0 && state.loss.is_empty() && !state.closed {
                state = self
                    .work_available
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
                continue;
            }
            let mut records = Vec::with_capacity(BATCH.min(state.len));
            while records.len() < BATCH {
                let Some(record) = state.pop_oldest() else {
                    break;
                };
                records.push(record);
            }
            let summary = if state.len == 0 {
                state.take_summary()
            } else {
                None
            };
            let done = state.closed && state.len == 0;
            drop(state);
            self.space_available.notify_all();
            return Batch {
                records,
                summary,
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

    /// Closes admission and wakes every waiter: producers drop new
    /// records, blocked producers return, and the worker exits once the
    /// queue drains.
    pub(crate) fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        drop(state);
        self.work_available.notify_all();
        self.space_available.notify_all();
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
        let queue = Arc::new(LogQueue::new());
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
        let queue = Arc::new(LogQueue::new());
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
