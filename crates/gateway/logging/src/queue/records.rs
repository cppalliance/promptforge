//! The value types the queue exchanges with producers, the worker, and shutdown.

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
    pub(super) fn lane(self) -> usize {
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
    pub(super) fn evictable(self) -> &'static [LogPriority] {
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

/// Records and already-built summaries that could not be delivered before
/// the shutdown budget expired. The counters sit in queue state from
/// construction, so recording a timeout never allocates.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ShutdownLoss {
    pub(crate) abandoned_records: u64,
    pub(crate) abandoned_summaries: u64,
    pub(crate) unreported_pressure_records: u64,
}
