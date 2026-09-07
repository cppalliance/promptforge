//! The owning handle: queues, sink, rotation, and the worker thread's
//! lifecycle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::config::{LOG_LIMITS, LogConfig};
use crate::error::LogError;
use crate::queue::{LogQueue, ShutdownLoss};
use crate::worker::{LogWorker, open_log_file};
use crate::writer::LogWriter;

const MAX_EMERGENCY_START_WAIT: Duration = Duration::from_millis(10);

/// The running log pipeline: the bounded queue, the rotated file sink, and
/// the worker thread that drains one to the other.
///
/// Created by [`start`](Self::start), cloned out as [`LogWriter`]s through
/// [`writer`](Self::writer), and closed by [`shutdown`](Self::shutdown),
/// which the caller runs last so a healthy sink receives final records
/// without allowing a stalled sink to hold process exit forever.
///
/// # Examples
/// ```
/// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-runtime-", env!("CARGO_PKG_VERSION")));
/// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
/// assert!(runtime.path().ends_with("gateway.log"));
/// runtime.shutdown()?;
/// # std::fs::remove_dir_all(&dir).ok();
/// # Ok::<(), gateway_logging::LogError>(())
/// ```
#[derive(Debug)]
pub struct LogRuntime {
    queue: Arc<LogQueue>,
    worker: Option<LogWorker>,
    path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownOutcome {
    Joined,
    Detached(ShutdownLoss),
}

impl LogRuntime {
    /// Rotates any existing log, opens a fresh `gateway.log` under
    /// `<state dir>/logs`, and spawns the single worker thread.
    ///
    /// # Errors
    /// Returns [`LogError`] when the logs directory cannot be created, the
    /// existing log cannot be rotated, the fresh file cannot be opened, or
    /// the worker thread cannot be spawned; classify with
    /// [`LogError::is_io`].
    ///
    /// # Examples
    /// ```no_run
    /// let runtime = gateway_logging::LogRuntime::start(
    ///     gateway_logging::LogConfig::new("/home/user/.promptforge"),
    /// )?;
    /// # Ok::<(), gateway_logging::LogError>(())
    /// ```
    pub fn start(config: LogConfig) -> Result<Self, LogError> {
        let state_dir = config.into_state_dir();
        let (path, file) = open_log_file(&state_dir)
            .map_err(|error| LogError::open(state_dir.join("logs/gateway.log"), error))?;
        let queue = Arc::new(LogQueue::new());
        let worker = LogWorker::spawn(Arc::clone(&queue), file).map_err(LogError::spawn)?;
        Ok(Self {
            queue,
            worker: Some(worker),
            path,
        })
    }

    /// A cloneable factory for the fmt layer's per-event writers.
    ///
    /// # Examples
    /// ```
    /// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-getwriter-", env!("CARGO_PKG_VERSION")));
    /// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
    /// let writer = runtime.writer();
    /// runtime.shutdown()?;
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), gateway_logging::LogError>(())
    /// ```
    #[must_use]
    pub fn writer(&self) -> LogWriter {
        LogWriter::new(Arc::clone(&self.queue))
    }

    /// The path of the log file this run writes.
    ///
    /// # Examples
    /// ```
    /// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-path-", env!("CARGO_PKG_VERSION")));
    /// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
    /// assert_eq!(runtime.path().file_name().and_then(|name| name.to_str()), Some("gateway.log"));
    /// runtime.shutdown()?;
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), gateway_logging::LogError>(())
    /// ```
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Closes admission and gives the worker the shared shutdown budget to
    /// drain and flush. A healthy sink preserves every admitted record and
    /// joins. After the budget expires, outstanding delivery is counted, an
    /// emergency stderr diagnostic is attempted on a detached helper, and
    /// the stalled worker is detached. Records enqueued after this call are
    /// dropped.
    ///
    /// # Errors
    /// Returns [`LogError`] when the worker thread panicked instead of
    /// draining cleanly.
    ///
    /// # Examples
    /// ```
    /// # let dir = std::env::temp_dir().join(concat!("gateway-logging-doc-shutdown-", env!("CARGO_PKG_VERSION")));
    /// let runtime = gateway_logging::LogRuntime::start(gateway_logging::LogConfig::new(&dir))?;
    /// runtime.shutdown()?;
    /// # std::fs::remove_dir_all(&dir).ok();
    /// # Ok::<(), gateway_logging::LogError>(())
    /// ```
    pub fn shutdown(self) -> Result<(), LogError> {
        self.shutdown_with(LOG_LIMITS.shutdown_wait, write_emergency_diagnostic)
            .map(|_| ())
    }

    fn shutdown_with(
        self,
        wait: Duration,
        emergency_diagnostic: impl FnOnce(ShutdownLoss) + Send + 'static,
    ) -> Result<ShutdownOutcome, LogError> {
        self.shutdown_with_waiter(wait, emergency_diagnostic, wait_for_worker_until)
    }

    fn shutdown_with_waiter(
        mut self,
        wait: Duration,
        emergency_diagnostic: impl FnOnce(ShutdownLoss) + Send + 'static,
        mut wait_for_worker: impl FnMut(&LogWorker, Instant) -> bool,
    ) -> Result<ShutdownOutcome, LogError> {
        let started = Instant::now();
        let shutdown_deadline = started.checked_add(wait).unwrap_or(started);
        let emergency_reserve = wait.min(MAX_EMERGENCY_START_WAIT);
        let worker_deadline = shutdown_deadline
            .checked_sub(emergency_reserve)
            .unwrap_or(started);

        if !self.queue.close_until(worker_deadline) {
            let loss = self.queue.abandon();
            attempt_emergency_diagnostic(loss, emergency_diagnostic, shutdown_deadline);
            return Ok(ShutdownOutcome::Detached(loss));
        }
        if let Some(worker) = self.worker.take() {
            if !wait_for_worker(&worker, worker_deadline) {
                let loss = self.queue.abandon();
                attempt_emergency_diagnostic(loss, emergency_diagnostic, shutdown_deadline);
                drop(worker);
                return Ok(ShutdownOutcome::Detached(loss));
            }
            worker.join().map_err(|_| LogError::worker_panicked())?;
        }
        Ok(ShutdownOutcome::Joined)
    }
}

fn wait_for_worker_until(worker: &LogWorker, deadline: Instant) -> bool {
    while !worker.is_finished() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::park_timeout(remaining.min(Duration::from_millis(1)));
    }
    true
}

/// Isolates stderr from the shutdown caller because a redirected console can
/// itself stall. The start handshake consumes only the reserved tail of the
/// shared shutdown budget. Failure to spawn leaves the loss accounted in the
/// queue without risking an unbounded fallback write.
fn attempt_emergency_diagnostic(
    loss: ShutdownLoss,
    diagnostic: impl FnOnce(ShutdownLoss) + Send + 'static,
    deadline: Instant,
) {
    let (started_tx, started_rx) = std::sync::mpsc::sync_channel(0);
    let spawned = std::thread::Builder::new()
        .name("gateway-log-emergency".to_string())
        .spawn(move || {
            let _ = started_tx.send(());
            diagnostic(loss);
        });
    if spawned.is_ok() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !remaining.is_zero() {
            let _ = started_rx.recv_timeout(remaining);
        }
    }
}

fn write_emergency_diagnostic(loss: ShutdownLoss) {
    use std::io::Write as _;

    let _ = writeln!(
        std::io::stderr().lock(),
        "gateway logging shutdown timed out: abandoned_records={}, abandoned_summaries={}, unreported_pressure_records={}",
        loss.abandoned_records,
        loss.abandoned_summaries,
        loss.unreported_pressure_records,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worker::StallPoint;
    use std::io::Write as _;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use tracing_subscriber::fmt::MakeWriter as _;

    struct TempStateDir(PathBuf);

    impl TempStateDir {
        fn new(test: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "gateway-logging-{test}-{}-{id}",
                std::process::id()
            ));
            std::fs::create_dir_all(&dir).expect("create the temp state dir");
            Self(dir)
        }
    }

    impl Drop for TempStateDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn shutdown_drains_flushes_and_joins() {
        let temp = TempStateDir::new("shutdown");
        let runtime = LogRuntime::start(LogConfig::new(&temp.0)).expect("start the runtime");
        let path = runtime.path().to_path_buf();
        for index in 0..600 {
            let mut event = runtime.writer().make_writer();
            writeln!(event, "record-{index}").expect("buffered write");
        }
        runtime.shutdown().expect("shutdown drains and joins");

        let contents = std::fs::read_to_string(&path).expect("read the log");
        for index in 0..600 {
            assert!(
                contents.contains(&format!("record-{index}")),
                "every queued record survived shutdown: missing record-{index}"
            );
        }
    }

    #[test]
    fn start_rotates_the_previous_run_log() {
        let temp = TempStateDir::new("start-rotation");
        std::fs::create_dir_all(temp.0.join("logs")).expect("logs dir");
        std::fs::write(temp.0.join("logs/gateway.log"), "previous run").expect("seed log");

        let runtime = LogRuntime::start(LogConfig::new(&temp.0)).expect("start the runtime");
        assert_eq!(
            std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("rotated log"),
            "previous run",
            "start rotates the previous run's log to .1"
        );
        runtime.shutdown().expect("shutdown");
    }

    #[test]
    fn shutdown_writes_every_record_in_sequence_before_the_join_returns() {
        let temp = TempStateDir::new("shutdown-order");
        let runtime = LogRuntime::start(LogConfig::new(&temp.0)).expect("start the runtime");
        let path = runtime.path().to_path_buf();
        for index in 0..300 {
            let mut event = runtime.writer().make_writer();
            writeln!(event, "ordered-{index}").expect("buffered write");
        }
        // After shutdown returns, the drain, the flush, and the join have
        // all completed: the file holds every record in enqueue order.
        runtime.shutdown().expect("shutdown drains and joins");

        let contents = std::fs::read_to_string(&path).expect("read the log");
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 300, "the flush preceded the join's return");
        for (position, line) in lines.iter().enumerate() {
            assert_eq!(
                *line,
                format!("ordered-{position}"),
                "the file's order is the global enqueue sequence"
            );
        }
    }

    fn assert_stalled_shutdown(point: StallPoint) {
        let queue = Arc::new(LogQueue::new_for_test_with_wait(
            8,
            128,
            Duration::from_millis(10),
        ));
        let (worker, stalled_sink) =
            LogWorker::spawn_stalled(Arc::clone(&queue), point).expect("spawn stalled worker");
        let runtime = LogRuntime {
            queue: Arc::clone(&queue),
            worker: Some(worker),
            path: PathBuf::from("stalled.log"),
        };

        queue.enqueue(crate::queue::LogPriority::Error, Box::from("in-flight"));
        stalled_sink
            .wait_until_stalled(Duration::from_secs(5))
            .expect("the sink stalls on the first admitted record");
        for index in 0..8 {
            queue.enqueue(
                crate::queue::LogPriority::Warn,
                format!("queued-{index}").into_boxed_str(),
            );
        }
        queue.enqueue(
            crate::queue::LogPriority::Error,
            Box::from("producer-timeout"),
        );

        let (diagnostic_tx, diagnostic_rx) = mpsc::sync_channel(1);
        let (release_diagnostic_tx, release_diagnostic_rx) = mpsc::channel();
        let (deadline_tx, deadline_rx) = mpsc::sync_channel(1);
        let (outcome_tx, outcome_rx) = mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            let outcome = runtime.shutdown_with_waiter(
                Duration::from_millis(40),
                move |diagnostic| {
                    diagnostic_tx
                        .send(diagnostic)
                        .expect("report emergency diagnostic");
                    release_diagnostic_rx
                        .recv()
                        .expect("hold the emergency sink stalled");
                },
                |worker, deadline| {
                    assert!(!worker.is_finished(), "the selected sink point is stalled");
                    deadline_tx
                        .send(deadline)
                        .expect("report the injected worker deadline");
                    false
                },
            );
            outcome_tx.send(outcome).expect("report shutdown outcome");
        });
        let outcome = outcome_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the outer watchdog observes bounded shutdown")
            .expect("a timeout is an accounted shutdown outcome");
        let worker_deadline = deadline_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("the deadline seam was exercised");
        assert!(
            worker_deadline.saturating_duration_since(Instant::now()) <= Duration::from_millis(30),
            "the worker receives no more than the configured budget minus emergency reserve"
        );

        let ShutdownOutcome::Detached(loss) = outcome else {
            panic!("the permanently stalled worker must detach");
        };
        assert_eq!(loss.abandoned_records, 9, "one in-flight and eight queued");
        assert_eq!(
            loss.abandoned_summaries, 1,
            "the unflushed pressure episode would have produced one summary"
        );
        assert_eq!(
            loss.unreported_pressure_records, 1,
            "the timed-out producer remains explicitly accounted"
        );
        assert_eq!(
            queue.shutdown_loss_for_test(),
            loss,
            "shutdown loss remains in preallocated queue accounting"
        );
        assert_eq!(
            diagnostic_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("the bounded emergency diagnostic is attempted"),
            loss,
            "the emergency diagnostic reports the accounted loss"
        );

        stalled_sink.release();
        release_diagnostic_tx
            .send(())
            .expect("release the emergency sink");
        shutdown.join().expect("the shutdown caller joins");
    }

    #[test]
    fn shutdown_deadline_detaches_a_stalled_write() {
        assert_stalled_shutdown(StallPoint::Write);
    }

    #[test]
    fn shutdown_deadline_detaches_a_stalled_flush() {
        assert_stalled_shutdown(StallPoint::Flush);
    }

    #[test]
    fn shutdown_deadline_includes_waiting_to_close_the_queue() {
        let wait = Duration::from_millis(40);
        let queue = Arc::new(LogQueue::new_for_test_with_wait(8, 128, wait));
        let (worker, stalled_sink) =
            LogWorker::spawn_stalled(Arc::clone(&queue), StallPoint::Write)
                .expect("spawn stalled worker");
        queue.enqueue(crate::queue::LogPriority::Error, Box::from("in-flight"));
        stalled_sink
            .wait_until_stalled(Duration::from_secs(5))
            .expect("the sink stalls outside the queue mutex");
        let runtime = LogRuntime {
            queue: Arc::clone(&queue),
            worker: Some(worker),
            path: PathBuf::from("lock-stalled.log"),
        };

        let lock_queue = Arc::clone(&queue);
        let (locked_tx, locked_rx) = mpsc::sync_channel(0);
        let (release_lock_tx, release_lock_rx) = mpsc::channel();
        let lock_holder = std::thread::spawn(move || {
            lock_queue.hold_lock_for_test(&locked_tx, &release_lock_rx);
        });
        locked_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the queue mutex is held");

        let (diagnostic_tx, diagnostic_rx) = mpsc::sync_channel(1);
        let (outcome_tx, outcome_rx) = mpsc::sync_channel(1);
        let shutdown = std::thread::spawn(move || {
            let outcome = runtime.shutdown_with(wait, move |diagnostic| {
                diagnostic_tx
                    .send(diagnostic)
                    .expect("report emergency diagnostic");
            });
            outcome_tx.send(outcome).expect("report shutdown outcome");
        });
        let bounded = outcome_rx.recv_timeout(wait + Duration::from_millis(75));
        release_lock_tx.send(()).expect("release queue mutex");
        lock_holder.join().expect("the lock holder joins");
        let outcome = bounded
            .expect("queue mutex acquisition stays inside the shutdown budget")
            .expect("lock contention becomes an accounted shutdown timeout");
        let ShutdownOutcome::Detached(loss) = outcome else {
            panic!("the mutex-stalled shutdown must detach");
        };
        assert_eq!(
            loss.abandoned_records, 1,
            "the in-flight record is accounted"
        );
        assert_eq!(
            loss.abandoned_summaries, 0,
            "no pressure summary was pending"
        );
        assert_eq!(
            diagnostic_rx
                .recv_timeout(Duration::from_secs(1))
                .expect("the emergency diagnostic receives the snapshot"),
            loss,
            "lock-free abandonment preserves exact accounting"
        );

        stalled_sink.release();
        shutdown.join().expect("the shutdown caller joins");
    }
}
