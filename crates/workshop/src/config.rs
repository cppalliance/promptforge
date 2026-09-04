//! The shell's workshop-server configuration: `workshop.toml` discovery
//! and the forced ephemeral loopback bind.
//!
//! The shell hosts the workshop server in-process, so the listener
//! settings are the shell's own: the bind is always `127.0.0.1:0` (an
//! OS-assigned port - a fixed port is a conflict class the
//! single-instance handoff cannot close) and `open_browser` stays off
//! (the shell drives its own window). A discovered `workshop.toml` still
//! owns the `[gateway]` connection settings and the state and
//! agent-program paths; the gateway endpoint itself resolves inside the
//! server, connection file first, explicit config second.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use workshop_server::Config;

/// Canonical file name searched for at each candidate location.
const CONFIG_FILE_NAME: &str = "workshop.toml";

/// The shell's listener bind: loopback on an OS-assigned port, reported
/// back through the server handle once bound.
const SHELL_BIND: &str = "127.0.0.1:0";

/// Loads the shell's workshop-server configuration.
///
/// A `workshop.toml` found in the search order - beside the executable,
/// then the current directory, then the user profile's `.promptforge`
/// directory - supplies the `[gateway]` connection and the path
/// settings; the listener settings are forced to the shell's own. With
/// no file, the default config anchors its state in the profile's
/// `.promptforge` directory and carries no explicit gateway, so endpoint
/// resolution attaches through the gateway's connection file or fails
/// plainly.
///
/// # Errors
/// Returns an error when a discovered file cannot be loaded or the
/// executable or current directory cannot be determined.
pub(crate) fn load() -> anyhow::Result<Config> {
    let exe_dir = std::env::current_exe()
        .context("locate the executable")
        .and_then(|exe| {
            exe.parent()
                .map(Path::to_path_buf)
                .context("the executable has no parent directory")
        })?;
    let cwd = std::env::current_dir().context("locate the current directory")?;
    load_in(&exe_dir, &cwd, std::env::home_dir().as_deref())
}

/// The testable core of [`load`], with the three base directories
/// injected. A missing home directory degrades to skipping the profile
/// candidate and anchoring the no-file default at the current directory,
/// matching `Config::parse`'s anchor degradation.
fn load_in(exe_dir: &Path, cwd: &Path, home: Option<&Path>) -> anyhow::Result<Config> {
    let candidates = candidates_from(exe_dir, cwd, home);
    match first_existing(&candidates) {
        Some(path) => {
            let mut config =
                Config::load(&path).with_context(|| format!("load {}", path.display()))?;
            shape_for_shell(&mut config);
            Ok(config)
        }
        None => Ok(default_config(home)),
    }
}

/// Builds the candidate list in search order from the three base
/// directories.
fn candidates_from(exe_dir: &Path, cwd: &Path, home: Option<&Path>) -> Vec<PathBuf> {
    let mut candidates = vec![exe_dir.join(CONFIG_FILE_NAME), cwd.join(CONFIG_FILE_NAME)];
    if let Some(home) = home {
        candidates.push(profile_dir(home).join(CONFIG_FILE_NAME));
    }
    candidates
}

/// Returns the first candidate path that exists, if any.
fn first_existing(candidates: &[PathBuf]) -> Option<PathBuf> {
    candidates.iter().find(|path| path.is_file()).cloned()
}

/// The profile directory: `<home>/.promptforge`.
fn profile_dir(home: &Path) -> PathBuf {
    home.join(".promptforge")
}

/// Forces the listener settings the shell owns onto a loaded config: the
/// ephemeral loopback bind and no browser opening.
fn shape_for_shell(config: &mut Config) {
    config.server.bind = SHELL_BIND.to_string();
    config.server.open_browser = false;
}

/// The no-file configuration: no explicit gateway (endpoint resolution
/// attaches through the connection file or fails plainly), with the
/// state and agent-program paths anchored at the profile directory.
fn default_config(home: Option<&Path>) -> Config {
    let mut config = Config {
        gateway: workshop_server::GatewayConfig {
            base_url: String::new(),
            api_key: String::new(),
        },
        server: workshop_server::ServerConfig {
            bind: SHELL_BIND.to_string(),
            open_browser: false,
            state_dir: PathBuf::new(),
        },
        agents: workshop_server::AgentsConfig::default(),
    };
    let anchor = home.map_or_else(|| PathBuf::from("."), profile_dir);
    config.anchor_path_defaults(&anchor);
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three empty candidate roots under one tempdir.
    fn roots() -> (tempfile::TempDir, PathBuf, PathBuf, PathBuf) {
        let temp = tempfile::TempDir::new().expect("tempdir");
        let exe = temp.path().join("exe");
        let cwd = temp.path().join("cwd");
        let home = temp.path().join("home");
        std::fs::create_dir_all(&exe).expect("exe dir");
        std::fs::create_dir_all(&cwd).expect("cwd dir");
        std::fs::create_dir_all(profile_dir(&home)).expect("profile dir");
        (temp, exe, cwd, home)
    }

    fn write(path: &Path, gateway_url: &str) {
        std::fs::write(
            path,
            format!("[gateway]\nbase_url = \"{gateway_url}\"\napi_key = \"k\"\n"),
        )
        .expect("write fixture");
    }

    #[test]
    fn candidates_are_ordered_exe_then_cwd_then_profile() {
        let candidates = candidates_from(
            Path::new("exe-dir"),
            Path::new("cwd-dir"),
            Some(Path::new("home-dir")),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("exe-dir/workshop.toml"),
                PathBuf::from("cwd-dir/workshop.toml"),
                PathBuf::from("home-dir/.promptforge/workshop.toml"),
            ]
        );
    }

    #[test]
    fn a_missing_home_skips_the_profile_candidate() {
        let candidates = candidates_from(Path::new("exe-dir"), Path::new("cwd-dir"), None);
        assert_eq!(candidates.len(), 2, "no profile candidate without a home");
    }

    #[test]
    fn the_first_existing_candidate_wins() {
        let (_temp, exe, cwd, home) = roots();
        let in_cwd = cwd.join(CONFIG_FILE_NAME);
        let in_home = profile_dir(&home).join(CONFIG_FILE_NAME);
        write(&in_cwd, "http://cwd:1");
        write(&in_home, "http://home:1");
        let config = load_in(&exe, &cwd, Some(&home)).expect("loads");
        assert_eq!(
            config.gateway.base_url, "http://cwd:1",
            "cwd beats the profile"
        );

        let in_exe = exe.join(CONFIG_FILE_NAME);
        write(&in_exe, "http://exe:1");
        let config = load_in(&exe, &cwd, Some(&home)).expect("loads");
        assert_eq!(
            config.gateway.base_url, "http://exe:1",
            "beside the executable beats everything"
        );
    }

    #[test]
    fn a_discovered_file_keeps_its_gateway_and_paths_but_not_the_listener() {
        let (_temp, exe, cwd, home) = roots();
        let path = profile_dir(&home).join(CONFIG_FILE_NAME);
        std::fs::write(
            &path,
            "[gateway]\nbase_url = \"http://gateway.lan:9999\"\napi_key = \"k\"\n\n\
             [server]\nbind = \"127.0.0.1:7910\"\nopen_browser = true\n",
        )
        .expect("write fixture");
        let config = load_in(&exe, &cwd, Some(&home)).expect("loads");
        assert_eq!(config.gateway.base_url, "http://gateway.lan:9999");
        assert_eq!(
            config.server.bind, SHELL_BIND,
            "the shell owns the listener: an OS-assigned loopback port"
        );
        assert!(
            !config.server.open_browser,
            "the shell drives its own window"
        );
        assert_eq!(
            config.server.state_dir,
            profile_dir(&home),
            "state anchors beside the discovered file"
        );
    }

    #[test]
    fn no_file_defaults_to_discovery_with_profile_anchored_state() {
        let (_temp, exe, cwd, home) = roots();
        let config = load_in(&exe, &cwd, Some(&home)).expect("the default config");
        assert_eq!(
            config.gateway.base_url, "",
            "an empty base_url is the not-explicit signal resolution reads"
        );
        assert_eq!(config.server.bind, SHELL_BIND);
        assert!(!config.server.open_browser);
        assert_eq!(config.server.state_dir, profile_dir(&home));
        assert_eq!(config.agents.path, profile_dir(&home).join("agents"));
    }

    #[test]
    fn no_file_and_no_home_anchors_at_the_working_directory() {
        let (_temp, exe, cwd, _home) = roots();
        let config = load_in(&exe, &cwd, None).expect("the default config");
        assert_eq!(config.server.state_dir, Path::new("."));
    }

    #[test]
    fn a_malformed_discovered_file_is_an_error() {
        let (_temp, exe, cwd, home) = roots();
        std::fs::write(exe.join(CONFIG_FILE_NAME), "[gateway\n").expect("write fixture");
        let error = load_in(&exe, &cwd, Some(&home)).expect_err("malformed TOML must fail");
        assert!(
            error.to_string().contains(CONFIG_FILE_NAME),
            "the error names the file: {error}"
        );
    }
}
