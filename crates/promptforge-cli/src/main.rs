//! The `promptforge` command-line tool.
//!
//! `promptforge run <file.md> [input]` parses the prompt and executes its
//! sections top to bottom (fall-through). `input` is the single raw argument
//! string exposed to the prompt as `args`; it defaults to empty. The file must
//! be a promptforge prompt - its frontmatter must declare a `promptforge:`
//! version - or the CLI declines to run it. Gateway credentials come only from
//! `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`.
//!
//! `main` is the process boundary: it parses arguments, installs the Ctrl-C
//! signal, invokes the application runner, prints its output or error chain, and
//! selects the exit status. All orchestration lives in [`app`].

use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use promptforge_core::CancelHandle;
use promptforge_core::execute::RunError;
use promptforge_core::observe::{NullObserver, Observer};

use crate::app::{Cli, Command, RunRequest};

mod app;
mod progress;
mod tools;

/// Parses arguments, runs the requested command, and maps its result to a
/// process exit status. `clap` owns usage failures and their status; this
/// returns 130 for a cancelled run and 1 for any other failure. The
/// current-thread flavor suffices: the executor interleaves a run's chains
/// on the driver task's one thread.
#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let cancel = install_cancel();

    match cli.command {
        Command::Run(args) => {
            let observer: Arc<dyn Observer> = Arc::new(NullObserver::default());
            let request = RunRequest {
                file: &args.file,
                input: args.input.as_deref().unwrap_or_default(),
                store_dir: args.store_dir.as_deref(),
                observer,
                cancel,
            };
            match app::run(request).await {
                Ok(output) => {
                    println!("{output}");
                    ExitCode::SUCCESS
                }
                Err(error) => {
                    eprintln!("error: {error:?}");
                    ExitCode::from(exit_code(&error))
                }
            }
        }
    }
}

/// Installs a Ctrl-C handler that trips the returned cancellation handle once.
///
/// A failure to register the signal listener is reported to stderr rather than
/// discarded: the run simply stays non-cancellable in that case.
fn install_cancel() -> CancelHandle {
    let cancel = CancelHandle::new();
    let signal_cancel = cancel.clone();
    tokio::spawn(async move {
        match tokio::signal::ctrl_c().await {
            Ok(()) => signal_cancel.cancel(),
            Err(error) => {
                eprintln!("warning: cannot listen for Ctrl-C, run is not cancellable: {error}");
            }
        }
    });
    cancel
}

/// Maps a run failure to a process exit code: 130 for a cooperative cancellation
/// (the conventional interrupted code), 1 for every other failure.
fn exit_code(error: &anyhow::Error) -> u8 {
    let cancelled = error.chain().any(|cause| {
        cause
            .downcast_ref::<RunError>()
            .is_some_and(RunError::is_cancelled)
    });
    if cancelled { 130 } else { 1 }
}
