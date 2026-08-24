//! The `promptforge-wb-server` binary: loads `workbench.toml` and serves the
//! workbench HTTP API.
//!
//! Thin shell around [`promptforge_wb_server`]: load the config, spawn the
//! server in-process, optionally open the system browser at its address (the
//! browser-tab frame, for when no desktop window is driving), and wait.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use promptforge_wb_server::{Config, DEFAULT_CONFIG_PATH};

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

fn serve() -> anyhow::Result<()> {
    let config = Config::load(Path::new(DEFAULT_CONFIG_PATH))
        .with_context(|| format!("load {DEFAULT_CONFIG_PATH}"))?;
    let open_browser = config.server.open_browser;
    let server = promptforge_wb_server::spawn(config).context("start workbench server")?;
    if open_browser {
        let url = server.url().to_string();
        // A browser that will not open is not worth killing a serving
        // server over; the address is logged either way.
        if let Err(error) = open::that(&url) {
            tracing::warn!(%error, %url, "could not open the system browser");
        }
    }
    server.join().context("serve workbench")
}
