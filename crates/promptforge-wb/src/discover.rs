//! `workbench.toml` discovery for the desktop shell.
//!
//! Search order, first found wins: beside the executable, then the current
//! directory, then `%USERPROFILE%\.promptforge\` (the user profile's
//! `.promptforge` directory). When no file is found, returns `None` so the
//! caller can generate a default configuration in the profile directory
//! (see [`generate_default`]).

use std::path::{Path, PathBuf};

use anyhow::Context as _;

/// File name searched for at each candidate location.
const CONFIG_FILE_NAME: &str = "workbench.toml";

/// The configuration written on first run, when the search finds nothing.
///
/// The gateway fields interpolate from the environment, so a machine with
/// `PROMPTFORGE_GATEWAY_URL` / `PROMPTFORGE_GATEWAY_API_KEY` set is
/// configured from the first launch; unset, they resolve to the built-in
/// defaults. Voice stays disabled until the whisper models land on disk;
/// the `*_source` URLs name where they come from.
const DEFAULT_CONFIG_TEMPLATE: &str = r#"# PromptForge Workbench configuration
# Generated on first run. Edit as needed.
# See: crates/promptforge-wb-server/README.md

[gateway]
base_url = "${PROMPTFORGE_GATEWAY_URL}"
api_key = "${PROMPTFORGE_GATEWAY_API_KEY}"

[server]
bind = "127.0.0.1:7910"
# open_browser = false

[tape]
path = "tape.jsonl"

[voice]
interim_source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin"
final_source = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3.bin"
# Voice is disabled until the models exist on disk; download the two files
# above, then uncomment:
# interim_model = "~/.promptforge/models/ggml-large-v3-turbo.bin"
# final_model = "~/.promptforge/models/ggml-large-v3.bin"
window_seconds = 5
interval_ms = 800
"#;

/// Locates `workbench.toml`, searching beside the executable first, then
/// the current directory, then the user profile's `.promptforge` directory.
///
/// Returns `None` when no location holds a config file, allowing the caller
/// to generate a default configuration instead.
///
/// # Errors
/// Returns an error when the executable or current directory cannot be
/// determined.
pub(crate) fn discover_config() -> anyhow::Result<Option<PathBuf>> {
    let exe_dir = std::env::current_exe()
        .context("locate the executable")
        .and_then(|exe| {
            exe.parent()
                .map(Path::to_path_buf)
                .context("the executable has no parent directory")
        })?;
    let cwd = std::env::current_dir().context("locate the current directory")?;
    let home = std::env::home_dir().context("locate the user profile directory")?;
    let candidates = candidates_from(&exe_dir, &cwd, &home);
    Ok(first_existing(&candidates))
}

/// Builds the candidate list in search order from the three base
/// directories.
fn candidates_from(exe_dir: &Path, cwd: &Path, home: &Path) -> Vec<PathBuf> {
    vec![
        exe_dir.join(CONFIG_FILE_NAME),
        cwd.join(CONFIG_FILE_NAME),
        home.join(".promptforge").join(CONFIG_FILE_NAME),
    ]
}

/// Returns the first candidate path that exists, if any.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// Writes the default configuration template to `path`, returning the path
/// written. The caller creates any parent directories.
///
/// # Errors
/// Returns the I/O error when the file cannot be written.
pub(crate) fn generate_default(path: &Path) -> std::io::Result<PathBuf> {
    std::fs::write(path, DEFAULT_CONFIG_TEMPLATE)?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_are_ordered_exe_then_cwd_then_profile() {
        let candidates = candidates_from(
            Path::new("exe-dir"),
            Path::new("cwd-dir"),
            Path::new("home-dir"),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("exe-dir/workbench.toml"),
                PathBuf::from("cwd-dir/workbench.toml"),
                PathBuf::from("home-dir/.promptforge/workbench.toml"),
            ]
        );
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let exe_dir = tempfile::TempDir::new().expect("tempdir");
        let cwd_dir = tempfile::TempDir::new().expect("tempdir");
        let home_dir = tempfile::TempDir::new().expect("tempdir");
        let promptforge = home_dir.path().join(".promptforge");
        std::fs::create_dir(&promptforge).expect("create profile dir");
        let in_cwd = cwd_dir.path().join("workbench.toml");
        let in_home = promptforge.join("workbench.toml");
        std::fs::write(&in_cwd, "").expect("write fixture");
        std::fs::write(&in_home, "").expect("write fixture");

        let candidates = candidates_from(exe_dir.path(), cwd_dir.path(), home_dir.path());
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_cwd.as_path()),
            "the current directory beats the profile"
        );

        let in_exe = exe_dir.path().join("workbench.toml");
        std::fs::write(&in_exe, "").expect("write fixture");
        assert_eq!(
            first_existing(&candidates).as_deref(),
            Some(in_exe.as_path()),
            "beside the executable beats everything"
        );
    }

    #[test]
    fn no_config_returns_none() {
        let exe_dir = tempfile::TempDir::new().expect("tempdir");
        let cwd_dir = tempfile::TempDir::new().expect("tempdir");
        let home_dir = tempfile::TempDir::new().expect("tempdir");
        let candidates = candidates_from(exe_dir.path(), cwd_dir.path(), home_dir.path());
        assert_eq!(first_existing(&candidates), None);
    }

    #[test]
    fn generate_default_writes_the_template() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let path = dir.path().join(CONFIG_FILE_NAME);
        let written = generate_default(&path).expect("template writes");
        assert_eq!(written, path);
        let contents = std::fs::read_to_string(&path).expect("read back");
        assert_eq!(contents, DEFAULT_CONFIG_TEMPLATE);
    }
}
