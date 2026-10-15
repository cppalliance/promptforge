//! The `promptforge-gateway` binary:
//! `promptforge-gateway [--config PATH] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]`.
//!
//! This is a thin shell: it parses arguments into a typed `ServeOptions` and
//! hands off to [`run_with_tray`], which owns the tokio runtime, provisioning,
//! and serving while the system tray occupies the main thread. `--no-tray`
//! keeps the headless Ctrl-C loop ([`run`]) for servers and CI. With no config
//! path from either source, the gateway runs boot discovery and, on first run,
//! generates a default config. A second launch while a gateway is already
//! running never boots a duplicate: it opens the running gateway's Settings
//! page (or prints its URL under `--print-url`) and exits.

use std::path::PathBuf;
use std::process::ExitCode;

use gateway::{
    GatewayStartup, ShutdownReason, run, run_printing_url, run_with_tray, settle_gateway_startup,
};
use gateway_logging::{LogConfig, LogRuntime};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::cli::{LaunchCommand, ParseError, USAGE, parse_args};

mod cli;

/// The log filter when `RUST_LOG` is unset: the gateway crates at `info`
/// (a download or a switch must say what it is doing), the chatty HTTP
/// dependencies at `warn`. `RUST_LOG` overrides the whole string.
const DEFAULT_LOG_FILTER: &str = "info,whisper_cpp=warn,hyper=warn,h2=warn,reqwest=warn,tower=warn";

/// Maximum time a process-lease loser waits for the owner's validated record.
const OWNER_PUBLICATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
#[cfg(feature = "test-fixtures")]
const TEST_START_READY_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_READY";
#[cfg(feature = "test-fixtures")]
const TEST_START_RELEASE_ENV: &str = "PROMPTFORGE_GATEWAY_TEST_START_RELEASE";
#[cfg(feature = "test-fixtures")]
const TEST_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[cfg_attr(
    windows,
    expect(
        unsafe_code,
        reason = "the one-call DPI-awareness shim at process start; every other unsafe lives in the tray and registry modules"
    )
)]
fn main() -> ExitCode {
    // The process is PerMonitorV2 DPI-aware from the start: the tray menu's
    // popup position comes from `Shell_NotifyIconGetRect` in physical
    // pixels, and a DPI-unaware process would have Windows scale the menu
    // away from the icon on a high-DPI display.
    #[cfg(target_os = "windows")]
    // SAFETY: called once at process start, before any window exists.
    unsafe {
        windows_sys::Win32::UI::HiDpi::SetProcessDpiAwarenessContext(
            windows_sys::Win32::UI::HiDpi::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        );
    }

    let invocation = match parse_args(std::env::args_os()) {
        Ok(invocation) => invocation,
        Err(ParseError::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(ParseError::Version) => {
            println!("promptforge-gateway {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Err(ParseError::Usage(message)) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    // The diagnostics report is not a boot: it runs before the handoff
    // check and before logging starts, and never rotates a log, parses a
    // config, or mutates the state directory.
    if invocation.command == LaunchCommand::Diagnostics {
        print!(
            "{}",
            gateway::diagnostics_json(invocation.serve.config_path)
        );
        return ExitCode::SUCCESS;
    }

    #[cfg(feature = "test-fixtures")]
    if let Err(error) = wait_for_test_start_rendezvous() {
        eprintln!("error: {error}");
        return ExitCode::FAILURE;
    }

    // Process ownership settles before canonical logging, stale cleanup by a
    // loser, recovery, bind, or publication. The lease holder re-resolves the
    // connection record; a loser waits only for the holder's validated record
    // and exits through this console-only path.
    let _instance_lease = match settle_gateway_startup(&invocation.serve, OWNER_PUBLICATION_TIMEOUT)
    {
        Ok(GatewayStartup::Boot(lease)) => lease,
        Ok(GatewayStartup::OpenSettings(url)) => {
            if invocation.print_url {
                println!("{url}");
            } else if invocation.login || invocation.serve.supervised {
                // A login-triggered start never opens a browser, and a
                // supervised child leaves what the user sees to its
                // supervisor; the running gateway leaves this launch
                // nothing to do.
            } else if let Err(error) = open::that(&url) {
                eprintln!(
                    "could not open the browser: {error}; the running gateway's Settings URL is {url}"
                );
            }
            return ExitCode::SUCCESS;
        }
        Ok(_) => {
            eprintln!("error: unrecognized gateway startup decision");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            print_error_chain(&error);
            return ExitCode::FAILURE;
        }
    };

    // Logging starts only on the serving path: `--help`, `--version`, and
    // a second-instance handoff must not rotate the running gateway's log
    // out from under it.
    let logging = init_logging();

    let result = if invocation.print_url {
        run_printing_url(&invocation.serve)
    } else if invocation.tray {
        run_with_tray(&invocation.serve)
    } else {
        run(&invocation.serve)
    };
    let exit = match result {
        Ok(shutdown_reason) => {
            if logging.is_some() {
                let log_message = match shutdown_reason {
                    ShutdownReason::Stop => "gateway exiting",
                    ShutdownReason::Restart => "gateway restarting",
                };

                tracing::info!(log_message);
            }

            shutdown_reason.exit_code()
        }

        Err(error) => {
            // A fatal error is logged once with its complete source chain;
            // raw stderr is only the fallback when the logger never
            // started.
            if logging.is_some() {
                log_error_chain(&error);
                tracing::error!("gateway exiting after a fatal error");
            } else {
                print_error_chain(&error);
            }
            ExitCode::FAILURE
        }
    };
    // The logger shuts down last, so a healthy sink drains the terminal
    // outcome and every admitted record before exit. A stalled sink gets
    // bounded loss accounting and cannot hold process exit forever.
    if let Some(runtime) = logging
        && let Err(error) = runtime.shutdown()
    {
        eprintln!("could not shut down the log worker: {error}");
    }
    exit
}

/// Test-only process rendezvous used by integration tests that must place
/// multiple production binaries immediately before lease acquisition.
#[cfg(feature = "test-fixtures")]
fn wait_for_test_start_rendezvous() -> anyhow::Result<()> {
    let ready = std::env::var_os(TEST_START_READY_ENV);
    let release = std::env::var_os(TEST_START_RELEASE_ENV);
    let (Some(ready), Some(release)) = (&ready, &release) else {
        return if ready.is_none() && release.is_none() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "{TEST_START_READY_ENV} and {TEST_START_RELEASE_ENV} must be set together"
            ))
        };
    };
    let ready = PathBuf::from(ready);
    let release = PathBuf::from(release);
    std::fs::write(&ready, b"ready")
        .map_err(|error| anyhow::anyhow!("write test start marker {}: {error}", ready.display()))?;
    let deadline = std::time::Instant::now() + TEST_START_TIMEOUT;
    while !release.is_file() {
        if std::time::Instant::now() >= deadline {
            return Err(anyhow::anyhow!(
                "test start release {} did not arrive within {TEST_START_TIMEOUT:?}",
                release.display()
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    Ok(())
}

/// Installs the global subscriber and starts the log pipeline: the filtered
/// stream on stdout, plus the same stream through the bounded queue into
/// `<state dir>/logs/gateway.log`, where the state dir is the
/// `.promptforge` directory the run directory's resolver already knows
/// (it holds `gateway.toml`, `run/`, and `models/`). A log file that cannot
/// be opened warns on stdout and never stops the gateway. The returned
/// runtime must be shut down last.
fn init_logging() -> Option<LogRuntime> {
    let state_dir = gateway_api_discovery::default_run_dir()
        .and_then(|run_dir| run_dir.parent().map(PathBuf::from));
    init_logging_for_state(state_dir)
}

fn init_logging_for_state(state_dir: Option<PathBuf>) -> Option<LogRuntime> {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
    };
    let stdout = tracing_subscriber::fmt::layer().with_filter(filter());
    let runtime = state_dir.map(|state_dir| LogRuntime::start(LogConfig::new(state_dir)));
    match runtime {
        Some(Ok(runtime)) => {
            let file_writer = runtime.writer();
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .fmt_fields(file_writer.clone())
                .with_writer(file_writer)
                .with_filter(filter());
            #[expect(
                clippy::disallowed_methods,
                reason = "the gateway binary's entry point installs the process subscriber"
            )]
            tracing_subscriber::registry()
                .with(stdout)
                .with(file_layer)
                .init();
            tracing::info!("promptforge-gateway {} starting", env!("CARGO_PKG_VERSION"));
            tracing::info!("logging to {}", runtime.path().display());
            Some(runtime)
        }
        Some(Err(error)) => {
            #[expect(
                clippy::disallowed_methods,
                reason = "the gateway binary's entry point installs the process subscriber"
            )]
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("could not start file logging: {error}; logging to stdout only");
            None
        }
        None => {
            #[expect(
                clippy::disallowed_methods,
                reason = "the gateway binary's entry point installs the process subscriber"
            )]
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("no user profile directory found; logging to stdout only");
            None
        }
    }
}

/// Logs the error and its full `source()` chain through the subscriber, so
/// the fatal outcome lands in the drained queue.
fn log_error_chain(error: &dyn std::error::Error) {
    tracing::error!("error: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        tracing::error!("  caused by: {cause}");
        source = cause.source();
    }
}

/// Prints the error and its full `source()` chain to stderr: the fallback
/// when the logger itself never started.
fn print_error_chain(error: &dyn std::error::Error) {
    eprintln!("error: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

#[cfg(test)]
#[path = "main-tests.rs"]
mod tests;

#[cfg(test)]
#[path = "main-logging-tests.rs"]
mod logging_tests;
