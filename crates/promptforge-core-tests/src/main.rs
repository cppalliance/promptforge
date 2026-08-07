//! Explicit real-model entry point plus offline prompt fixture tests.

mod artifacts;
mod dev;
mod scenarios;
mod server;
mod watch;

#[cfg(test)]
mod suite;

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context as _, Result, bail};

use crate::server::{DevServerOptions, ServerGuard, ServerProfile};

/// Usage text printed beneath every argument error.
const USAGE: &str = "usage:
  promptforge-core-tests [scenarios]
  promptforge-core-tests dev <prompt-file> [input] [--watch] [--context N] [--max-tokens N] [--no-think]";

/// One parsed invocation of this binary.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// Run the fixed deterministic scenario suite, the default.
    Scenarios,
    /// Run one prompt file against the dev-profile server.
    Dev(DevCommand),
}

/// Everything the dev path needs, parsed from the `dev` subcommand arguments.
#[derive(Debug, PartialEq, Eq)]
struct DevCommand {
    /// The prompt file to run.
    prompt: PathBuf,
    /// The prompt input, defaulting to empty.
    input: String,
    /// When `true`, keep the server warm and rerun on every save.
    watch: bool,
    /// Server knobs assembled from `--context`, `--max-tokens`, and
    /// `--no-think`.
    options: DevServerOptions,
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

/// Where dispatch-level status lines go.
///
/// The scenario suite keeps its status lines on stdout, byte-identical to the
/// original output; dev mode reserves stdout for the final result, so its
/// status lines go to stderr.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusStream {
    /// Status lines print to stdout, the scenario-suite contract.
    Stdout,
    /// Status lines print to stderr, the dev-mode contract.
    Stderr,
}

impl StatusStream {
    /// Selects the status stream for provisioning `kind`.
    fn for_kind(kind: artifacts::ModelKind) -> Self {
        match kind {
            artifacts::ModelKind::Scenario => Self::Stdout,
            artifacts::ModelKind::Dev => Self::Stderr,
        }
    }

    /// Writes `line` to whichever of `stdout` and `stderr` this stream names.
    fn write_line(
        self,
        stdout: &mut impl std::io::Write,
        stderr: &mut impl std::io::Write,
        line: &str,
    ) -> std::io::Result<()> {
        match self {
            Self::Stdout => writeln!(stdout, "{line}"),
            Self::Stderr => writeln!(stderr, "{line}"),
        }
    }

    /// Emits `line` on the process's own streams.
    fn emit(self, line: &str) {
        // Status lines are advisory; a closed pipe must not fail the run.
        let _ = self.write_line(&mut std::io::stdout(), &mut std::io::stderr(), line);
    }
}

/// Selects the failure prefix for `command`: the scenario wording stays
/// byte-identical, and the dev wording matches the watch loop's rendering.
fn failure_prefix(command: &Command) -> &'static str {
    match command {
        Command::Scenarios => "real-model suite failed",
        Command::Dev(_) => "dev run failed",
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    let command = match parse_args(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let prefix = failure_prefix(&command);
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
            eprintln!("{prefix}: {error:#}");
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
        "dev" => parse_dev(args),
        other => Err(UsageError(format!("unknown command `{other}`"))),
    }
}

/// Parses everything after the `dev` subcommand into a [`DevCommand`].
fn parse_dev(mut args: impl Iterator<Item = String>) -> Result<Command, UsageError> {
    let mut prompt: Option<PathBuf> = None;
    let mut input: Option<String> = None;
    let mut watch = false;
    let mut options = DevServerOptions::default();
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--watch" => watch = true,
            "--no-think" => options.think = false,
            "--context" => options.ctx_size = parse_count("--context", args.next())?,
            "--max-tokens" => options.n_predict = parse_count("--max-tokens", args.next())?,
            flag if flag.starts_with("--") => {
                return Err(UsageError(format!("unknown flag `{flag}`")));
            }
            _ if prompt.is_none() => prompt = Some(PathBuf::from(&argument)),
            _ if input.is_none() => input = Some(argument),
            extra => return Err(UsageError(format!("unexpected argument `{extra}`"))),
        }
    }
    let Some(prompt) = prompt else {
        return Err(UsageError("dev requires a prompt file".to_owned()));
    };
    Ok(Command::Dev(DevCommand {
        prompt,
        input: input.unwrap_or_default(),
        watch,
        options,
    }))
}

/// Parses one numeric flag value as a positive integer.
fn parse_count(flag: &str, value: Option<String>) -> Result<u32, UsageError> {
    let Some(value) = value else {
        return Err(UsageError(format!("{flag} requires a value")));
    };
    match value.parse::<u32>() {
        Ok(count) if count > 0 => Ok(count),
        Ok(_) | Err(_) => Err(UsageError(format!(
            "{flag} requires a positive integer, got `{value}`"
        ))),
    }
}

/// Routes one parsed command to its path under the caller's Ctrl-C wrapper.
async fn run_command(command: Command, interrupted: Arc<AtomicBool>) -> Result<()> {
    match command {
        Command::Scenarios => run_explicit_suite(interrupted).await,
        Command::Dev(dev) => run_dev(dev, interrupted).await,
    }
}

/// Provisions `kind`'s pinned artifacts off the runtime and starts the
/// guarded server with `profile`.
async fn provision_and_start(
    kind: artifacts::ModelKind,
    profile: ServerProfile,
    interrupted: Arc<AtomicBool>,
) -> Result<ServerGuard> {
    let status = StatusStream::for_kind(kind);
    status.emit("provisioning pinned real-model artifacts");
    let artifacts = tokio::task::spawn_blocking(move || artifacts::provision(kind))
        .await
        .context("join artifact provisioner")?
        .context("provision pinned real-model artifacts")?;
    status.emit("pinned artifacts are ready");

    let server_executable = artifacts.llama_server;
    let model = artifacts.model;
    let server = tokio::task::spawn_blocking(move || {
        ServerGuard::start(&server_executable, &model, profile, &interrupted)
    })
    .await
    .context("join llama-server startup")??;
    status.emit(&format!("llama-server is ready at {}", server.base_url()));
    Ok(server)
}

/// Runs the fixed deterministic scenario suite against the scenario profile.
async fn run_explicit_suite(interrupted: Arc<AtomicBool>) -> Result<()> {
    let server = provision_and_start(
        artifacts::ModelKind::Scenario,
        ServerProfile::Scenario,
        interrupted,
    )
    .await?;
    let result =
        scenarios::run_all(&server.base_url(), server.api_key(), server.model_alias()).await;
    if let Err(error) = result {
        bail!("{error:#}\n{}", server.diagnostics());
    }
    println!("real-model suite passed");
    Ok(())
}

/// Runs one prompt against the dev profile, single-shot or in watch mode.
async fn run_dev(command: DevCommand, interrupted: Arc<AtomicBool>) -> Result<()> {
    let server = provision_and_start(
        artifacts::ModelKind::Dev,
        ServerProfile::Dev(command.options),
        interrupted,
    )
    .await?;
    if command.watch {
        return watch::run(&command.prompt, &command.input, &server).await;
    }
    match dev::run_once(
        &command.prompt,
        &command.input,
        &server.base_url(),
        server.api_key(),
        server.model_alias(),
    )
    .await
    {
        Ok(result) => {
            println!("{result}");
            Ok(())
        }
        Err(error) => bail!("{error:#}\n{}", server.diagnostics()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Command, UsageError> {
        parse_args(list.iter().map(|argument| (*argument).to_owned()))
    }

    fn dev_command(prompt: &str) -> DevCommand {
        DevCommand {
            prompt: PathBuf::from(prompt),
            input: String::new(),
            watch: false,
            options: DevServerOptions::default(),
        }
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
    fn dev_with_a_prompt_alone_uses_every_default() {
        assert_eq!(
            parse(&["dev", "prompt.md"]),
            Ok(Command::Dev(dev_command("prompt.md")))
        );
    }

    #[test]
    fn dev_parses_input_watch_and_every_numeric_flag() {
        assert_eq!(
            parse(&[
                "dev",
                "prompt.md",
                "hello",
                "--watch",
                "--context",
                "32768",
                "--max-tokens",
                "512",
                "--no-think",
            ]),
            Ok(Command::Dev(DevCommand {
                prompt: PathBuf::from("prompt.md"),
                input: "hello".to_owned(),
                watch: true,
                options: DevServerOptions {
                    ctx_size: 32_768,
                    n_predict: 512,
                    think: false,
                },
            }))
        );
    }

    #[test]
    fn dev_flags_may_precede_the_positionals() {
        assert_eq!(
            parse(&["dev", "--watch", "prompt.md", "hello"]),
            Ok(Command::Dev(DevCommand {
                input: "hello".to_owned(),
                watch: true,
                ..dev_command("prompt.md")
            }))
        );
    }

    #[test]
    fn dev_requires_a_prompt_file() {
        assert_eq!(
            parse(&["dev"]),
            Err(UsageError("dev requires a prompt file".to_owned()))
        );
        assert_eq!(
            parse(&["dev", "--watch"]),
            Err(UsageError("dev requires a prompt file".to_owned()))
        );
    }

    #[test]
    fn dev_rejects_a_third_positional() {
        assert_eq!(
            parse(&["dev", "prompt.md", "hello", "surplus"]),
            Err(UsageError("unexpected argument `surplus`".to_owned()))
        );
    }

    #[test]
    fn dev_rejects_unknown_flags() {
        assert_eq!(
            parse(&["dev", "prompt.md", "--verbose"]),
            Err(UsageError("unknown flag `--verbose`".to_owned()))
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
    fn numeric_flags_reject_missing_zero_and_malformed_values() {
        for (arguments, expected) in [
            (
                &["dev", "prompt.md", "--context"][..],
                "--context requires a value",
            ),
            (
                &["dev", "prompt.md", "--context", "abc"][..],
                "--context requires a positive integer, got `abc`",
            ),
            (
                &["dev", "prompt.md", "--context", "0"][..],
                "--context requires a positive integer, got `0`",
            ),
            (
                &["dev", "prompt.md", "--max-tokens", "-5"][..],
                "--max-tokens requires a positive integer, got `-5`",
            ),
            (
                &["dev", "prompt.md", "--max-tokens", "1.5"][..],
                "--max-tokens requires a positive integer, got `1.5`",
            ),
        ] {
            assert_eq!(
                parse(arguments),
                Err(UsageError(expected.to_owned())),
                "arguments: {arguments:?}"
            );
        }
    }

    #[test]
    fn the_failure_prefix_follows_the_parsed_command() {
        assert_eq!(
            failure_prefix(&Command::Scenarios),
            "real-model suite failed"
        );
        assert_eq!(
            failure_prefix(&Command::Dev(dev_command("prompt.md"))),
            "dev run failed"
        );
    }

    #[test]
    fn dev_status_lines_route_to_stderr_and_scenario_lines_to_stdout() {
        assert_eq!(
            StatusStream::for_kind(artifacts::ModelKind::Scenario),
            StatusStream::Stdout
        );
        assert_eq!(
            StatusStream::for_kind(artifacts::ModelKind::Dev),
            StatusStream::Stderr
        );

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        StatusStream::Stdout
            .write_line(&mut stdout, &mut stderr, "scenario status")
            .unwrap();
        StatusStream::Stderr
            .write_line(&mut stdout, &mut stderr, "dev status")
            .unwrap();
        assert_eq!(stdout, b"scenario status\n");
        assert_eq!(stderr, b"dev status\n");
    }

    #[test]
    fn usage_errors_render_the_problem_above_the_pinned_usage_text() {
        assert_eq!(
            UsageError("unknown command `x`".to_owned()).to_string(),
            "unknown command `x`\n\
             usage:\n\
             \x20 promptforge-core-tests [scenarios]\n\
             \x20 promptforge-core-tests dev <prompt-file> [input] [--watch] [--context N] [--max-tokens N] [--no-think]"
        );
    }
}
