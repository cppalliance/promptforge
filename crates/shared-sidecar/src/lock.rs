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
pub(crate) fn launch_or_attach_named(
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
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;
    use std::sync::mpsc;

    /// The test process's own image name, so the pid and image checks
    /// pass and the loser reaches the probe path.
    fn own_image_name() -> String {
        std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("the exe has a file name")
            .to_string_lossy()
            .into_owned()
    }

    /// A fixture gateway answering health and the presented key with 200.
    fn fixture_gateway() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                for _ in 0..2 {
                    let mut buffer = [0u8; 1024];
                    if stream.read(&mut buffer).is_err() {
                        break;
                    }
                    if stream
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}")
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        port
    }

    /// A live gateway discovery file on the fixture gateway, owned by the test
    /// process.
    fn live_file(port: u16) -> GatewayDiscoveryFile {
        GatewayDiscoveryFile {
            port,
            api_key: "key".to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn an_empty_run_dir_elects_a_launcher() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let decision = launch_or_attach_named(dir.path(), "irrelevant", Duration::from_secs(1))
            .expect("the race settles");
        assert!(
            matches!(decision, LaunchDecision::Launch(_)),
            "no file means this caller launches"
        );
        assert!(
            lock_file_path(dir.path()).exists(),
            "the lock file was created"
        );
    }

    #[test]
    fn process_ownership_is_separate_from_parent_launch_election() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let lease = GatewayInstanceLease::try_acquire(dir.path())
            .expect("attempt the process lease")
            .expect("the first process acquires its lease");
        let decision = launch_or_attach_named(dir.path(), "irrelevant", Duration::from_millis(100))
            .expect("the independent parent launch election settles");
        assert!(
            matches!(decision, LaunchDecision::Launch(_)),
            "holding the process lease never contends with LaunchLock"
        );
        assert!(
            GatewayInstanceLease::try_acquire(dir.path())
                .expect("contend for process ownership")
                .is_none(),
            "a second process lease cannot coexist"
        );
        drop(lease);
        assert!(
            GatewayInstanceLease::try_acquire(dir.path())
                .expect("reacquire released process ownership")
                .is_some(),
            "dropping the handle releases ownership"
        );
    }

    #[test]
    fn a_live_file_attaches_without_contesting_the_lock() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = live_file(fixture_gateway());
        file.write_to(dir.path()).expect("write");

        let decision =
            launch_or_attach_named(dir.path(), &own_image_name(), Duration::from_secs(5))
                .expect("the race settles");
        match decision {
            LaunchDecision::Attach(attached) => assert_eq!(attached, file),
            LaunchDecision::Launch(_) => panic!("a live gateway must be attached, not relaunched"),
        }
    }

    #[test]
    fn the_race_loser_attaches_to_the_winners_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let image = own_image_name();
        // The winner holds the lock with no file written yet.
        let LaunchDecision::Launch(winner) =
            launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
                .expect("the first caller wins the lock")
        else {
            panic!("an empty run dir elects a launcher");
        };

        // The loser waits; the winner's file appears mid-wait.
        let loser_dir = dir.path().to_owned();
        let loser = std::thread::spawn(move || {
            launch_or_attach_named(&loser_dir, &image, Duration::from_secs(10))
                .expect("the loser's race settles")
        });
        std::thread::sleep(Duration::from_millis(100));
        let file = live_file(fixture_gateway());
        file.write_to(dir.path()).expect("the winner writes");

        match loser.join().expect("the loser thread ran") {
            LaunchDecision::Attach(attached) => assert_eq!(attached, file),
            LaunchDecision::Launch(_) => {
                panic!("the loser must attach to the winner, not launch a second gateway")
            }
        }
        drop(winner);
    }

    #[test]
    fn a_loser_becomes_the_launcher_when_the_winner_dies_silent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let image = own_image_name();
        {
            let LaunchDecision::Launch(winner) =
                launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
                    .expect("the first caller wins the lock")
            else {
                panic!("an empty run dir elects a launcher");
            };
            // The winner dies (or gives up) without writing a file.
            drop(winner);
        }

        let decision = launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
            .expect("the race settles");
        assert!(
            matches!(decision, LaunchDecision::Launch(_)),
            "a dead winner's lock passes to the next launcher"
        );
    }

    #[test]
    fn a_loser_times_out_when_the_winner_stays_silent() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let image = own_image_name();
        let LaunchDecision::Launch(_winner) =
            launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
                .expect("the first caller wins the lock")
        else {
            panic!("an empty run dir elects a launcher");
        };

        let error = launch_or_attach_named(dir.path(), &image, Duration::from_millis(150))
            .expect_err("a silent winner starves the loser");
        assert!(
            matches!(error, SidecarError::LaunchTimeout { .. }),
            "the loser reports the timeout: {error}"
        );
    }

    #[test]
    fn cancellation_joins_a_blocked_launch_race_without_launching() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let image = own_image_name();
        let LaunchDecision::Launch(winner) =
            launch_or_attach_named(dir.path(), &image, Duration::from_secs(5))
                .expect("the first caller wins the lock")
        else {
            panic!("an empty run dir elects a launcher");
        };
        let cancellation = crate::CancellationToken::new();
        let worker_cancellation = cancellation.clone();
        let run_dir = dir.path().to_owned();
        let worker_image = image.clone();
        let (entered, blocked) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            launch_or_attach_named_cancellable_with(
                &run_dir,
                &worker_image,
                Duration::from_secs(30),
                &worker_cancellation,
                |delay| {
                    entered.send(()).expect("announce blocked launch race");
                    worker_cancellation.wait_timeout(delay)
                },
            )
        });
        blocked
            .recv_timeout(Duration::from_secs(1))
            .expect("the launch-race phase blocks deterministically");

        let started = Instant::now();
        cancellation.cancel();
        let result = worker.join().expect("the launch-race worker joins");

        assert!(
            started.elapsed() < Duration::from_millis(250),
            "cancellation bounds the blocked launch race"
        );
        assert!(matches!(result, Err(SidecarError::Cancelled)));
        assert!(
            !crate::paths::gateway_discovery_file_path(dir.path()).exists(),
            "the cancelled loser never publishes or launches"
        );
        drop(winner);
    }
}
