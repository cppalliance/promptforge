//! Startup and size-triggered rotation of the retained log chain.

use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use super::compact::compact_oversized_segment;
use super::{
    FaultInjector, ReplacementMode, RotationLimits, SegmentedFile, artifact_path,
    existing_file_len, remove_file_if_present, sync_parent, write_durable_file,
};
use crate::config::LogConfig;

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

pub(super) fn open_log_file_with_limits(
    state_dir: &Path,
    limits: RotationLimits,
) -> io::Result<(PathBuf, SegmentedFile)> {
    limits.validate()?;
    let config = LogConfig::new(state_dir);
    let current = config.log_path();
    if let Some(logs) = current.parent() {
        std::fs::create_dir_all(logs)?;
    }
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

pub(super) fn rotation_committed_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-committed")
}

pub(super) fn rotation_staged_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-staged")
}

pub(super) fn rotation_prepared_path(current: &Path, old_mask: u8) -> PathBuf {
    artifact_path(current, &format!(".rotation-prepared-{old_mask:02x}"))
}

fn legacy_rotation_prepared_path(current: &Path) -> PathBuf {
    artifact_path(current, ".rotation-prepared")
}

pub(super) fn rotation_targets(current: &Path, retained: &[PathBuf]) -> Vec<PathBuf> {
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

pub(super) fn find_rotation_prepared(current: &Path) -> io::Result<Option<(PathBuf, u8)>> {
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

pub(super) fn cleanup_rotation_with(
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

pub(super) fn recover_rotation(current: &Path, retained: &[PathBuf]) -> io::Result<()> {
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

pub(super) fn rotate_files(
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

pub(super) fn prune_oldest_for(
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
