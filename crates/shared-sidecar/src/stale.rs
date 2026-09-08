//! Stale detection: decide whether a connection file names a live
//! gateway, and remove it when it does not.
//!
//! A file is live when one OS process boot with a `promptforge-gateway`
//! image is unchanged across a same-socket health and bearer proof, and
//! the file carries a boot identity. Anything else is stale - the Jupyter
//! phantom-server bug class - and the file is deleted so the next reader
//! relaunches instead of retrying a corpse.

use std::fs;
use std::io;
use std::path::Path;

use crate::ConnectionFile;
use crate::error::SidecarError;
use crate::paths::connection_file_path;
pub(crate) use crate::validated::GATEWAY_IMAGE_NAME;
use crate::validated::ValidatedConnection;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum StaleReason {
    /// The file was not valid JSON or failed validation.
    #[error("the connection file is invalid")]
    Invalid,
    /// The pid is dead.
    #[error("the recorded gateway process is dead")]
    ProcessDead,
    /// The pid is alive but its image is not a `promptforge-gateway`
    /// binary (a reused pid).
    #[error("the recorded pid belongs to another process image")]
    ImageMismatch,
    /// The connection file does not carry a usable boot identity.
    #[error("the connection file has no usable boot identity")]
    BootIdentityInvalid,
    /// The pid changed process boot while validation was in progress.
    #[error("the recorded process identity changed during validation")]
    ProcessChanged,
    /// The health endpoint did not answer 200.
    #[error("the recorded gateway does not answer its health probe")]
    HealthFailed,
    /// The bearer key was rejected.
    #[error("the connection file bearer was rejected")]
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
    match ValidatedConnection::validate_named(file, image_name) {
        Ok(validated) => Ok(Resolution::Attach(validated.into_connection_file())),
        Err(reason) => {
            remove_stale(run_dir)?;
            Ok(Resolution::Stale(reason))
        }
    }
}

/// Whether the connection file in `run_dir` names a live gateway right
/// now, with no cleanup: the read-only check a diagnostics report runs.
/// Stale-file deletion is the prospective owner's privilege, so a stale
/// file reads as not-running and stays on disk for the next launch to
/// clean.
#[must_use]
pub fn is_running(run_dir: &Path) -> bool {
    is_running_named(run_dir, GATEWAY_IMAGE_NAME)
}

/// [`is_running`] against a caller-named process image, so a test binary -
/// never named `promptforge-gateway` - can run the full liveness gauntlet.
pub(crate) fn is_running_named(run_dir: &Path, image_name: &str) -> bool {
    match ConnectionFile::read(run_dir) {
        Ok(Some(file)) => is_live(&file, image_name),
        // A missing, unreadable, or invalid file reads as not-running.
        Ok(None) | Err(_) => false,
    }
}

/// Whether the file's gateway is live right now, with no cleanup: the
/// check a launch-race loser runs, since deleting is the lock holder's
/// privilege.
pub(crate) fn is_live(file: &ConnectionFile, image_name: &str) -> bool {
    ValidatedConnection::validate_named(file.clone(), image_name).is_ok()
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
                for _ in 0..2 {
                    let mut buffer = [0u8; 1024];
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    let request = String::from_utf8_lossy(&buffer[..read]);
                    let accepted = request.starts_with("GET /health ")
                        || request.contains(&format!("Authorization: Bearer {expected_key}\r\n"));
                    let response = if accepted {
                        &b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"[..]
                    } else {
                        &b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n"[..]
                    };
                    if stream.write_all(response).is_err() {
                        break;
                    }
                }
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

    #[test]
    fn is_running_reports_a_live_gateway_without_touching_the_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let port = fixture_gateway("right");
        live_file(port, "right")
            .write_to(dir.path())
            .expect("write");

        assert!(
            is_running_named(dir.path(), &own_image_name()),
            "a fully live file reads as running"
        );
        assert!(
            connection_file_path(dir.path()).exists(),
            "the read-only check never deletes"
        );
    }

    #[test]
    fn is_running_leaves_a_stale_file_in_place() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = ConnectionFile {
            pid: dead_pid(),
            ..live_file(1, "key")
        };
        file.write_to(dir.path()).expect("write");

        assert!(
            !is_running_named(dir.path(), &own_image_name()),
            "a dead pid reads as not-running"
        );
        assert!(
            connection_file_path(dir.path()).exists(),
            "stale-file deletion is the prospective owner's privilege"
        );
    }

    #[test]
    fn is_running_reads_absent_and_corrupt_files_as_not_running() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        assert!(
            !is_running_named(dir.path(), &own_image_name()),
            "no connection file reads as not-running"
        );
        fs::write(connection_file_path(dir.path()), b"not json").expect("write fixture");
        assert!(
            !is_running_named(dir.path(), &own_image_name()),
            "a corrupt file reads as not-running and is left alone"
        );
        assert!(
            connection_file_path(dir.path()).exists(),
            "the corrupt file was not deleted"
        );
    }
}
