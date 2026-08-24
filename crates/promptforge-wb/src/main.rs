//! The `promptforge-wb` binary: the PromptForge Workbench desktop window
//! shell.
//!
//! Loads `workbench.toml` (see [`discover`] for the search order), starts
//! the workbench server in-process on its own thread, waits for its health
//! endpoint to answer, and opens a window pointed at it. Closing the window
//! shuts the server down cleanly.

mod discover;
mod health;
mod window;

use std::process::ExitCode;
use std::time::Duration;

use anyhow::Context as _;
use promptforge_wb_server::Config;

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
    let config_path = discover::discover_config()?;
    let config =
        Config::load(&config_path).with_context(|| format!("load {}", config_path.display()))?;
    let server = promptforge_wb_server::spawn(config).context("start workbench server")?;
    let url = server.url().to_string();

    // The server is shut down whether the window ran, failed, or never
    // opened because the health probe timed out.
    let window_result = health::wait_for_health(&url, HEALTH_TIMEOUT)
        .context("wait for the workbench server")
        .and_then(|()| window::run(&url));
    let shutdown_result = server.shutdown().context("stop workbench server");
    // A shutdown failure stacked on a window failure is reported, not lost.
    if let (Err(_), Err(shutdown_error)) = (&window_result, &shutdown_result) {
        eprintln!("{shutdown_error:?}");
    }
    window_result.and(shutdown_result)
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_is_named_promptforge_wb() {
        assert_eq!(env!("CARGO_PKG_NAME"), "promptforge-wb");
    }
}
