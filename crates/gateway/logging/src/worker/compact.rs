//! Startup compaction of oversized segments through a durable, rollback-safe replacement.

use std::fs::{File, OpenOptions};
use std::io::{self, Read as _, Seek as _};
use std::path::Path;

use super::{
    FaultInjector, ReplacementMode, artifact_path, existing_file_len, remove_file_if_present,
    sync_parent, write_durable_file,
};
use crate::config::SEGMENT_TRUNCATION_MARKER;

pub(super) fn compact_oversized_segment(path: &Path, segment_bytes: u64) -> io::Result<()> {
    compact_oversized_segment_with(
        path,
        segment_bytes,
        &mut FaultInjector::default(),
        ReplacementMode::production(),
    )
}

pub(super) fn compact_oversized_segment_with(
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

pub(super) fn recover_replacement(path: &Path) -> io::Result<()> {
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
