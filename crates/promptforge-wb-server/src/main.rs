//! The `promptforge-wb-server` binary: loads `workbench.toml` and serves the
//! workbench HTTP API.
//!
//! Thin shell around [`promptforge_wb_server`]: load the config, build the
//! shared state, bind, and serve.

use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use promptforge_wb_server::{AppState, Config, DEFAULT_CONFIG_PATH};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt::init();
    match serve().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error:?}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> anyhow::Result<()> {
    let config = Config::load(Path::new(DEFAULT_CONFIG_PATH))
        .with_context(|| format!("load {DEFAULT_CONFIG_PATH}"))?;
    let state = AppState::new(&config).context("build shared state")?;
    promptforge_wb_server::run(state, &config.server.bind)
        .await
        .context("serve workbench")
}
