//! Stale detection: decide whether a connection file names a live
//! gateway, and remove it when it does not.
//!
//! A file is live when the pid is alive, the pid's process image is a
//! `promptforge-gateway` binary (a reused pid cannot impersonate the
//! gateway), `GET /health` answers 200, and the file's bearer key is
//! accepted on a key-gated route. Anything else is stale - the Jupyter
//! phantom-server bug class - and the file is deleted so the next reader
//! relaunches instead of retrying a corpse.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use crate::ConnectionFile;
use crate::error::SidecarError;
use crate::health::{self, KeyProbe};
use crate::paths::connection_file_path;
use crate::sys::process_image_path;

/// The image file name a live gateway process must have.
#[cfg(windows)]
pub(crate) const GATEWAY_IMAGE_NAME: &str = "promptforge-gateway.exe";
/// The image file name a live gateway process must have.
#[cfg(not(windows))]
pub(crate) const GATEWAY_IMAGE_NAME: &str = "promptforge-gateway";

/// The bearer-gated route used to prove the presented key is accepted.
/// `GET /v1/models` is key-gated in every gateway build.
const KEY_PROBE_PATH: &str = "/v1/models";

/// Budget the health probe gets before a file is condemned: the writer
/// lands the file before its serve loop starts accepting, and a busy
/// runtime can starve one probe, so a single failed attempt must never
/// read as stale - a false stale deletes a live gateway's file and a
/// reader relaunches a duplicate.
const LIVENESS_BUDGET: Duration = Duration::from_secs(2);

/// What [`resolve`] found in the run directory.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Resolution {
    /// A live gateway: attach with these parameters.
    Attach(ConnectionFile),
    /// No connection file exists: nothing to attach to, nothing to clean.
    Absent,
    /// A connection file existed but was stale; it was removed.
    Stale(StaleReason),
}

/// Why a connection file was judged stale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StaleReason {
    /// The file was not valid JSON or failed validation.
    Invalid,
    /// The pid is dead.
    ProcessDead,
    /// The pid is alive but its image is not a `promptforge-gateway`
    /// binary (a reused pid).
    ImageMismatch,
    /// The health endpoint did not answer 200.
    HealthFailed,
    /// The bearer key was rejected.
    KeyRejected,
}

/// Resolves the connection file in `run_dir`: attach parameters for a
/// live gateway, or stale-file cleanup plus the reason.
///
/// # Errors
/// Returns [`SidecarError::Read`] when the file exists but cannot be
/// read, and [`SidecarError::Remove`] when a stale file cannot be
/// deleted.
pub fn resolve(run_dir: &Path) -> Result<Resolution, SidecarError> {
    resolve_named(run_dir, GATEWAY_IMAGE_NAME)
}

/// [`resolve`] against a caller-named process image, so a consumer's test
/// binary - never named `promptforge-gateway` - can run the full liveness
/// gauntlet. Test builds only, behind the `test-fixtures` feature.
#[cfg(feature = "test-fixtures")]
#[doc(hidden)]
pub fn resolve_for_test(run_dir: &Path, image_name: &str) -> Result<Resolution, SidecarError> {
    resolve_named(run_dir, image_name)
}

/// [`resolve`] against a caller-named process image, so tests can run the
/// full liveness gauntlet from a test binary, which is never named
/// `promptforge-gateway`.
pub(crate) fn resolve_named(run_dir: &Path, image_name: &str) -> Result<Resolution, SidecarError> {
    let file = match ConnectionFile::read(run_dir) {
        Ok(Some(file)) => file,
        Ok(None) => return Ok(Resolution::Absent),
        Err(SidecarError::Parse { .. } | SidecarError::Invalid { .. }) => {
            remove_stale(run_dir)?;
            return Ok(Resolution::Stale(StaleReason::Invalid));
        }
        Err(error) => return Err(error),
    };
    match liveness_failure(&file, image_name) {
        None => Ok(Resolution::Attach(file)),
        Some(reason) => {
            remove_stale(run_dir)?;
            Ok(Resolution::Stale(reason))
        }
    }
}

/// Whether the file's gateway is live right now, with no cleanup: the
/// check a launch-race loser runs, since deleting is the lock holder's
/// privilege.
pub(crate) fn is_live(file: &ConnectionFile, image_name: &str) -> bool {
    liveness_failure(file, image_name).is_none()
}

/// The first liveness check the file fails, or `None` when it is fully
/// live.
fn liveness_failure(file: &ConnectionFile, image_name: &str) -> Option<StaleReason> {
    let Some(image) = process_image_path(file.pid) else {
        return Some(StaleReason::ProcessDead);
    };
    if !image_name_matches(&image, image_name) {
        return Some(StaleReason::ImageMismatch);
    }
    let port = file.port;
    let address = format!("127.0.0.1:{port}");
    if health::wait_for_health(&format!("http://{address}"), LIVENESS_BUDGET).is_err() {
        return Some(StaleReason::HealthFailed);
    }
    match health::probe_bearer(&address, KEY_PROBE_PATH, &file.api_key) {
        KeyProbe::Accepted => None,
        KeyProbe::Rejected => Some(StaleReason::KeyRejected),
        // The health probe answered moments ago; a now-silent server is a
        // health failure, not a key rejection.
        KeyProbe::Unreachable => Some(StaleReason::HealthFailed),
    }
}

/// Deletes the stale connection file, tolerating a concurrent deletion.
fn remove_stale(run_dir: &Path) -> Result<(), SidecarError> {
    let path = connection_file_path(run_dir);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(SidecarError::Remove {
            path,
            source: error,
        }),
    }
}

/// Whether the image path's file name matches the expected gateway image
/// name.
fn image_name_matches(image: &Path, expected: &str) -> bool {
    let Some(name) = image.file_name() else {
        return false;
    };
    image_file_name_matches(name, expected)
}

/// Windows filesystems are case-insensitive; match the image name the
/// same way.
#[cfg(windows)]
fn image_file_name_matches(name: &OsStr, expected: &str) -> bool {
    name.to_string_lossy().eq_ignore_ascii_case(expected)
}

/// Unix filesystems are case-sensitive; match the image name exactly.
#[cfg(not(windows))]
fn image_file_name_matches(name: &OsStr, expected: &str) -> bool {
    name == OsStr::new(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write as _};
    use std::net::TcpListener;

    use crate::paths::connection_file_path;

    /// The test process's own image name, so the pid and image checks
    /// pass and the test reaches the probe under test.
    fn own_image_name() -> String {
        std::env::current_exe()
            .expect("current exe")
            .file_name()
            .expect("the exe has a file name")
            .to_string_lossy()
            .into_owned()
    }

    /// A connection file pointing at the test process itself.
    fn live_file(port: u16, api_key: &str) -> ConnectionFile {
        ConnectionFile {
            port,
            api_key: api_key.to_owned(),
            pid: std::process::id(),
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    /// A pid guaranteed dead: a short-lived child, reaped and dropped so
    /// no handle keeps the process object alive.
    fn dead_pid() -> u32 {
        let mut child = std::process::Command::new(std::env::current_exe().expect("current exe"))
            .arg("--list")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn a short-lived child");
        let pid = child.id();
        child.wait().expect("the child exits");
        drop(child);
        pid
    }

    /// A fixture gateway: answers `GET /health` with 200 and the key
    /// probe with 200 only when the bearer matches `expected_key`.
    fn fixture_gateway(expected_key: &'static str) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let Ok(read) = stream.read(&mut buffer) else {
                    continue;
                };
                let request = String::from_utf8_lossy(&buffer[..read]);
                let response = if request.starts_with("GET /health ")
                    || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"))
                {
                    &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                } else {
                    &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                };
                let _ = stream.write_all(response);
            }
        });
        port
    }

    #[test]
    fn a_dead_pid_is_stale_and_cleaned() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = live_file(1, "key");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..file
        };
        file.write_to(dir.path()).expect("write");

        let resolution = resolve(dir.path()).expect("resolve");
        assert_eq!(resolution, Resolution::Stale(StaleReason::ProcessDead));
        assert!(
            !connection_file_path(dir.path()).exists(),
            "the stale file was deleted"
        );
    }

    #[test]
    fn an_alive_pid_with_a_foreign_image_is_stale() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // The test binary is never named promptforge-gateway, so the real
        // image name must reject the test process's own pid.
        live_file(1, "key").write_to(dir.path()).expect("write");

        let resolution = resolve(dir.path()).expect("resolve");
        assert_eq!(resolution, Resolution::Stale(StaleReason::ImageMismatch));
        assert!(
            !connection_file_path(dir.path()).exists(),
            "the stale file was deleted"
        );
    }

    #[test]
    fn a_silent_health_endpoint_is_stale() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // Port 1 never listens, so every probe is refused until the
        // liveness budget elapses.
        live_file(1, "key").write_to(dir.path()).expect("write");

        let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
        assert_eq!(resolution, Resolution::Stale(StaleReason::HealthFailed));
        assert!(
            !connection_file_path(dir.path()).exists(),
            "the stale file was deleted"
        );
    }

    #[test]
    fn a_transiently_silent_health_endpoint_is_not_stale() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        // The first probe connection is hung up without a response; only
        // a retried probe learns the gateway is live.
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let port = listener.local_addr().expect("fixture address").port();
        std::thread::spawn(move || {
            let mut first = true;
            while let Ok((mut stream, _)) = listener.accept() {
                if std::mem::take(&mut first) {
                    drop(stream);
                    continue;
                }
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        let file = live_file(port, "key");
        file.write_to(dir.path()).expect("write");

        let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
        assert_eq!(
            resolution,
            Resolution::Attach(file),
            "one dropped probe must not condemn a live gateway"
        );
        assert!(
            connection_file_path(dir.path()).exists(),
            "a live file is left in place"
        );
    }

    #[test]
    fn a_rejected_key_is_stale_and_cleaned() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("right");
        live_file(port, "wrong")
            .write_to(dir.path())
            .expect("write");

        let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
        assert_eq!(resolution, Resolution::Stale(StaleReason::KeyRejected));
        assert!(
            !connection_file_path(dir.path()).exists(),
            "the stale file was deleted"
        );
    }

    #[test]
    fn a_fully_live_file_attaches() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("right");
        let file = live_file(port, "right");
        file.write_to(dir.path()).expect("write");

        let resolution = resolve_named(dir.path(), &own_image_name()).expect("resolve");
        assert_eq!(resolution, Resolution::Attach(file));
        assert!(
            connection_file_path(dir.path()).exists(),
            "a live file is left in place"
        );
    }
}
