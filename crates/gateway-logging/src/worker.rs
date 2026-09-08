//! The worker thread, the segmented file sink with its stderr fallback,
//! and startup plus size-triggered log rotation.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use crate::config::{LOG_LIMITS, LogConfig, SEGMENT_TRUNCATION_MARKER};
use crate::queue::LogQueue;

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
    #[cfg_attr(not(test), allow(clippy::unused_self, clippy::unnecessary_wraps))]
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

    #[cfg_attr(not(test), allow(clippy::unused_self))]
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

    #[cfg_attr(not(test), allow(clippy::unused_self))]
    fn record_commit_marker(&mut self) {
        #[cfg(test)]
        {
            self.commit_marker_written = true;
        }
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

/// Opens `<state_dir>/logs/gateway.log` fresh and returns the worker-owned
/// segmented sink. Existing files are normalized before the current log is
/// shifted to `.1`, so the first write of a restarted process begins inside
/// the same segment and aggregate budgets used at runtime.
///
/// # Errors
/// Returns the I/O failure from creating the directory, normalizing or
/// rotating retained logs, or opening the fresh active segment.
pub(crate) fn open_log_file(state_dir: &Path) -> io::Result<(PathBuf, SegmentedFile)> {
    open_log_file_with_limits(state_dir, RotationLimits::production())
}

fn open_log_file_with_limits(
    state_dir: &Path,
    limits: RotationLimits,
) -> io::Result<(PathBuf, SegmentedFile)> {
    limits.validate()?;
    let config = LogConfig::new(state_dir);
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let current = config.log_path();
    let retained = config.retained_log_paths();
    recover_rotation(&current, &retained)?;

    compact_oversized_segment(&current, limits.segment)?;
    for path in &retained {
        compact_oversized_segment(path, limits.segment)?;
    }
    let mut retained_bytes = retained.iter().try_fold(0u64, |total, path| {
        Ok::<_, io::Error>(total.saturating_add(existing_file_len(path)?.unwrap_or(0)))
    })?;
    let current_bytes = existing_file_len(&current)?.unwrap_or(0);
    prune_oldest_for(
        &retained,
        &mut retained_bytes,
        current_bytes,
        limits.aggregate,
    )?;

    if current_bytes != 0 {
        rotate_files(
            &current,
            &retained,
            &mut FaultInjector::default(),
            ReplacementMode::production(),
        )?;
        retained_bytes = retained.iter().try_fold(0u64, |total, path| {
            Ok::<_, io::Error>(total.saturating_add(existing_file_len(path)?.unwrap_or(0)))
        })?;
    } else {
        File::create(&current)?.sync_all()?;
    }
    let file = OpenOptions::new().append(true).open(&current)?;
    let sink = SegmentedFile {
        current: current.clone(),
        retained,
        file: Some(BufWriter::new(file)),
        current_bytes: 0,
        retained_bytes,
        limits,
    };
    Ok((current, sink))
}

fn existing_file_len(path: &Path) -> io::Result<Option<u64>> {
    match path.metadata() {
        Ok(metadata) if metadata.is_file() => Ok(Some(metadata.len())),
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn compact_oversized_segment(path: &Path, segment_bytes: u64) -> io::Result<()> {
    compact_oversized_segment_with(
        path,
        segment_bytes,
        &mut FaultInjector::default(),
        ReplacementMode::production(),
    )
}

fn compact_oversized_segment_with(
    path: &Path,
    segment_bytes: u64,
    fault: &mut FaultInjector,
    replacement: ReplacementMode,
) -> io::Result<()> {
    recover_replacement(path)?;
    let Some(file_bytes) = existing_file_len(path)? else {
        return Ok(());
    };
    if file_bytes <= segment_bytes {
        return Ok(());
    }
    let payload_bytes = segment_bytes
        .checked_sub(SEGMENT_TRUNCATION_MARKER.len() as u64)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "segment marker exceeds budget")
        })?;
    let payload_len = usize::try_from(payload_bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "segment budget is too large"))?;
    let mut file = File::open(path)?;
    file.seek(io::SeekFrom::End(-i64::try_from(payload_bytes).map_err(
        |_| io::Error::new(io::ErrorKind::InvalidInput, "segment budget is too large"),
    )?))?;
    let mut tail = vec![0; payload_len];
    file.read_exact(&mut tail)?;
    let tail = valid_utf8_tail(&tail);
    let mut replacement_bytes = Vec::with_capacity(SEGMENT_TRUNCATION_MARKER.len() + tail.len());
    replacement_bytes.extend_from_slice(SEGMENT_TRUNCATION_MARKER.as_bytes());
    replacement_bytes.extend_from_slice(tail);
    durable_replace(path, &replacement_bytes, fault, replacement)
}

fn valid_utf8_tail(mut bytes: &[u8]) -> &[u8] {
    loop {
        match std::str::from_utf8(bytes) {
            Ok(_) => return bytes,
            Err(error) => match error.error_len() {
                Some(invalid_bytes) => {
                    bytes = &bytes[error.valid_up_to().saturating_add(invalid_bytes)..];
                }
                None => return &bytes[..error.valid_up_to()],
            },
        }
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

fn backup_file(source: &Path, backup: &Path, fault: &mut FaultInjector) -> io::Result<()> {
    fault.checkpoint("create rollback copy")?;
    if std::fs::hard_link(source, backup).is_ok() {
        return Ok(());
    }
    let building = artifact_path(backup, ".building");
    remove_file_if_present(&building)?;
    let mut source = File::open(source)?;
    let mut staged_backup = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&building)?;
    io::copy(&mut source, &mut staged_backup)?;
    staged_backup.sync_all()?;
    drop(staged_backup);
    std::fs::rename(building, backup)
}

fn install_file(
    target: &Path,
    staged: Option<&Path>,
    mode: ReplacementMode,
    fault: &mut FaultInjector,
) -> io::Result<()> {
    let Some(staged) = staged else {
        if existing_file_len(target)?.is_some() {
            fault.checkpoint("remove rotation target")?;
            std::fs::remove_file(target)?;
        }
        return Ok(());
    };
    if matches!(mode, ReplacementMode::RemoveThenRename) && existing_file_len(target)?.is_some() {
        fault.checkpoint("remove replacement target")?;
        std::fs::remove_file(target)?;
    }
    fault.checkpoint("install replacement")?;
    std::fs::rename(staged, target)
}

fn recover_replacement(path: &Path) -> io::Result<()> {
    let staged = artifact_path(path, ".compacting");
    let backup = artifact_path(path, ".compact-backup");
    remove_file_if_present(&artifact_path(&backup, ".building"))?;
    if backup.exists() {
        if path.exists() {
            remove_file_if_present(&backup)?;
        } else {
            std::fs::rename(&backup, path)?;
        }
    }
    remove_file_if_present(&staged)
}

fn rollback_replacement(path: &Path) -> io::Result<()> {
    let staged = artifact_path(path, ".compacting");
    let backup = artifact_path(path, ".compact-backup");
    remove_file_if_present(&artifact_path(&backup, ".building"))?;
    if backup.exists() {
        remove_file_if_present(path)?;
        std::fs::rename(&backup, path)?;
    }
    remove_file_if_present(&staged)?;
    sync_parent(path, &mut FaultInjector::default())
}

fn durable_replace(
    path: &Path,
    contents: &[u8],
    fault: &mut FaultInjector,
    mode: ReplacementMode,
) -> io::Result<()> {
    recover_replacement(path)?;
    let staged = artifact_path(path, ".compacting");
    let backup = artifact_path(path, ".compact-backup");
    let result = (|| {
        write_durable_file(&staged, contents, fault)?;
        backup_file(path, &backup, fault)?;
        sync_parent(path, fault)?;
        install_file(path, Some(&staged), mode, fault)?;
        sync_parent(path, fault)
    })();
    if let Err(error) = result {
        if fault.is_simulated_crash() {
            return Err(error);
        }
        return match rollback_replacement(path) {
            Ok(()) => Err(error),
            Err(rollback) => Err(io::Error::other(format!(
                "{error}; replacement rollback failed: {rollback}"
            ))),
        };
    }
    remove_file_if_present(&backup)?;
    sync_parent(path, &mut FaultInjector::default())
}

fn rotation_committed_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-committed")
}

fn rotation_staged_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-staged")
}

fn rotation_prepared_path(current: &Path, old_mask: u8) -> PathBuf {
    artifact_path(current, &format!(".rotation-prepared-{old_mask:02x}"))
}

fn legacy_rotation_prepared_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-prepared")
}

fn rotation_targets(current: &Path, retained: &[PathBuf]) -> Vec<PathBuf> {
    std::iter::once(current.to_path_buf())
        .chain(retained.iter().cloned())
        .collect()
}

fn rotation_old_mask(targets: &[PathBuf]) -> io::Result<u8> {
    targets
        .iter()
        .enumerate()
        .try_fold(0u8, |mask, (index, path)| {
            Ok(if existing_file_len(path)?.is_some() {
                mask | (1 << index)
            } else {
                mask
            })
        })
}

fn read_legacy_rotation_mask(path: &Path) -> io::Result<u8> {
    let bytes = std::fs::read(path)?;
    if bytes.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid log rotation recovery marker",
        ));
    }
    Ok(bytes[0])
}

fn find_rotation_prepared(current: &Path) -> io::Result<Option<(PathBuf, u8)>> {
    for old_mask in 0..64 {
        let path = rotation_prepared_path(current, old_mask);
        if path.exists() {
            return Ok(Some((path, old_mask)));
        }
    }
    let legacy = legacy_rotation_prepared_path(current);
    if legacy.exists() {
        return read_legacy_rotation_mask(&legacy).map(|old_mask| Some((legacy, old_mask)));
    }
    Ok(None)
}

fn remove_rotation_file(
    path: &Path,
    operation: &'static str,
    fault: &mut FaultInjector,
) -> io::Result<()> {
    if existing_file_len(path)?.is_some() {
        fault.checkpoint(operation)?;
        std::fs::remove_file(path)?;
    }
    Ok(())
}

fn cleanup_rotation_with(
    current: &Path,
    retained: &[PathBuf],
    fault: &mut FaultInjector,
) -> io::Result<()> {
    for target in rotation_targets(current, retained) {
        remove_rotation_file(
            &artifact_path(&target, ".rotation-new"),
            "cleanup staged rotation file",
            fault,
        )?;
        let backup = artifact_path(&target, ".rotation-old");
        remove_rotation_file(
            &artifact_path(&backup, ".building"),
            "cleanup partial rollback file",
            fault,
        )?;
        remove_rotation_file(&backup, "cleanup committed rollback file", fault)?;
    }
    remove_rotation_file(
        &rotation_staged_path(current),
        "cleanup staged rotation marker",
        fault,
    )?;
    sync_parent(current, fault)?;
    while let Some((prepared, _)) = find_rotation_prepared(current)? {
        remove_rotation_file(&prepared, "cleanup prepared rotation marker", fault)?;
    }
    sync_parent(current, fault)?;
    remove_rotation_file(
        &rotation_committed_path(current),
        "cleanup committed rotation marker",
        fault,
    )?;
    sync_parent(current, fault)
}

fn cleanup_rotation(current: &Path, retained: &[PathBuf]) -> io::Result<()> {
    cleanup_rotation_with(current, retained, &mut FaultInjector::default())
}

fn rotation_source_for<'a>(
    current: &'a Path,
    retained: &'a [PathBuf],
    destination_index: usize,
) -> &'a Path {
    if destination_index == 0 {
        current
    } else {
        &retained[destination_index - 1]
    }
}

fn rollback_rotation(
    current: &Path,
    retained: &[PathBuf],
    prepared: &Path,
    old_mask: u8,
) -> io::Result<()> {
    let targets = rotation_targets(current, retained);
    if rotation_staged_path(current).exists() {
        for (index, destination) in retained.iter().enumerate().rev() {
            let source = rotation_source_for(current, retained, index);
            let backup = artifact_path(source, ".rotation-old");
            if old_mask & (1 << index) != 0 && !backup.exists() && destination.exists() {
                std::fs::rename(destination, backup)?;
            }
        }
        remove_file_if_present(current)?;
    }
    for (index, target) in targets.iter().enumerate().rev() {
        let backup = artifact_path(target, ".rotation-old");
        remove_file_if_present(&artifact_path(&backup, ".building"))?;
        if backup.exists() {
            remove_file_if_present(target)?;
            std::fs::rename(&backup, target)?;
        } else if old_mask & (1 << index) == 0 {
            remove_file_if_present(target)?;
        }
    }
    for target in &targets {
        remove_file_if_present(&artifact_path(target, ".rotation-new"))?;
    }
    remove_file_if_present(&rotation_committed_path(current))?;
    remove_file_if_present(&rotation_staged_path(current))?;
    sync_parent(current, &mut FaultInjector::default())?;
    remove_file_if_present(prepared)?;
    sync_parent(current, &mut FaultInjector::default())
}

fn recover_rotation(current: &Path, retained: &[PathBuf]) -> io::Result<()> {
    if rotation_committed_path(current).exists() {
        return cleanup_rotation(current, retained);
    }
    let Some((prepared, old_mask)) = find_rotation_prepared(current)? else {
        return cleanup_rotation(current, retained);
    };
    if old_mask >= 64 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid log rotation recovery mask",
        ));
    }
    rollback_rotation(current, retained, &prepared, old_mask)
}

fn rotate_files(
    current: &Path,
    retained: &[PathBuf],
    fault: &mut FaultInjector,
    _mode: ReplacementMode,
) -> io::Result<()> {
    recover_rotation(current, retained)?;
    let targets = rotation_targets(current, retained);
    let old_mask = rotation_old_mask(&targets)?;
    let prepared = rotation_prepared_path(current, old_mask);
    let result = (|| {
        write_durable_file(&prepared, b"", fault)?;
        sync_parent(current, fault)?;
        for target in &targets {
            if existing_file_len(target)?.is_some() {
                fault.checkpoint("stage rotation source")?;
                std::fs::rename(target, artifact_path(target, ".rotation-old"))?;
            }
        }
        sync_parent(current, fault)?;
        write_durable_file(&rotation_staged_path(current), b"", fault)?;
        write_durable_file(&artifact_path(current, ".rotation-new"), b"", fault)?;
        sync_parent(current, fault)?;
        for (index, destination) in retained.iter().enumerate() {
            let source = rotation_source_for(current, retained, index);
            let staged = artifact_path(source, ".rotation-old");
            if staged.exists() {
                fault.checkpoint("install rotated segment")?;
                std::fs::rename(staged, destination)?;
            }
        }
        let staged_current = artifact_path(current, ".rotation-new");
        fault.checkpoint("install fresh active segment")?;
        std::fs::rename(staged_current, current)?;
        sync_parent(current, fault)?;
        write_durable_file(&rotation_committed_path(current), b"", fault)?;
        fault.record_commit_marker();
        sync_parent(current, fault)
    })();
    if let Err(error) = result {
        if fault.is_simulated_crash() {
            return Err(error);
        }
        return match rollback_rotation(current, retained, &prepared, old_mask) {
            Ok(()) => Err(error),
            Err(rollback) => Err(io::Error::other(format!(
                "{error}; rotation rollback failed: {rollback}"
            ))),
        };
    }
    cleanup_rotation_with(current, retained, fault)
}

fn prune_oldest_for(
    retained: &[PathBuf],
    retained_bytes: &mut u64,
    required_bytes: u64,
    aggregate_bytes: u64,
) -> io::Result<()> {
    while retained_bytes.saturating_add(required_bytes) > aggregate_bytes {
        let mut oldest = None;
        for path in retained.iter().rev() {
            if let Some(bytes) = existing_file_len(path)? {
                oldest = Some((path, bytes));
                break;
            }
        }
        let Some((oldest, removed)) = oldest else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "active log cannot fit the aggregate budget",
            ));
        };
        std::fs::remove_file(oldest)?;
        *retained_bytes = retained_bytes.saturating_sub(removed);
    }
    Ok(())
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
        entered: std::sync::mpsc::SyncSender<()>,
        release: Option<std::sync::mpsc::Receiver<()>>,
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
                entered,
                release,
            } => {
                if let Some(release) = release.take() {
                    let _ = entered.send(());
                    let _ = release.recv();
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
                entered,
                release,
            } => {
                if let Some(release) = release.take() {
                    let _ = entered.send(());
                    let _ = release.recv();
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

impl LogWorker {
    /// Spawns the single worker thread. It blocks on the queue, swaps up to
    /// a batch of records into local storage, and performs every write and
    /// flush outside the mutex.
    ///
    /// # Errors
    /// Returns the I/O failure from spawning the thread.
    pub(crate) fn spawn(queue: Arc<LogQueue>, file: SegmentedFile) -> io::Result<Self> {
        Self::spawn_with_sink(queue, Sink::Segmented(file))
    }

    fn spawn_with_sink(queue: Arc<LogQueue>, mut sink: Sink) -> io::Result<Self> {
        let handle = std::thread::Builder::new()
            .name("gateway-logging".to_string())
            .spawn(move || {
                loop {
                    let batch = queue.take_batch();
                    let records = batch.records.len();
                    let had_summary = batch.summary.is_some();
                    for record in &batch.records {
                        if queue.is_abandoned() {
                            return;
                        }
                        sink.write_line(&record.line);
                    }
                    if let Some(summary) = &batch.summary {
                        if queue.is_abandoned() {
                            return;
                        }
                        sink.write_line(summary);
                    }
                    if queue.is_abandoned() {
                        return;
                    }
                    sink.flush();
                    if queue.is_abandoned() {
                        return;
                    }
                    queue.complete_batch(records, had_summary, batch.summary_affected);
                    if batch.done {
                        break;
                    }
                }
            })?;
        Ok(Self { handle })
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.handle.is_finished()
    }

    pub(crate) fn join(self) -> std::thread::Result<()> {
        self.handle.join()
    }

    #[cfg(test)]
    pub(crate) fn spawn_stalled(
        queue: Arc<LogQueue>,
        point: StallPoint,
    ) -> io::Result<(Self, StalledSinkControl)> {
        let (entered_tx, entered_rx) = std::sync::mpsc::sync_channel(1);
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        let worker = Self::spawn_with_sink(
            queue,
            Sink::Stalled {
                point,
                entered: entered_tx,
                release: Some(release_rx),
            },
        )?;
        Ok((
            worker,
            StalledSinkControl {
                entered: entered_rx,
                release: release_tx,
            },
        ))
    }
}

#[cfg(test)]
pub(crate) struct StalledSinkControl {
    entered: std::sync::mpsc::Receiver<()>,
    release: std::sync::mpsc::SyncSender<()>,
}

#[cfg(test)]
impl StalledSinkControl {
    pub(crate) fn wait_until_stalled(
        &self,
        timeout: std::time::Duration,
    ) -> Result<(), std::sync::mpsc::RecvTimeoutError> {
        self.entered.recv_timeout(timeout)
    }

    pub(crate) fn release(self) {
        let _ = self.release.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::LogPriority;

    /// A file whose handle rejects writes, standing in for a disk
    /// failure: opened read-only, every write and flush errors.
    fn rejected_file(dir: &Path) -> File {
        let path = dir.join("rejected.log");
        std::fs::write(&path, "").expect("seed the file");
        std::fs::OpenOptions::new()
            .read(true)
            .open(&path)
            .expect("open read-only")
    }

    #[test]
    fn a_failed_file_write_falls_back_to_synchronous_stderr() {
        let temp = TempStateDir::new("sink-write-fallback");
        let mut sink = Sink::File(BufWriter::new(rejected_file(&temp.0)));
        assert!(!sink.is_stderr(), "the sink starts on the file");

        // A record larger than the buffer bypasses it and reaches the
        // rejecting handle immediately.
        let big = "x".repeat(16 * 1024);
        sink.write_line(&big);
        assert!(
            sink.is_stderr(),
            "a rejected write switches the sink to stderr"
        );
        sink.write_line("after the fallback\n");
        sink.flush();
        assert!(
            sink.is_stderr(),
            "the fallback keeps accepting records instead of failing"
        );
    }

    #[test]
    fn a_failed_file_flush_falls_back_to_synchronous_stderr() {
        let temp = TempStateDir::new("sink-flush-fallback");
        let mut sink = Sink::File(BufWriter::new(rejected_file(&temp.0)));

        // A small record sits in the buffer, so the write succeeds and
        // the flush is what the handle rejects.
        sink.write_line("buffered record\n");
        assert!(!sink.is_stderr(), "a buffered write has not failed yet");
        sink.flush();
        assert!(
            sink.is_stderr(),
            "a rejected flush switches the sink to stderr"
        );
    }

    /// Enqueues `records` lines and drains them through `sink` with the
    /// real worker's batch loop, returning the p95 enqueue-to-write
    /// latency. The enqueue instant is stamped before the record enters
    /// the queue, so the queue's sequence number indexes the stamps.
    fn measure_p95_enqueue_to_write(sink: Sink, records: usize) -> std::time::Duration {
        use std::sync::Mutex;
        use std::time::Instant;

        let queue = Arc::new(LogQueue::new());
        let stamps = Arc::new(Mutex::new(Vec::<Instant>::with_capacity(records)));
        let worker = {
            let queue = Arc::clone(&queue);
            let stamps = Arc::clone(&stamps);
            std::thread::spawn(move || {
                let mut sink = sink;
                let mut latencies = Vec::with_capacity(records);
                loop {
                    let batch = queue.take_batch();
                    for record in &batch.records {
                        sink.write_line(&record.line);
                        let written = Instant::now();
                        let index = usize::try_from(record.sequence).expect("sequence fits");
                        let enqueued = stamps.lock().expect("stamps mutex")[index];
                        latencies.push(written - enqueued);
                    }
                    if let Some(summary) = &batch.summary {
                        sink.write_line(summary);
                    }
                    sink.flush();
                    if batch.done {
                        break;
                    }
                }
                latencies
            })
        };
        for index in 0..records {
            stamps.lock().expect("stamps mutex").push(Instant::now());
            queue.enqueue(
                LogPriority::Info,
                Box::from(format!(
                    "latency probe {index}: a record of roughly the size a formatted event has\n"
                )),
            );
        }
        queue.close();
        let mut latencies = worker.join().expect("the worker joins");
        assert_eq!(
            latencies.len(),
            records,
            "every enqueued record was written"
        );
        let p95 = records * 95 / 100;
        latencies.select_nth_unstable(p95);
        latencies[p95]
    }

    #[test]
    #[ignore = "release-mode latency budget: run `cargo test -p gateway-logging --release -- --ignored`"]
    fn production_logging_stays_within_latency_budget() {
        use std::time::Duration;

        const RECORDS: usize = 20_000;

        let baseline = measure_p95_enqueue_to_write(Sink::Null, RECORDS);
        let temp = TempStateDir::new("latency");
        let (_path, file) = open_log_file(&temp.0).expect("open the production segmented sink");
        let file_sink = measure_p95_enqueue_to_write(Sink::Segmented(file), RECORDS);

        // The budget: less than 2% over the null-sink baseline, or 1 ms,
        // whichever is larger.
        let budget = (baseline / 50).max(Duration::from_millis(1));
        println!("p95 enqueue-to-write: null sink {baseline:?}, file sink {file_sink:?}");
        println!("budget: {budget:?} (2% of baseline or 1 ms, whichever is larger)");
        assert!(
            file_sink <= baseline + budget,
            "the file sink's p95 {file_sink:?} exceeds the baseline {baseline:?} by more than {budget:?}"
        );
    }

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
    fn the_log_rotation_retains_five_previous_runs() {
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

        // Five more runs fill the retained chain: after six rotations the
        // first run has shifted to .5 and every slot holds its run.
        for run in 2..=6u32 {
            std::fs::write(&path, format!("run {run}")).expect("write the run's log");
            let (_path, file) = open_log_file(&temp.0).expect("rotation opens");
            drop(file);
        }
        for run in 1..=5u32 {
            assert_eq!(
                std::fs::read_to_string(temp.0.join(format!("logs/gateway.log.{run}")))
                    .expect("retained log"),
                format!("run {}", 7 - run),
                ".{run} holds the run {n} log",
                n = 7 - run
            );
        }
        assert!(
            !temp.0.join("logs/gateway.log.6").exists(),
            "retention stops at five previous runs"
        );
    }

    #[test]
    fn the_sixth_previous_run_drops_off_the_retained_chain() {
        let temp = TempStateDir::new("rotation-drop");
        std::fs::create_dir_all(temp.0.join("logs")).expect("logs dir");

        // Seven runs: the two oldest must leave the chain entirely once
        // more than five previous runs exist.
        for run in 1..=7u32 {
            std::fs::write(temp.0.join("logs/gateway.log"), format!("run {run}"))
                .expect("write the run's log");
            let (_path, file) = open_log_file(&temp.0).expect("rotation opens");
            drop(file);
        }
        let retained: Vec<String> = (1..=5u32)
            .map(|run| {
                std::fs::read_to_string(temp.0.join(format!("logs/gateway.log.{run}")))
                    .expect("retained log")
            })
            .collect();
        assert_eq!(
            retained,
            vec!["run 7", "run 6", "run 5", "run 4", "run 3"],
            "the chain holds exactly the five newest previous runs"
        );
        assert!(
            !retained.iter().any(|contents| contents == "run 1"),
            "the sixth previous run is deleted, not retained"
        );
    }

    fn total_log_bytes(state_dir: &Path) -> u64 {
        let config = LogConfig::new(state_dir);
        std::iter::once(config.log_path())
            .chain(config.retained_log_paths())
            .map(|path| path.metadata().map_or(0, |metadata| metadata.len()))
            .sum()
    }

    fn total_directory_file_bytes(directory: &Path) -> u64 {
        std::fs::read_dir(directory)
            .expect("read log directory")
            .map(|entry| {
                entry
                    .expect("read log entry")
                    .metadata()
                    .expect("read log metadata")
                    .len()
            })
            .sum()
    }

    fn crashing_fault(fail_at: usize) -> FaultInjector {
        FaultInjector {
            fail_at: Some(fail_at),
            simulated_crash: true,
            ..FaultInjector::default()
        }
    }

    fn assert_injected_checkpoint(fault: &FaultInjector, fail_at: usize, transaction: &str) {
        assert_eq!(
            fault.calls, fail_at,
            "only the selected filesystem checkpoint interrupts {transaction}"
        );
        assert!(
            fault.failed_operation.is_some(),
            "an injected {transaction} crash records its filesystem operation"
        );
    }

    fn observed_rotation_commit(current: &Path, fault: &FaultInjector) -> bool {
        rotation_committed_path(current).exists() || fault.commit_marker_written
    }

    fn interrupted_final_commit_cleanup(current: &Path, fault: &FaultInjector) -> bool {
        fault.failed_operation == Some("sync parent directory")
            && fault.commit_marker_written
            && !rotation_committed_path(current).exists()
            && find_rotation_prepared(current)
                .expect("inspect prepared rotation marker")
                .is_none()
    }

    #[test]
    fn restart_compaction_recovers_every_injected_filesystem_failure() {
        let original = "old-prefix-".repeat(20) + "terminal diagnostic\n";
        let mut forced_replacement_gap = false;
        let mut completed = false;
        for (failures, fail_at) in (1..=32).enumerate() {
            let temp = TempStateDir::new("compaction-crash");
            let path = temp.0.join("gateway.log");
            std::fs::write(&path, &original).expect("seed oversized source");
            let mut fault = crashing_fault(fail_at);
            let result = compact_oversized_segment_with(
                &path,
                64,
                &mut fault,
                ReplacementMode::RemoveThenRename,
            );
            if result.is_ok() {
                completed = true;
                assert_eq!(
                    failures, fault.calls,
                    "the loop injected every staged replacement checkpoint independently"
                );
                break;
            }
            assert_injected_checkpoint(&fault, fail_at, "replacement");
            if fault.failed_operation == Some("install replacement") {
                forced_replacement_gap = true;
                assert!(
                    !path.exists() && artifact_path(&path, ".compact-backup").exists(),
                    "the forced Windows replacement gap retains the original rollback copy"
                );
            }
            recover_replacement(&path).expect("restart recovers compaction");
            let recovered = std::fs::read_to_string(&path).expect("one complete copy survives");
            assert!(
                recovered == original
                    || (recovered.starts_with(SEGMENT_TRUNCATION_MARKER)
                        && recovered.ends_with("terminal diagnostic\n")
                        && recovered.len() <= 64),
                "recovery keeps either the source or the complete durable replacement"
            );
        }
        assert!(
            forced_replacement_gap,
            "fault injection reaches the destructive Windows rename boundary"
        );
        assert!(
            completed,
            "the fault loop reaches the first non-failing run"
        );
    }

    #[test]
    fn live_rotation_recovers_every_injected_filesystem_failure() {
        let mut forced_staging_gap = false;
        let mut forced_commit_cleanup_gap = false;
        let mut completed = false;
        for (failures, fail_at) in (1..=128).enumerate() {
            let temp = TempStateDir::new("rotation-crash");
            let logs = temp.0.join("logs");
            std::fs::create_dir_all(&logs).expect("create logs");
            let current = logs.join("gateway.log");
            let retained = LogConfig::new(&temp.0).retained_log_paths();
            std::fs::write(&current, "active\n").expect("seed active");
            for (index, path) in retained.iter().enumerate() {
                std::fs::write(path, format!("old-{}\n", index + 1)).expect("seed retained");
            }
            let old: Vec<Vec<u8>> = std::iter::once(&current)
                .chain(retained.iter())
                .map(|path| std::fs::read(path).expect("snapshot old chain"))
                .collect();
            let disk_budget = old.iter().map(Vec::len).sum::<usize>() as u64;
            let mut fault = crashing_fault(fail_at);
            let result = rotate_files(
                &current,
                &retained,
                &mut fault,
                ReplacementMode::RemoveThenRename,
            );
            if result.is_ok() {
                completed = true;
                assert_eq!(
                    failures, fault.calls,
                    "the loop injected every rotation checkpoint independently"
                );
                assert!(
                    total_directory_file_bytes(&logs) <= disk_budget,
                    "a completed rotation stays inside the original aggregate bytes"
                );
                assert_eq!(std::fs::read(&current).expect("new active"), b"");
                for (index, path) in retained.iter().enumerate() {
                    assert_eq!(
                        std::fs::read(path).expect("new retained"),
                        old[index],
                        "the committed chain shifts each prior segment exactly once"
                    );
                }
                break;
            }
            assert_injected_checkpoint(&fault, fail_at, "rotation");
            assert!(
                total_directory_file_bytes(&logs) <= disk_budget,
                "transaction artifacts stay inside the aggregate budget at checkpoint {fail_at}"
            );
            // Cleanup removes the marker before its final parent sync. The
            // per-transaction state preserves that commit decision if that
            // exact sync is the injected crash boundary.
            let committed = observed_rotation_commit(&current, &fault);
            forced_commit_cleanup_gap |= interrupted_final_commit_cleanup(&current, &fault);
            if fault.failed_operation == Some("stage rotation source")
                && rotation_targets(&current, &retained).iter().any(|target| {
                    !target.exists() && artifact_path(target, ".rotation-old").exists()
                })
            {
                forced_staging_gap = true;
            }
            recover_rotation(&current, &retained).expect("restart recovers rotation");
            assert!(
                total_directory_file_bytes(&logs) <= disk_budget,
                "recovery stays inside the same aggregate disk budget"
            );
            if committed {
                assert_eq!(std::fs::read(&current).expect("committed active"), b"");
                for (index, path) in retained.iter().enumerate() {
                    assert_eq!(
                        std::fs::read(path).expect("committed retained"),
                        old[index],
                        "a durable commit marker keeps the complete new chain"
                    );
                }
            } else {
                for (index, path) in std::iter::once(&current).chain(retained.iter()).enumerate() {
                    assert_eq!(
                        std::fs::read(path).expect("rolled back chain"),
                        old[index],
                        "an uncommitted rotation restores every prior diagnostic name"
                    );
                }
            }
        }
        assert!(
            forced_staging_gap,
            "fault injection reaches an in-place staging boundary with the source preserved"
        );
        assert!(
            forced_commit_cleanup_gap,
            "fault injection reaches the final sync after commit-marker cleanup"
        );
        assert!(
            completed,
            "the fault loop reaches the first non-failing run"
        );
    }

    #[test]
    fn committed_sparse_rotation_survives_every_cleanup_crash_boundary() {
        let mut completed = false;
        for (failures, fail_at) in (1..=32).enumerate() {
            let temp = TempStateDir::new("sparse-cleanup-crash");
            let logs = temp.0.join("logs");
            std::fs::create_dir_all(&logs).expect("create logs");
            let config = LogConfig::new(&temp.0);
            let current = config.log_path();
            let retained = config.retained_log_paths();
            std::fs::write(&current, "").expect("seed fresh active");
            std::fs::write(&retained[0], "active\n").expect("seed shifted active");
            std::fs::write(&retained[2], "old-2\n").expect("seed sparse shifted segment");
            std::fs::write(&retained[4], "old-4\n").expect("seed sparse oldest destination");
            std::fs::write(artifact_path(&retained[4], ".rotation-old"), "old-5\n")
                .expect("seed pruned rollback segment");
            std::fs::write(artifact_path(&current, ".rotation-new"), "")
                .expect("seed stale empty stage");
            let old_mask = 1 | (1 << 2) | (1 << 4) | (1 << 5);
            std::fs::write(rotation_prepared_path(&current, old_mask), "")
                .expect("seed prepared marker");
            std::fs::write(rotation_staged_path(&current), "").expect("seed staged marker");
            std::fs::write(rotation_committed_path(&current), "").expect("seed commit marker");
            let disk_budget = total_directory_file_bytes(&logs);

            let mut fault = crashing_fault(fail_at);
            let result = cleanup_rotation_with(&current, &retained, &mut fault);
            if result.is_ok() {
                completed = true;
                assert_eq!(
                    failures, fault.calls,
                    "the loop injected every sparse cleanup checkpoint independently"
                );
            } else {
                assert_injected_checkpoint(&fault, fail_at, "cleanup");
                assert!(
                    total_directory_file_bytes(&logs) <= disk_budget,
                    "interrupted cleanup never duplicates segment bytes"
                );
                recover_rotation(&current, &retained).expect("restart completes committed cleanup");
            }

            assert_eq!(std::fs::read(&current).expect("active survives"), b"");
            assert_eq!(
                std::fs::read(&retained[0]).expect("newest survives"),
                b"active\n"
            );
            assert!(!retained[1].exists(), "the sparse .2 remains absent");
            assert_eq!(
                std::fs::read(&retained[2]).expect("sparse .3 survives"),
                b"old-2\n"
            );
            assert!(!retained[3].exists(), "the sparse .4 remains absent");
            assert_eq!(
                std::fs::read(&retained[4]).expect("sparse .5 survives"),
                b"old-4\n"
            );
            assert!(
                total_directory_file_bytes(&logs) < disk_budget,
                "the committed oldest rollback segment is pruned after recovery"
            );
            if completed {
                break;
            }
        }
        assert!(
            completed,
            "the fault loop reaches the first non-failing sparse cleanup"
        );
    }

    #[test]
    fn rotation_reserves_the_marker_and_preserves_the_terminal_record() {
        let temp = TempStateDir::new("segment-terminal");
        let limits = RotationLimits {
            segment: 64,
            aggregate: 128,
            terminal_record: 24,
        };
        let (path, sink) = open_log_file_with_limits(&temp.0, limits).expect("open segmented log");
        let queue = Arc::new(LogQueue::new());
        let worker =
            LogWorker::spawn(Arc::clone(&queue), sink).expect("spawn the production worker");
        queue.enqueue(LogPriority::Info, Box::from("ordinary-record-000\n"));
        queue.enqueue(LogPriority::Info, Box::from("ordinary-record-001\n"));
        queue.enqueue(LogPriority::Info, Box::from("gateway exiting\n"));
        queue.close();
        worker.join().expect("worker drains segmented sink");

        let retained =
            std::fs::read_to_string(temp.0.join("logs/gateway.log.1")).expect("retained segment");
        assert!(
            retained.ends_with(SEGMENT_TRUNCATION_MARKER),
            "the full segment ends with the reserved marker"
        );
        assert!(
            retained.len() as u64 <= limits.segment,
            "the retained segment obeys its fixed-size budget"
        );
        assert_eq!(
            std::fs::read_to_string(path).expect("active segment"),
            "gateway exiting\n",
            "the terminal record moves whole to the active segment"
        );
        assert!(
            total_log_bytes(&temp.0) <= limits.aggregate,
            "active and retained bytes stay inside one aggregate budget"
        );
    }

    #[test]
    fn byte_boundaries_rotate_a_full_numbered_chain_without_splitting_utf8() {
        let temp = TempStateDir::new("segment-byte-boundaries");
        let logs = temp.0.join("logs");
        std::fs::create_dir_all(&logs).expect("create logs");
        let config = LogConfig::new(&temp.0);
        let current = config.log_path();
        let retained = config.retained_log_paths();
        File::create(&current).expect("create active");
        for (index, path) in retained.iter().enumerate() {
            std::fs::write(path, format!("old-{}\n", index + 1)).expect("seed full chain");
        }
        let retained_bytes = retained
            .iter()
            .map(|path| path.metadata().expect("retained metadata").len())
            .sum();
        let limits = RotationLimits {
            segment: 48,
            aggregate: 48 * 6,
            terminal_record: 16,
        };
        let mut sink = SegmentedFile {
            current: current.clone(),
            retained: retained.clone(),
            file: Some(BufWriter::new(
                OpenOptions::new()
                    .append(true)
                    .open(&current)
                    .expect("open active"),
            )),
            current_bytes: 0,
            retained_bytes,
            limits,
        };
        let multibyte = "😀aaaaaaaaaaaaaa\n";
        let exact_boundary = "bbbbbbbbbbbbbbb\n";
        let maximum_terminal = "ccccccccccccccc\n";
        assert_eq!(multibyte.len(), 19);
        assert_eq!(exact_boundary.len(), 16);
        assert_eq!(
            u64::try_from(maximum_terminal.len()).expect("record length fits u64"),
            limits.terminal_record
        );

        sink.write_line(multibyte).expect("write multibyte record");
        sink.write_line(exact_boundary)
            .expect("exact byte boundary stays in the active segment");
        sink.flush().expect("flush exact boundary");
        assert_eq!(
            std::fs::read_to_string(&current).expect("read exact active"),
            format!("{multibyte}{exact_boundary}"),
            "equality with the reserved marker does not rotate"
        );

        sink.write_line(maximum_terminal)
            .expect("one byte over rotates before the maximum terminal record");
        sink.flush().expect("flush terminal");
        let newest =
            std::fs::read_to_string(&retained[0]).expect("newest retained remains valid UTF-8");
        assert_eq!(
            newest,
            format!("{multibyte}{exact_boundary}{SEGMENT_TRUNCATION_MARKER}")
        );
        assert_eq!(newest.len() as u64, limits.segment);
        assert_eq!(
            std::fs::read_to_string(&current).expect("active terminal"),
            maximum_terminal
        );
        for (index, path) in retained.iter().enumerate().skip(1) {
            assert_eq!(
                std::fs::read_to_string(path).expect("shifted retained"),
                format!("old-{index}\n"),
                "the complete numbered chain shifts oldest-first"
            );
        }
        assert!(
            !std::fs::read_to_string(&retained[retained.len() - 1])
                .expect("oldest retained")
                .contains("old-5"),
            "the prior oldest segment is pruned only after its replacement is durable"
        );
        assert!(
            total_log_bytes(&temp.0) <= limits.aggregate,
            "all named segments remain within the aggregate byte budget"
        );
    }

    #[test]
    fn restart_caps_legacy_segments_and_prunes_oldest_before_admission() {
        let temp = TempStateDir::new("segment-restart");
        let logs = temp.0.join("logs");
        std::fs::create_dir_all(&logs).expect("logs dir");
        let terminal = "gateway exiting after a fatal error\n";
        std::fs::write(
            logs.join("gateway.log"),
            format!("{}{}", "😀".repeat(40), terminal),
        )
        .expect("seed oversized active log");
        std::fs::write(logs.join("gateway.log.1"), "newer-retained".repeat(4))
            .expect("seed newer retained log");
        std::fs::write(logs.join("gateway.log.2"), "oldest-retained".repeat(4))
            .expect("seed oldest retained log");
        let limits = RotationLimits {
            segment: 64,
            aggregate: 80,
            terminal_record: 40,
        };

        let (_path, mut first) =
            open_log_file_with_limits(&temp.0, limits).expect("normalize first restart");
        first
            .write_line("first restart\n")
            .expect("write after first restart");
        first.flush().expect("flush first restart");
        let normalized =
            std::fs::read_to_string(logs.join("gateway.log.1")).expect("normalized legacy segment");
        assert!(
            normalized.starts_with(SEGMENT_TRUNCATION_MARKER),
            "an oversized legacy segment records the omitted prefix"
        );
        assert!(
            normalized.ends_with(terminal),
            "tail compaction reserves enough room for the prior terminal record"
        );
        drop(first);
        let (_path, mut second) =
            open_log_file_with_limits(&temp.0, limits).expect("normalize second restart");
        second
            .write_line("second restart\n")
            .expect("write after second restart");
        second.flush().expect("flush second restart");

        let config = LogConfig::new(&temp.0);
        for path in std::iter::once(config.log_path()).chain(config.retained_log_paths()) {
            let bytes = path.metadata().map_or(0, |metadata| metadata.len());
            assert!(
                bytes <= limits.segment,
                "{} exceeded the segment budget with {bytes} bytes",
                path.display()
            );
        }
        assert!(
            total_log_bytes(&temp.0) <= limits.aggregate,
            "restart normalization and later writes preserve the aggregate budget"
        );
        assert!(
            !logs.join("gateway.log.2").exists(),
            "oldest segments are pruned before newer bytes are admitted"
        );
        let retained =
            std::fs::read_to_string(logs.join("gateway.log.1")).expect("newest retained segment");
        assert!(
            retained.contains("first restart"),
            "the current numbered diagnostic name retains the newest prior segment"
        );
    }
}
