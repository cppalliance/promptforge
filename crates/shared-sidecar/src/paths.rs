//! Run-directory and connection-file paths: the one place that knows the
//! `<home>/.promptforge/run` layout, matching the profile convention in the
//! workshop's `discover.rs`.

use std::path::{Path, PathBuf};

/// The connection file's name inside the run directory.
pub const CONNECTION_FILE_NAME: &str = "gateway.json";

/// The launch lock's name, beside the connection file.
pub const LOCK_FILE_NAME: &str = "gateway.json.lock";

/// The run directory under the state dir: `<home>/.promptforge/run`.
#[must_use]
pub fn run_dir(home: &Path) -> PathBuf {
    home.join(".promptforge").join("run")
}

/// This process's default run directory: the user profile's
/// `.promptforge/run`.
///
/// Returns `None` when the user profile directory cannot be located.
#[must_use]
pub fn default_run_dir() -> Option<PathBuf> {
    std::env::home_dir().map(|home| run_dir(&home))
}

/// The connection file inside `run_dir`.
#[must_use]
pub fn connection_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(CONNECTION_FILE_NAME)
}

/// The launch lock inside `run_dir`.
#[must_use]
pub fn lock_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(LOCK_FILE_NAME)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_run_dir_sits_under_the_profile_promptforge_dir() {
        assert_eq!(
            run_dir(Path::new("home")),
            Path::new("home").join(".promptforge").join("run")
        );
    }

    #[test]
    fn the_connection_file_and_lock_sit_beside_each_other() {
        let dir = Path::new("run");
        assert_eq!(
            connection_file_path(dir),
            Path::new("run").join("gateway.json")
        );
        assert_eq!(
            lock_file_path(dir),
            Path::new("run").join("gateway.json.lock")
        );
    }
}
