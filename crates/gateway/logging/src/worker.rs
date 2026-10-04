//! The worker thread, the segmented file sink with its stderr fallback,
//! and startup plus size-triggered log rotation.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Write as _};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;

use crate::config::{LOG_LIMITS, SEGMENT_TRUNCATION_MARKER};

mod compact;
mod lifecycle;
mod rotation;
#[cfg(test)]
mod tests;

pub(crate) use rotation::open_log_file;
use rotation::{prune_oldest_for, rotate_files};

#[derive(Debug, Clone, Copy)]
struct RotationLimits {
    segment: u64,
    aggregate: u64,
    terminal_record: u64,
}

#[derive(Debug, Clone, Copy)]
enum ReplacementMode {
    Atomic,
    RemoveThenRename,
}

impl ReplacementMode {
    const fn production() -> Self {
        if cfg!(windows) {
            Self::RemoveThenRename
        } else {
            Self::Atomic
        }
    }
}

#[derive(Debug, Default)]
struct FaultInjector {
    #[cfg(test)]
    fail_at: Option<usize>,
    #[cfg(test)]
    calls: usize,
    #[cfg(test)]
    simulated_crash: bool,
    #[cfg(test)]
    failed_operation: Option<&'static str>,
    #[cfg(test)]
    commit_marker_written: bool,
}

impl FaultInjector {
    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            clippy::unnecessary_wraps,
            reason = "in non-test builds the fault injector is inert: checkpoint ignores self and never fails"
        )
    )]
    fn checkpoint(&mut self, operation: &'static str) -> io::Result<()> {
        #[cfg(not(test))]
        let _ = operation;
        #[cfg(test)]
        {
            self.calls += 1;
            if self.fail_at == Some(self.calls) {
                self.fail_at = None;
                self.failed_operation = Some(operation);
                return Err(io::Error::other(format!(
                    "injected filesystem failure at {operation}"
                )));
            }
        }
        Ok(())
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "in non-test builds the injector state is cfg'd out, so the method ignores self"
        )
    )]
    fn is_simulated_crash(&self) -> bool {
        #[cfg(test)]
        {
            self.simulated_crash && self.failed_operation.is_some()
        }
        #[cfg(not(test))]
        {
            false
        }
    }

    #[cfg_attr(
        not(test),
        expect(
            clippy::unused_self,
            reason = "in non-test builds the injector state is cfg'd out, so the method ignores self"
        )
    )]
    fn record_commit_marker(&mut self) {
        #[cfg(test)]
        {
            self.commit_marker_written = true;
        }
    }

    #[cfg(test)]
    fn crashing(fail_at: usize) -> Self {
        Self {
            fail_at: Some(fail_at),
            simulated_crash: true,
            ..Self::default()
        }
    }

    #[cfg(test)]
    fn assert_selected(&self, fail_at: usize, transaction: &str) -> &'static str {
        assert_eq!(
            self.calls, fail_at,
            "only the selected filesystem checkpoint interrupts {transaction}"
        );
        self.failed_operation
            .unwrap_or_else(|| panic!("an injected {transaction} crash records its operation"))
    }
}

impl RotationLimits {
    const fn production() -> Self {
        Self {
            segment: LOG_LIMITS.segment_bytes,
            aggregate: LOG_LIMITS.aggregate_retained_bytes,
            terminal_record: LOG_LIMITS.max_formatted_record_bytes as u64,
        }
    }

    fn validate(self) -> io::Result<()> {
        let reserved = self
            .terminal_record
            .checked_add(SEGMENT_TRUNCATION_MARKER.len() as u64)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "log limits overflow"))?;
        if self.segment < reserved || self.aggregate < self.segment {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log limits cannot reserve one marked terminal record",
            ));
        }
        Ok(())
    }
}

fn existing_file_len(path: &Path) -> io::Result<Option<u64>> {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn artifact_path(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path
        .file_name()
        .map_or_else(|| "gateway.log".into(), std::ffi::OsStr::to_os_string);
    name.push(suffix);
    path.with_file_name(name)
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn sync_parent(path: &Path, fault: &mut FaultInjector) -> io::Result<()> {
    fault.checkpoint("sync parent directory")?;
    #[cfg(unix)]
    {
        File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

fn write_durable_file(path: &Path, contents: &[u8], fault: &mut FaultInjector) -> io::Result<()> {
    fault.checkpoint("create staged file")?;
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    fault.checkpoint("write staged file")?;
    file.write_all(contents)?;
    fault.checkpoint("sync staged file")?;
    file.sync_all()
}

#[derive(Debug)]
pub(crate) struct SegmentedFile {
    current: PathBuf,
    retained: Vec<PathBuf>,
    file: Option<BufWriter<File>>,
    current_bytes: u64,
    retained_bytes: u64,
    limits: RotationLimits,
}

impl SegmentedFile {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        let line_bytes = line.len() as u64;
        let marker_bytes = SEGMENT_TRUNCATION_MARKER.len() as u64;
        if line_bytes.saturating_add(marker_bytes) > self.limits.segment {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "formatted record exceeds the segment terminal reserve",
            ));
        }
        if self.current_bytes != 0
            && self
                .current_bytes
                .saturating_add(line_bytes)
                .saturating_add(marker_bytes)
                > self.limits.segment
        {
            self.ensure_aggregate_room(marker_bytes)?;
            self.write_bytes(SEGMENT_TRUNCATION_MARKER.as_bytes())?;
            self.flush()?;
            self.rotate()?;
        }
        self.ensure_aggregate_room(line_bytes)?;
        self.write_bytes(line.as_bytes())
    }

    fn write_bytes(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("active log segment is closed"))?
            .write_all(bytes)?;
        self.current_bytes = self.current_bytes.saturating_add(bytes.len() as u64);
        Ok(())
    }

    fn ensure_aggregate_room(&mut self, additional_bytes: u64) -> io::Result<()> {
        let required_retained_room = self.current_bytes.saturating_add(additional_bytes);
        prune_oldest_for(
            &self.retained,
            &mut self.retained_bytes,
            required_retained_room,
            self.limits.aggregate,
        )
    }

    fn rotate(&mut self) -> io::Result<()> {
        self.flush()?;
        self.file
            .as_ref()
            .ok_or_else(|| io::Error::other("active log segment is closed"))?
            .get_ref()
            .sync_all()?;
        drop(self.file.take());
        let result = rotate_files(
            &self.current,
            &self.retained,
            &mut FaultInjector::default(),
            ReplacementMode::production(),
        );
        self.file = OpenOptions::new()
            .append(true)
            .open(&self.current)
            .map(BufWriter::new)
            .map(Some)?;
        result?;
        self.retained_bytes = self.retained.iter().try_fold(0u64, |total, path| {
            Ok::<_, io::Error>(total.saturating_add(existing_file_len(path)?.unwrap_or(0)))
        })?;
        self.current_bytes = 0;
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("active log segment is closed"))?
            .flush()
    }
}

/// The drain target: the rotated log file until a write fails, then
/// synchronous stderr so records still land somewhere.
#[derive(Debug)]
enum Sink {
    Segmented(SegmentedFile),
    /// A plain file used only to isolate fallback and latency behavior from
    /// rotation in focused tests.
    #[cfg(test)]
    File(BufWriter<File>),
    Stderr,
    /// The latency test's baseline: every write accepted, nothing done.
    #[cfg(test)]
    Null,
    /// A controllable permanently stalled operation for shutdown tests.
    #[cfg(test)]
    Stalled {
        point: StallPoint,
        release: Option<crate::fault_injection::ReleasePoint>,
    },
}

#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum StallPoint {
    Write,
    Flush,
}

impl Sink {
    fn write_line(&mut self, line: &str) {
        match self {
            Self::Segmented(file) => {
                if let Err(error) = file.write_line(line) {
                    eprintln!(
                        "the log file rejected a write ({error}); logging falls back to stderr"
                    );
                    *self = Self::Stderr;
                    self.write_line(line);
                }
            }
            #[cfg(test)]
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
            #[cfg(test)]
            Self::Null => {}
            #[cfg(test)]
            Self::Stalled {
                point: StallPoint::Write,
                release,
            } => {
                if let Some(release) = release.take() {
                    release.wait();
                }
            }
            #[cfg(test)]
            Self::Stalled {
                point: StallPoint::Flush,
                ..
            } => {}
        }
    }

    fn flush(&mut self) {
        match self {
            Self::Segmented(file) => {
                if let Err(error) = file.flush() {
                    eprintln!(
                        "the log file rejected a flush ({error}); logging falls back to stderr"
                    );
                    *self = Self::Stderr;
                }
            }
            #[cfg(test)]
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
            #[cfg(test)]
            Self::Null => {}
            #[cfg(test)]
            Self::Stalled {
                point: StallPoint::Flush,
                release,
            } => {
                if let Some(release) = release.take() {
                    release.wait();
                }
            }
            #[cfg(test)]
            Self::Stalled {
                point: StallPoint::Write,
                ..
            } => {}
        }
    }

    /// Whether the sink has fallen back to stderr. Test seam for the
    /// file-failure fallback contract.
    #[cfg(test)]
    fn is_stderr(&self) -> bool {
        matches!(self, Self::Stderr)
    }
}

/// The worker owner: spawned by
/// [`LogRuntime::start`](crate::LogRuntime::start), then joined after a
/// healthy drain or detached after the shutdown budget expires.
#[derive(Debug)]
pub(crate) struct LogWorker {
    handle: JoinHandle<()>,
}

#[cfg(test)]
pub(crate) struct StalledSinkControl(crate::fault_injection::ReleaseControl);
