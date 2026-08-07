//! Explicit real-model entry point plus offline prompt fixture tests.

mod artifacts;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "argv dispatch reaches the dev runner in a later step"
    )
)]
mod dev;
mod scenarios;
mod server;

#[cfg(test)]
mod suite;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::server::{ServerGuard, ServerProfile};

#[tokio::main]
async fn main() -> ExitCode {
    let interrupted = Arc::new(AtomicBool::new(false));
    let result = tokio::select! {
        result = run_explicit_suite(Arc::clone(&interrupted)) => result,
        signal = tokio::signal::ctrl_c() => {
            interrupted.store(true, Ordering::Release);
            match signal {
                Ok(()) => Err(anyhow::anyhow!("real-model run interrupted by Ctrl-C")),
                Err(error) => Err(error).context("install Ctrl-C handler"),
            }
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("real-model suite failed: {error:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run_explicit_suite(interrupted: Arc<AtomicBool>) -> Result<()> {
    println!("provisioning pinned real-model artifacts");
    let artifacts =
        tokio::task::spawn_blocking(|| artifacts::provision(artifacts::ModelKind::Scenario))
            .await
            .context("join artifact provisioner")?
            .context("provision pinned real-model artifacts")?;
    println!("pinned artifacts are ready");

    let server_executable = artifacts.llama_server;
    let model = artifacts.model;
    let server = tokio::task::spawn_blocking(move || {
        ServerGuard::start(
            &server_executable,
            &model,
            ServerProfile::Scenario,
            &interrupted,
        )
    })
    .await
    .context("join llama-server startup")??;
    println!("llama-server is ready at {}", server.base_url());
    let result =
        scenarios::run_all(&server.base_url(), server.api_key(), server.model_alias()).await;
    if let Err(error) = result {
        bail!("{error:#}\n{}", server.diagnostics());
    }
    println!("real-model suite passed");
    Ok(())
}
