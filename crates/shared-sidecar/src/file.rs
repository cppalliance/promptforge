//! The `gateway.json` connection-file type: what the gateway writes after
//! a successful bind and what readers validate before attaching.

use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::atomic::write_atomic_owner_only;
use crate::error::SidecarError;
use crate::paths::connection_file_path;

/// The on-disk connection file naming a running gateway's loopback port
/// and bearer key.
///
/// Readers must tolerate unknown fields: a newer gateway may write fields
/// an older reader does not know, and serde ignores them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionFile {
    /// The gateway's bound port; readers connect to `127.0.0.1:{port}`.
    pub port: u16,
    /// The bearer key the gateway expects.
    pub api_key: String,
    /// The gateway process id.
    pub pid: u32,
    /// The gateway's boot instant in unix seconds.
    pub epoch: u64,
    /// The gateway crate version.
    pub version: String,
    /// RFC 3339 rendering of the boot instant.
    pub started_at: String,
}

impl ConnectionFile {
    /// The reason the file fails validation, or `None` when it is valid.
    ///
    /// Validation covers the fields attach depends on: a real port, a
    /// real key, a real pid. The remaining fields are informational.
    #[must_use]
    pub fn validation_error(&self) -> Option<&'static str> {
        if self.port == 0 {
            return Some("port must not be 0");
        }
        if self.api_key.is_empty() {
            return Some("api_key must not be empty");
        }
        if self.pid == 0 {
            return Some("pid must not be 0");
        }
        None
    }

    /// Reads and validates the connection file in `run_dir`, returning
    /// `None` when no file exists.
    ///
    /// # Errors
    /// Returns [`SidecarError::Read`] when the file exists but cannot be
    /// read, [`SidecarError::Parse`] when it is not valid JSON, and
    /// [`SidecarError::Invalid`] when it fails validation.
    ///
    /// # Examples
    /// ```
    /// # let dir = tempfile::tempdir()?;
    /// let file = shared_sidecar::ConnectionFile::read(dir.path())?;
    /// assert!(file.is_none());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn read(run_dir: &Path) -> Result<Option<ConnectionFile>, SidecarError> {
        let path = connection_file_path(run_dir);
        let raw = match fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(SidecarError::Read {
                    path,
                    source: error,
                });
            }
        };
        let file: ConnectionFile =
            serde_json::from_str(&raw).map_err(|source| SidecarError::Parse {
                path: path.clone(),
                source,
            })?;
        if let Some(reason) = file.validation_error() {
            return Err(SidecarError::Invalid {
                path,
                reason: reason.to_owned(),
            });
        }
        Ok(Some(file))
    }

    /// Writes the connection file into `run_dir`, creating the directory
    /// when missing. The write is atomic (temp sibling, sync, rename) and
    /// the file lands owner-only; see the crate docs for the permission
    /// contract.
    ///
    /// # Errors
    /// Returns [`SidecarError::Invalid`] when the file fails validation,
    /// [`SidecarError::CreateDir`] when the run directory cannot be
    /// created, [`SidecarError::Serialize`] when the file cannot be
    /// serialized, and [`SidecarError::Write`] when the atomic write
    /// fails.
    pub fn write_to(&self, run_dir: &Path) -> Result<(), SidecarError> {
        let path = connection_file_path(run_dir);
        if let Some(reason) = self.validation_error() {
            return Err(SidecarError::Invalid {
                path,
                reason: reason.to_owned(),
            });
        }
        fs::create_dir_all(run_dir).map_err(|source| SidecarError::CreateDir {
            path: run_dir.to_owned(),
            source,
        })?;
        let bytes =
            serde_json::to_vec_pretty(self).map_err(|source| SidecarError::Serialize { source })?;
        write_atomic_owner_only(&path, &bytes)
            .map_err(|source| SidecarError::Write { path, source })
    }
}

/// Removes the connection file in `run_dir` when it still belongs to
/// process `pid`, so a shutting-down gateway withdraws itself from
/// discovery but can never delete a replacement's file. A missing file is
/// not an error; an unreadable or foreign file is left alone for the next
/// [`crate::resolve`] to clean.
///
/// # Errors
/// Returns [`SidecarError::Remove`] when the file is this process's own
/// but cannot be removed.
pub fn remove_if_mine(run_dir: &Path, pid: u32) -> Result<(), SidecarError> {
    let path = connection_file_path(run_dir);
    match fs::read_to_string(&path) {
        Ok(raw) => {
            let owned =
                serde_json::from_str::<ConnectionFile>(&raw).is_ok_and(|file| file.pid == pid);
            if !owned {
                return Ok(());
            }
        }
        // A missing or unreadable file is left alone.
        Err(_) => return Ok(()),
    }
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

    fn valid_file() -> ConnectionFile {
        ConnectionFile {
            port: 8081,
            api_key: "key".to_owned(),
            pid: 4242,
            epoch: 1_757_000_000,
            version: "0.2.0".to_owned(),
            started_at: "2026-09-03T12:00:00Z".to_owned(),
        }
    }

    #[test]
    fn the_connection_file_round_trips_json() {
        let file = valid_file();
        let json = serde_json::to_string(&file).expect("serialize");
        let back: ConnectionFile = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(file, back);
    }

    #[test]
    fn unknown_fields_are_tolerated_for_forward_compatibility() {
        let json = r#"{"port":8081,"api_key":"k","pid":1,"epoch":0,"version":"0","started_at":"","future":true}"#;
        let file: ConnectionFile = serde_json::from_str(json).expect("unknown fields ignored");
        assert_eq!(file.port, 8081);
    }

    #[test]
    fn validation_rejects_a_zero_port_empty_key_and_zero_pid() {
        let mut file = valid_file();
        assert_eq!(file.validation_error(), None);
        file.port = 0;
        assert_eq!(file.validation_error(), Some("port must not be 0"));
        file = valid_file();
        file.api_key.clear();
        assert_eq!(file.validation_error(), Some("api_key must not be empty"));
        file = valid_file();
        file.pid = 0;
        assert_eq!(file.validation_error(), Some("pid must not be 0"));
    }

    #[test]
    fn read_returns_none_when_no_file_exists() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        assert_eq!(ConnectionFile::read(dir.path()).expect("read"), None);
    }

    #[test]
    fn read_rejects_corrupt_json() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        fs::write(dir.path().join("gateway.json"), b"not json").expect("write fixture");
        let error = ConnectionFile::read(dir.path()).expect_err("corrupt JSON must fail");
        assert!(
            matches!(error, SidecarError::Parse { .. }),
            "a corrupt file is a parse error: {error}"
        );
    }

    #[test]
    fn read_rejects_an_invalid_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut file = valid_file();
        file.port = 0;
        let json = serde_json::to_string(&file).expect("serialize");
        fs::write(dir.path().join("gateway.json"), json).expect("write fixture");
        let error = ConnectionFile::read(dir.path()).expect_err("an invalid file must fail");
        assert!(
            matches!(error, SidecarError::Invalid { .. }),
            "a zero port is a validation error: {error}"
        );
    }

    #[test]
    fn write_refuses_an_invalid_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let mut file = valid_file();
        file.api_key.clear();
        let error = file
            .write_to(dir.path())
            .expect_err("an invalid file must not be written");
        assert!(matches!(error, SidecarError::Invalid { .. }));
        assert!(
            !dir.path().join("gateway.json").exists(),
            "nothing was written"
        );
    }

    #[test]
    fn remove_if_mine_removes_only_the_owning_pid() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let file = valid_file();
        file.write_to(dir.path()).expect("write");

        remove_if_mine(dir.path(), 9999).expect("a foreign pid is tolerated");
        assert!(
            dir.path().join("gateway.json").exists(),
            "a foreign pid's removal spares the file"
        );

        remove_if_mine(dir.path(), file.pid).expect("the owning pid removes");
        assert!(
            !dir.path().join("gateway.json").exists(),
            "the owning pid's removal deletes the file"
        );
    }

    #[test]
    fn remove_if_mine_tolerates_a_missing_file() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        remove_if_mine(dir.path(), 4242).expect("a missing file is not an error");
    }
}
