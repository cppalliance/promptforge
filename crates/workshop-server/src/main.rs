//! The `workshop-server` binary: loads `workshop.toml` and serves the
//! workshop HTTP API.
//!
//! Thin shell around [`workshop_server`]: load the config, spawn the
//! server in-process, optionally open the system browser at its address (the
//! browser-tab frame, for when no desktop window is driving), and wait.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use workshop_server::{Config, DEFAULT_CONFIG_PATH};

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    match serve() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

const LEGACY_CONFIG_PATH: &str = "workbench.toml";

fn serve() -> anyhow::Result<()> {
    let path = if Path::new(DEFAULT_CONFIG_PATH).is_file() {
        DEFAULT_CONFIG_PATH
    } else if Path::new(LEGACY_CONFIG_PATH).is_file() {
        LEGACY_CONFIG_PATH
    } else {
        DEFAULT_CONFIG_PATH
    };
    let config = Config::load(Path::new(path)).with_context(|| format!("load {path}"))?;
    let open_browser = config.server.open_browser;
    let server = workshop_server::spawn(config).context("start workshop server")?;
    if open_browser {
        let url = server.url().to_string();
        // A browser that will not open is not worth killing a serving
        // server over; the address is logged either way.
        if let Err(error) = open::that(&url) {
            tracing::warn!(%error, %url, "could not open the system browser");
        }
    }
    server.join().context("serve workshop")
}
