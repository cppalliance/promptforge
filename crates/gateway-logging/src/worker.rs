//! The worker thread, the file sink with its stderr fallback, and the log
//! rotation performed before the fresh file opens.

use std::fs::File;
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::queue::LogQueue;

/// Opens `<state_dir>/logs/gateway.log` fresh for this run, first rotating
/// an existing log to `gateway.log.1` and overwriting any older rotation,
/// so one previous run is kept and disk use stays bounded.
///
/// # Errors
/// Returns the I/O failure from creating the directory, rotating the
/// existing log, or opening the fresh one.
pub(crate) fn open_log_file(state_dir: &Path) -> io::Result<(PathBuf, File)> {
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let current = logs.join("gateway.log");
    let previous = logs.join("gateway.log.1");
    if current.is_file() {
        // A rename cannot overwrite an existing destination on Windows, so
        // the older rotation is removed first.
        if previous.is_file() {
            std::fs::remove_file(&previous)?;
        }
        std::fs::rename(&current, &previous)?;
    }
    let file = File::create(&current)?;
    Ok((current, file))
}

/// The drain target: the rotated log file until a write fails, then
/// synchronous stderr so records still land somewhere.
#[derive(Debug)]
enum Sink {
    File(BufWriter<File>),
    Stderr,
}

impl Sink {
    fn write_line(&mut self, line: &str) {
        match self {
            Self::File(file) => {
                if let Err(error) = file.write_all(line.as_bytes()) {
                    eprintln!(
                        "the log file rejected a write ({error}); logging falls back to stderr"
                    );
                    *self = Self::Stderr;
                    self.write_line(line);
                }
            }
            Self::Stderr => {
                let _ = io::stderr().lock().write_all(line.as_bytes());
            }
        }
    }

    fn flush(&mut self) {
        match self {
            Self::File(file) => {
                if let Err(error) = file.flush() {
                    eprintln!(
                        "the log file rejected a flush ({error}); logging falls back to stderr"
                    );
                    *self = Self::Stderr;
                }
            }
            Self::Stderr => {
                let _ = io::stderr().lock().flush();
            }
        }
    }
}

/// The worker owner: spawned by
/// [`LogRuntime::start`](crate::LogRuntime::start), joined by
/// [`LogRuntime::shutdown`](crate::LogRuntime::shutdown).
pub(crate) struct LogWorker;

impl LogWorker {
    /// Spawns the single worker thread. It blocks on the queue, swaps up to
    /// a batch of records into local storage, and performs every write and
    /// flush outside the mutex.
    ///
    /// # Errors
    /// Returns the I/O failure from spawning the thread.
    pub(crate) fn spawn(queue: Arc<LogQueue>, file: File) -> io::Result<JoinHandle<()>> {
        std::thread::Builder::new()
            .name("gateway-logging".to_string())
            .spawn(move || {
                let mut sink = Sink::File(BufWriter::new(file));
                loop {
                    let batch = queue.take_batch();
                    for record in &batch.records {
                        sink.write_line(&record.line);
                    }
                    if let Some(summary) = &batch.summary {
                        sink.write_line(summary);
                    }
                    sink.flush();
                    if batch.done {
                        break;
                    }
                }
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_log_rotation_keeps_one_previous_run() {
        let temp = TempStateDir::new("rotation");
        std::fs::create_dir_all(temp.0.join("logs")).expect("logs dir");
        std::fs::write(temp.0.join("logs/gateway.log"), "first run").expect("seed log");

        let (path, file) = open_log_file(&temp.0).expect("first rotation opens");
        drop(file);
        assert_eq!(path, temp.0.join("logs/gateway.log"));
        assert_eq!(
            std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("rotated log"),
            "first run",
            "the previous run's log rotates to .1"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("fresh log"),
            "",
            "the new run starts on a fresh file"
        );

        std::fs::write(&path, "second run").expect("write second run");
        let (_path, file) = open_log_file(&temp.0).expect("second rotation opens");
        drop(file);
        assert_eq!(
            std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("rotated log"),
            "second run",
            "a second rotation overwrites the older .1"
        );
    }
}
