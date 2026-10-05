//! The launch-race lock: `gateway.json.lock` beside the gateway discovery file,
//! electing one launcher when two readers find no live gateway at the
//! same moment. The loser attaches to the winner.
//!
//! The protocol: the lock holder re-validates the gateway discovery file before
//! deciding, because a previous winner may have written it and released
//! the lock; a loser never deletes anything, it waits for the winner's
//! file to go live and attaches. Cleanup is the lock holder's privilege.
//! The OS-file-lock pattern mirrors gateway-local's `lock_artifact`: an
//! `OpenOptions` create plus a `std::fs::File` advisory lock, released by
//! dropping the handle.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::SidecarError;
use crate::paths::{instance_lock_file_path, lock_file_path};
use crate::stale::{self, GATEWAY_IMAGE_NAME, Resolution};
use crate::{CancellationToken, GatewayDiscoveryFile};

/// Delay between lock retries while a winner finishes its launch.
const RETRY_INTERVAL: Duration = Duration::from_millis(25);

/// The held launch lock. The holder launches the gateway; dropping the
/// guard releases the lock.
#[derive(Debug)]
pub struct LaunchLock {
    // The handle owns the OS lock; dropping it releases.
    _file: File,
}

/// The Gateway process-lifetime ownership lease.
///
/// This lock is distinct from [`LaunchLock`]: a Workshop parent may hold the
/// launch election while the spawned Gateway acquires this lease. The file
/// handle owns the operating-system lock, so dropping it or terminating its
/// process releases ownership without pid files or explicit cleanup.
#[derive(Debug)]
pub struct GatewayInstanceLease {
    // The handle owns the OS lock; dropping it releases.
    _file: File,
}

impl GatewayInstanceLease {
    /// Attempts to acquire process-lifetime Gateway ownership without
    /// blocking. `Ok(None)` means another process owns the lease.
    ///
    /// # Errors
    /// Returns [`SidecarError::CreateDir`] when the run directory cannot be
    /// created, or [`SidecarError::Lock`] when the lease file cannot be opened
    /// or an unexpected operating-system lock error occurs.
    pub fn try_acquire(run_dir: &Path) -> Result<Option<Self>, SidecarError> {
        std::fs::create_dir_all(run_dir).map_err(|source| SidecarError::CreateDir {
            path: run_dir.to_owned(),
            source,
        })?;
        let lock_path = instance_lock_file_path(run_dir);
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|source| SidecarError::Lock {
                path: lock_path.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Self { _file: file })),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(source)) => Err(SidecarError::Lock {
                path: lock_path,
                source,
            }),
        }
    }
}

/// What [`launch_or_attach`] decided.
#[derive(Debug)]
#[non_exhaustive]
pub enum LaunchDecision {
    /// This caller holds the launch lock and no live gateway exists:
    /// launch one. The lock stays held until the returned guard drops, so
    /// a concurrent racer attaches to the launched gateway instead of
    /// launching its own.
    Launch(LaunchLock),
    /// A live gateway already exists: attach to it.
    Attach(GatewayDiscoveryFile),
}

/// Settles a launch race in `run_dir`: returns [`LaunchDecision::Launch`]
/// with the held lock when no live gateway exists, or
/// [`LaunchDecision::Attach`] when one does - either already running, or
/// launched by the race winner while this caller waited. A loser waits up
/// to `timeout` for the winner's gateway to become attachable, and takes
/// the lock itself when the winner dies without writing one.
///
/// # Errors
/// Returns [`SidecarError::CreateDir`] when the run directory cannot be
/// created, [`SidecarError::Lock`] when the lock file cannot be opened or
/// an unexpected lock error occurs, [`SidecarError::LaunchTimeout`] when
/// no winner became attachable within `timeout`, and the [`crate::resolve`]
/// errors when the lock holder's own re-validation fails.
pub fn launch_or_attach(run_dir: &Path, timeout: Duration) -> Result<LaunchDecision, SidecarError> {
    launch_or_attach_named(run_dir, GATEWAY_IMAGE_NAME, timeout)
}

/// Settles a launch race while observing caller cancellation.
///
/// # Errors
/// Returns [`SidecarError::Cancelled`] when cancellation wins, plus the
/// file, lock, and timeout failures documented by [`launch_or_attach`].
pub fn launch_or_attach_cancellable(
    run_dir: &Path,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<LaunchDecision, SidecarError> {
    launch_or_attach_named_cancellable(run_dir, GATEWAY_IMAGE_NAME, timeout, cancellation)
}

/// [`launch_or_attach`] against a caller-named process image, so tests
/// can run the full liveness gauntlet from a test binary, which is never
/// named `promptforge-gateway`.
fn launch_or_attach_named(
    run_dir: &Path,
    image_name: &str,
    timeout: Duration,
) -> Result<LaunchDecision, SidecarError> {
    std::fs::create_dir_all(run_dir).map_err(|source| SidecarError::CreateDir {
        path: run_dir.to_owned(),
        source,
    })?;
    let lock_path = lock_file_path(run_dir);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| SidecarError::Lock {
            path: lock_path.clone(),
            source,
        })?;
    let deadline = Instant::now() + timeout;
    loop {
        match lock.try_lock() {
            Ok(()) => {
                // The lock is held; a previous winner may have written a
                // live file before releasing, so re-validate before
                // launching. As the holder, cleanup is ours.
                return Ok(match stale::resolve_named(run_dir, image_name)? {
                    Resolution::Attach(file) => LaunchDecision::Attach(file),
                    Resolution::Absent | Resolution::Stale(_) => {
                        LaunchDecision::Launch(LaunchLock { _file: lock })
                    }
                });
            }
            Err(TryLockError::WouldBlock) => {
                // Another process holds the lock and is mid-launch. Attach
                // as soon as its file goes live; never delete here.
                if let Ok(Some(file)) = GatewayDiscoveryFile::read(run_dir)
                    && stale::is_live(&file, image_name)
                {
                    return Ok(LaunchDecision::Attach(file));
                }
                if Instant::now() >= deadline {
                    return Err(SidecarError::LaunchTimeout { timeout });
                }
                std::thread::sleep(RETRY_INTERVAL);
            }
            Err(TryLockError::Error(source)) => {
                return Err(SidecarError::Lock {
                    path: lock_path.clone(),
                    source,
                });
            }
        }
    }
}

fn launch_or_attach_named_cancellable(
    run_dir: &Path,
    image_name: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
) -> Result<LaunchDecision, SidecarError> {
    launch_or_attach_named_cancellable_with(run_dir, image_name, timeout, cancellation, |delay| {
        cancellation.wait_timeout(delay)
    })
}

fn launch_or_attach_named_cancellable_with(
    run_dir: &Path,
    image_name: &str,
    timeout: Duration,
    cancellation: &CancellationToken,
    mut wait: impl FnMut(Duration) -> bool,
) -> Result<LaunchDecision, SidecarError> {
    if cancellation.is_cancelled() {
        return Err(SidecarError::Cancelled);
    }
    std::fs::create_dir_all(run_dir).map_err(|source| SidecarError::CreateDir {
        path: run_dir.to_owned(),
        source,
    })?;
    if cancellation.is_cancelled() {
        return Err(SidecarError::Cancelled);
    }
    let lock_path = lock_file_path(run_dir);
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .map_err(|source| SidecarError::Lock {
            path: lock_path.clone(),
            source,
        })?;
    let deadline = Instant::now() + timeout;
    loop {
        if cancellation.is_cancelled() {
            return Err(SidecarError::Cancelled);
        }
        match lock.try_lock() {
            Ok(()) => {
                let resolution =
                    stale::resolve_named_cancellable(run_dir, image_name, cancellation)?;
                if cancellation.is_cancelled() {
                    return Err(SidecarError::Cancelled);
                }
                return Ok(match resolution {
                    Resolution::Attach(file) => LaunchDecision::Attach(file),
                    Resolution::Absent | Resolution::Stale(_) => {
                        LaunchDecision::Launch(LaunchLock { _file: lock })
                    }
                });
            }
            Err(TryLockError::WouldBlock) => {
                if cancellation.is_cancelled() {
                    return Err(SidecarError::Cancelled);
                }
                if let Ok(Some(file)) = GatewayDiscoveryFile::read(run_dir)
                    && stale::is_live_cancellable(&file, image_name, cancellation)?
                {
                    if cancellation.is_cancelled() {
                        return Err(SidecarError::Cancelled);
                    }
                    return Ok(LaunchDecision::Attach(file));
                }
                if Instant::now() >= deadline {
                    return Err(SidecarError::LaunchTimeout { timeout });
                }
                if wait(RETRY_INTERVAL) {
                    return Err(SidecarError::Cancelled);
                }
            }
            Err(TryLockError::Error(source)) => {
                return Err(SidecarError::Lock {
                    path: lock_path.clone(),
                    source,
                });
            }
        }
    }
}

#[cfg(test)]
#[path = "lock-tests.rs"]
mod tests;
