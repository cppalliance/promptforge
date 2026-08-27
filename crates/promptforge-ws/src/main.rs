//! The `promptforge-ws` binary: the PromptForge Workshop desktop window
//! shell.
//!
//! Loads `workshop.toml` (see [`discover`] for the search order),
//! generating a default one in the profile's `.promptforge` directory on
//! first run, starts the workshop server in-process on its own thread,
//! waits for its health endpoint to answer, and opens a window pointed at
//! it. Closing the window shuts the server down cleanly.

mod discover;
// The only unsafe module in the workspace: the WebView2 COM surface that
// reads real OS paths out of dropped File objects has no safe wrapper.
// The clippy allows cover code the #[implement] macro expands in tests.
#[cfg(target_os = "windows")]
#[allow(unsafe_code, clippy::inline_always, clippy::ref_as_ptr)]
mod file_drop;
mod health;
mod window;

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context as _;
use promptforge_ws_server::Config;

/// How long the shell waits for the in-process server's health endpoint
/// before giving up.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(15);

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let config_path = match discover::discover_config()? {
        Some(config_path) => config_path,
        None => generate_in_profile()?,
    };
    let config =
        Config::load(&config_path).with_context(|| format!("load {}", config_path.display()))?;
    let server = promptforge_ws_server::spawn(config).context("start workshop server")?;
    let url = server.url().to_string();

    // The server is shut down whether the window ran, failed, or never
    // opened because the health probe timed out.
    let window_result = health::wait_for_health(&url, HEALTH_TIMEOUT)
        .context("wait for the workshop server")
        .and_then(|()| window::run(&url));
    let shutdown_result = server.shutdown().context("stop workshop server");
    // A shutdown failure stacked on a window failure is reported, not lost.
    if let (Err(_), Err(shutdown_error)) = (&window_result, &shutdown_result) {
        eprintln!("{shutdown_error:?}");
    }
    window_result.and(shutdown_result)
}

/// First run: creates the profile's `.promptforge` directory if needed and
/// writes the default `workshop.toml` into it.
fn generate_in_profile() -> anyhow::Result<PathBuf> {
    let home = std::env::home_dir().context("locate the user profile directory")?;
    let path = discover::profile_config_path(&home);
    let dir = path
        .parent()
        .context("the profile config path has no parent")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let path = discover::generate_default(&path).context("write the default configuration")?;
    eprintln!(
        "no workshop.toml found; wrote default config to {}",
        path.display()
    );
    Ok(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_named_promptforge_ws() {
        assert_eq!(env!("CARGO_PKG_NAME"), "promptforge-ws");
    }
}
