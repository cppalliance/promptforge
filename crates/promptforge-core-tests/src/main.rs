//! Offline prompt fixture tests plus the explicit 0.6B scenario harness.

mod gateway;
mod scenarios;

#[cfg(test)]
mod suite;

use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::gateway::{GatewayGuard, GatewayProfile};

/// Usage text printed beneath every argument error.
const USAGE: &str = "usage:
  promptforge-core-tests [scenarios]";

/// One parsed invocation of this binary.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the fixed deterministic scenario suite, the default.
    Scenarios,
}

/// A rejected argument list: the one-line problem plus the usage text.
#[derive(Debug, PartialEq, Eq)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}\n{USAGE}", self.0)
    }
}

impl std::error::Error for UsageError {}

#[tokio::main]
async fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let interrupted = Arc::new(AtomicBool::new(false));
    let result = tokio::select! {
        result = run_command(command, Arc::clone(&interrupted)) => result,
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

/// Parses the argument list after the program name into a [`Command`].
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Command, UsageError> {
    let mut args = args.into_iter();
    let Some(first) = args.next() else {
        return Ok(Command::Scenarios);
    };
    match first.as_str() {
        "scenarios" => match args.next() {
            None => Ok(Command::Scenarios),
            Some(extra) => Err(UsageError(format!(
                "unexpected argument `{extra}` after `scenarios`"
            ))),
        },
        other => Err(UsageError(format!("unknown command `{other}`"))),
    }
}

/// Routes one parsed command to its path under the caller's Ctrl-C wrapper.
async fn run_command(command: Command, interrupted: Arc<AtomicBool>) -> Result<()> {
    match command {
        Command::Scenarios => run_explicit_suite(interrupted).await,
    }
}

/// Starts `promptforge-gateway` with the scenario profile.
async fn start_gateway(interrupted: Arc<AtomicBool>) -> Result<GatewayGuard> {
    println!("starting promptforge-gateway with generated profile");
    let server = tokio::task::spawn_blocking(move || {
        GatewayGuard::start(GatewayProfile::Scenario, &interrupted)
    })
    .await
    .context("join promptforge-gateway startup")??;
    println!("promptforge-gateway is ready at {}", server.base_url());
    Ok(server)
}

/// Runs the fixed deterministic scenario suite against the scenario profile.
async fn run_explicit_suite(interrupted: Arc<AtomicBool>) -> Result<()> {
    let server = start_gateway(interrupted).await?;
    let result =
        scenarios::run_all(&server.base_url(), server.api_key(), server.model_alias()).await;
    if let Err(error) = result {
        bail!("{error:#}\n{}", server.diagnostics());
    }
    println!("real-model suite passed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Command, UsageError> {
        parse_args(list.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn empty_args_default_to_the_scenario_suite() {
        assert_eq!(parse(&[]), Ok(Command::Scenarios));
    }

    #[test]
    fn the_explicit_scenarios_subcommand_matches_the_default() {
        assert_eq!(parse(&["scenarios"]), Ok(Command::Scenarios));
    }

    #[test]
    fn scenarios_rejects_trailing_arguments() {
        assert_eq!(
            parse(&["scenarios", "extra"]),
            Err(UsageError(
                "unexpected argument `extra` after `scenarios`".to_owned()
            ))
        );
    }

    #[test]
    fn unknown_commands_are_rejected() {
        assert_eq!(
            parse(&["frobnicate"]),
            Err(UsageError("unknown command `frobnicate`".to_owned()))
        );
    }

    #[test]
    fn former_dev_subcommand_is_rejected_as_unknown() {
        assert_eq!(
            parse(&["dev", "prompt.md"]),
            Err(UsageError("unknown command `dev`".to_owned()))
        );
    }

    #[test]
    fn usage_errors_render_the_problem_above_the_pinned_usage_text() {
        assert_eq!(
            UsageError("unknown command `x`".to_owned()).to_string(),
            "unknown command `x`\n\
             usage:\n\
             \x20 promptforge-core-tests [scenarios]"
        );
    }
}
