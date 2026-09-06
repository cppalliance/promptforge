//! The owning handle: queues, sink, rotation, and the worker thread's
//! lifecycle.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::config::LogConfig;
use crate::error::LogError;
use crate::queue::LogQueue;
use crate::worker::{LogWorker, open_log_file};
use crate::writer::LogWriter;

/// The running log pipeline: the bounded queue, the rotated file sink, and
/// the worker thread that drains one to the other.
///
/// Created by [`start`](Self::start), cloned out as [`LogWriter`]s through
/// [`writer`](Self::writer), and closed by [`shutdown`](Self::shutdown),
/// which the caller runs last so the final records still reach the disk.
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
    worker: Option<JoinHandle<()>>,
    path: PathBuf,
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

    /// Closes admission, drains every queued record, flushes the sink, and
    /// joins the worker thread. Records enqueued after this call are
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
    pub fn shutdown(mut self) -> Result<(), LogError> {
        self.queue.close();
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| LogError::worker_panicked())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;
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
}
