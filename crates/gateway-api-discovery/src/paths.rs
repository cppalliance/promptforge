//! Run-directory, gateway-discovery-file, and gateway-log paths: the one
//! place that knows the `<home>/.promptforge/run` and
//! `<home>/.promptforge/logs` layout, matching the profile convention in the
//! workshop's `discover.rs`.

use std::path::{Path, PathBuf};

/// The gateway discovery file's name inside the run directory.
pub const GATEWAY_DISCOVERY_FILE_NAME: &str = "gateway.json";

/// The gateway log's name inside the state directory's `logs` directory.
pub const GATEWAY_LOG_FILE_NAME: &str = "gateway.log";

/// The launch lock's name, beside the gateway discovery file.
pub const LOCK_FILE_NAME: &str = "gateway.json.lock";

/// The process-lifetime Gateway instance lock's name.
pub const INSTANCE_LOCK_FILE_NAME: &str = "gateway.instance.lock";

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

/// The gateway discovery file inside `run_dir`.
#[must_use]
pub fn gateway_discovery_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(GATEWAY_DISCOVERY_FILE_NAME)
}

/// The launch lock inside `run_dir`.
#[must_use]
pub fn lock_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(LOCK_FILE_NAME)
}

/// The process-lifetime Gateway instance lock inside `run_dir`.
#[must_use]
pub fn instance_lock_file_path(run_dir: &Path) -> PathBuf {
    run_dir.join(INSTANCE_LOCK_FILE_NAME)
}

/// The gateway log under `state_dir`, the directory that holds the run
/// directory: `<home>/.promptforge/logs/gateway.log`. The gateway starts
/// logging there before it loads its configuration, so a gateway that exits
/// on its configuration still records why.
#[must_use]
pub fn gateway_log_path(state_dir: &Path) -> PathBuf {
    state_dir.join("logs").join(GATEWAY_LOG_FILE_NAME)
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
    fn the_gateway_discovery_file_and_locks_sit_beside_each_other() {
        let dir = Path::new("run");
        assert_eq!(
            gateway_discovery_file_path(dir),
            Path::new("run").join("gateway.json")
        );
        assert_eq!(
            lock_file_path(dir),
            Path::new("run").join("gateway.json.lock")
        );
        assert_eq!(
            instance_lock_file_path(dir),
            Path::new("run").join("gateway.instance.lock")
        );
    }

    #[test]
    fn the_gateway_log_sits_in_a_logs_directory_beside_the_run_directory() {
        let run = run_dir(Path::new("home"));
        let state_dir = run.parent().expect("the run directory has a parent");
        assert_eq!(
            gateway_log_path(state_dir),
            Path::new("home")
                .join(".promptforge")
                .join("logs")
                .join("gateway.log")
        );
    }
}
