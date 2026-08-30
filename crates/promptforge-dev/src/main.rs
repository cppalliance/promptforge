//! Interactive PromptForge prompt runner against an already-running gateway.
//!
//! `promptforge-dev [--watch] [--capture-raw] <prompt.md> [input]` requires
//! `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_API_KEY`, then runs one
//! prompt file. Context, thinking, and max tokens are declared on the prompt
//! (`models.bind` / `models.always`); this binary does not accept those flags.
//!
//! `--capture-raw` opts into persisting verbatim request and response bodies
//! (full prompts, tool arguments and results, and model output) under
//! `<prompt-stem>/.trace/`. It is off by default because that material
//! is sensitive. Use `--` to pass an input that begins with `--`.

mod config;
mod diagnostics;
mod dump;
mod progress;
mod run;
mod tools;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use promptforge_core::CancelHandle;

use crate::config::{GatewayEnv, require_gateway_env};
use crate::diagnostics::format_dev_failure;
use crate::run::CapturePolicy;

/// Usage text printed beneath every argument error.
const USAGE: &str = "usage: promptforge-dev [--watch] [--capture-raw] <prompt.md> [input]";

/// One parsed invocation.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    /// The prompt file to run.
    prompt: PathBuf,
    /// The prompt input, defaulting to empty.
    input: String,
    /// When `true`, keep watching and rerun on every save.
    watch: bool,
    /// When `true`, persist raw sensitive turn traces (opt-in).
    capture_raw: bool,
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

/// Where the process ended, mapped to a stable exit status at the boundary.
#[derive(Debug)]
enum Drive {
    /// Dispatch finished on its own with this result.
    Completed(Result<()>),
    /// A Ctrl-C signal cancelled the run.
    Interrupted,
    /// The Ctrl-C handler could not be installed.
    SignalError(anyhow::Error),
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> ExitCode {
    // Parse arguments first so a syntactically bad invocation reports the usage
    // error rather than an unrelated missing-credentials message.
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::from(2);
        }
    };
    // Only a valid invocation needs the gateway environment.
    let gateway = match require_gateway_env() {
        Ok(gateway) => gateway,
        Err(error) => {
            eprintln!("{error:#}");
            return ExitCode::from(1);
        }
    };

    let capture = if args.capture_raw {
        eprintln!(
            "warning: --capture-raw persists verbatim prompts, tool arguments and results, \
             and model output to {}",
            run::store_directory(&args.prompt).join(".trace").display()
        );
        CapturePolicy::RawSensitive(dump::SensitiveCapture::authorized())
    } else {
        CapturePolicy::Off
    };

    let prompt_path = args.prompt.clone();
    let cancel = CancelHandle::new();
    let dispatch = dispatch(args, gateway, capture, cancel.clone());

    match Box::pin(drive(dispatch, tokio::signal::ctrl_c(), &cancel)).await {
        Drive::Completed(Ok(())) => ExitCode::SUCCESS,
        Drive::Completed(Err(error)) => {
            eprintln!("{}", format_dev_failure(&prompt_path, &error));
            ExitCode::from(1)
        }
        Drive::Interrupted => {
            eprintln!("interrupted by Ctrl-C");
            ExitCode::from(130)
        }
        Drive::SignalError(error) => {
            eprintln!("{error:#}");
            ExitCode::from(1)
        }
    }
}

/// Runs `dispatch` while watching for a Ctrl-C signal.
///
/// If `dispatch` finishes first, its result is returned. If the signal fires
/// first, the run is cancelled cooperatively and awaited so it unwinds
/// through its own cancellation path rather than being dropped mid-flight,
/// then reported as interrupted. A signal-handler installation error is
/// returned rather than silently dropped.
async fn drive<D, S>(dispatch: D, signal: S, cancel: &CancelHandle) -> Drive
where
    D: Future<Output = Result<()>>,
    S: Future<Output = std::io::Result<()>>,
{
    tokio::pin!(dispatch);
    tokio::pin!(signal);
    tokio::select! {
        result = &mut dispatch => Drive::Completed(result),
        outcome = &mut signal => match outcome {
            Ok(()) => {
                cancel.cancel();
                // Let dispatch observe cancellation and unwind rather than being
                // dropped mid-flight; its result is the expected cancellation.
                let _joined = (&mut dispatch).await;
                Drive::Interrupted
            }
            Err(error) => {
                cancel.cancel();
                let _joined = (&mut dispatch).await;
                Drive::SignalError(
                    anyhow::Error::from(error).context("install the Ctrl-C handler"),
                )
            }
        }
    }
}

/// Routes one parsed invocation to single-shot or watch mode.
async fn dispatch(
    args: Args,
    gateway: GatewayEnv,
    capture: CapturePolicy,
    cancel: CancelHandle,
) -> Result<()> {
    if args.watch {
        return watch::run(&args.prompt, &args.input, &gateway, capture, &cancel).await;
    }
    let result = run::run_once(&args.prompt, &args.input, &gateway, capture, cancel).await?;
    println!("{result}");
    Ok(())
}

/// Parses the argument list after the program name.
///
/// Flags (`--watch`, `--capture-raw`) may appear before the positionals. A
/// bare `--` ends option parsing, so every following token is a positional even
/// if it begins with `--`; this is how an input that starts with `--` is
/// passed literally.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, UsageError> {
    let mut prompt: Option<PathBuf> = None;
    let mut input: Option<String> = None;
    let mut watch = false;
    let mut capture_raw = false;
    let mut positional_only = false;
    for argument in args {
        if positional_only {
            assign_positional(&mut prompt, &mut input, argument)?;
            continue;
        }
        match argument.as_str() {
            "--" => positional_only = true,
            "--watch" => watch = true,
            "--capture-raw" => capture_raw = true,
            flag if flag.starts_with("--") => {
                return Err(UsageError(format!("unknown flag `{flag}`")));
            }
            _ => assign_positional(&mut prompt, &mut input, argument)?,
        }
    }
    let Some(prompt) = prompt else {
        return Err(UsageError("requires a prompt file".to_owned()));
    };
    Ok(Args {
        prompt,
        input: input.unwrap_or_default(),
        watch,
        capture_raw,
    })
}

/// Assigns one positional token to the prompt, then the input, then rejects a
/// surplus.
fn assign_positional(
    prompt: &mut Option<PathBuf>,
    input: &mut Option<String>,
    argument: String,
) -> Result<(), UsageError> {
    if prompt.is_none() {
        *prompt = Some(PathBuf::from(&argument));
        Ok(())
    } else if input.is_none() {
        *input = Some(argument);
        Ok(())
    } else {
        Err(UsageError(format!("unexpected argument `{argument}`")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Args, UsageError> {
        parse_args(list.iter().map(|argument| (*argument).to_owned()))
    }

    fn args(prompt: &str, input: &str, watch: bool, capture_raw: bool) -> Args {
        Args {
            prompt: PathBuf::from(prompt),
            input: input.to_owned(),
            watch,
            capture_raw,
        }
    }

    #[test]
    fn prompt_alone_uses_empty_input_and_no_flags() {
        assert_eq!(
            parse(&["prompt.md"]),
            Ok(args("prompt.md", "", false, false))
        );
    }

    #[test]
    fn parses_input_and_watch() {
        assert_eq!(
            parse(&["prompt.md", "hello", "--watch"]),
            Ok(args("prompt.md", "hello", true, false))
        );
    }

    #[test]
    fn parses_the_capture_raw_flag() {
        assert_eq!(
            parse(&["--capture-raw", "prompt.md", "hello"]),
            Ok(args("prompt.md", "hello", false, true))
        );
    }

    #[test]
    fn watch_may_precede_the_positionals() {
        assert_eq!(
            parse(&["--watch", "prompt.md", "hello"]),
            Ok(args("prompt.md", "hello", true, false))
        );
    }

    #[test]
    fn a_double_dash_lets_an_input_begin_with_dashes() {
        assert_eq!(
            parse(&["prompt.md", "--", "--verbose"]),
            Ok(args("prompt.md", "--verbose", false, false))
        );
    }

    #[test]
    fn a_double_dash_can_precede_a_dash_prefixed_prompt() {
        assert_eq!(
            parse(&["--", "--weird-name.md"]),
            Ok(args("--weird-name.md", "", false, false))
        );
    }

    #[test]
    fn a_surplus_positional_after_the_delimiter_is_rejected() {
        assert_eq!(
            parse(&["prompt.md", "--", "input", "surplus"]),
            Err(UsageError("unexpected argument `surplus`".to_owned()))
        );
    }

    #[test]
    fn requires_a_prompt_file() {
        assert_eq!(
            parse(&[]),
            Err(UsageError("requires a prompt file".to_owned()))
        );
        assert_eq!(
            parse(&["--watch"]),
            Err(UsageError("requires a prompt file".to_owned()))
        );
    }

    #[test]
    fn rejects_a_third_positional() {
        assert_eq!(
            parse(&["prompt.md", "hello", "surplus"]),
            Err(UsageError("unexpected argument `surplus`".to_owned()))
        );
    }

    #[test]
    fn rejects_unknown_flags_including_former_server_knobs() {
        for flag in ["--verbose", "--context", "--max-tokens", "--no-think"] {
            assert_eq!(
                parse(&["prompt.md", flag]),
                Err(UsageError(format!("unknown flag `{flag}`"))),
                "{flag}"
            );
        }
    }

    #[test]
    fn usage_errors_render_the_problem_above_the_usage_text() {
        assert_eq!(
            UsageError("unknown flag `--context`".to_owned()).to_string(),
            format!("unknown flag `--context`\n{USAGE}")
        );
    }

    #[tokio::test]
    async fn dispatch_completing_first_is_reported_as_completed() {
        let cancel = CancelHandle::new();
        let dispatch = async { Ok(()) };
        let signal = std::future::pending::<std::io::Result<()>>();
        match drive(dispatch, signal, &cancel).await {
            Drive::Completed(Ok(())) => {}
            other => panic!("expected Completed(Ok), got {other:?}"),
        }
        assert!(
            !cancel.is_cancelled(),
            "no signal fired, so nothing cancels"
        );
    }

    #[tokio::test]
    async fn a_ctrl_c_signal_cancels_then_reports_interrupted() {
        let cancel = CancelHandle::new();
        let cancel_observer = cancel.clone();
        // A cooperative run finishes only once cancellation fires.
        let dispatch = async move {
            cancel_observer.cancelled().await;
            Ok(())
        };
        let signal = async { Ok(()) };
        match drive(dispatch, signal, &cancel).await {
            Drive::Interrupted => {}
            other => panic!("expected Interrupted, got {other:?}"),
        }
        assert!(cancel.is_cancelled(), "the signal must cancel the run");
    }

    #[tokio::test]
    async fn a_signal_install_error_is_returned_not_dropped() {
        let cancel = CancelHandle::new();
        let cancel_observer = cancel.clone();
        let dispatch = async move {
            cancel_observer.cancelled().await;
            Ok(())
        };
        let signal = async { Err(std::io::Error::other("no signal backend")) };
        match drive(dispatch, signal, &cancel).await {
            Drive::SignalError(error) => assert!(
                format!("{error:#}").contains("no signal backend"),
                "the signal error must be preserved: {error:#}"
            ),
            other => panic!("expected SignalError, got {other:?}"),
        }
        assert!(cancel.is_cancelled(), "a signal error must still cancel");
    }
}
