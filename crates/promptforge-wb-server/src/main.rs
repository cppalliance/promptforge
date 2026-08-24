//! The `promptforge-wb-server` binary: binds the workbench HTTP server.
//!
//! Thin shell around [`promptforge_wb_server::run`]; argument parsing and
//! configuration arrive with the workbench API step.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match promptforge_wb_server::run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
