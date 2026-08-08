//! Interactive PromptForge prompt runner against an already-running gateway.
//!
//! `promptforge-dev <prompt.md> [input] [--watch]` requires
//! `PROMPTFORGE_GATEWAY_URL` and `PROMPTFORGE_GATEWAY_KEY`, then runs one
//! prompt file. Context, thinking, and max tokens are declared on the prompt
//! (`models.need` / `models.always`); this binary does not accept those flags.

mod dump;
mod run;
mod tools;
mod watch;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context as _, Result};

use crate::run::require_gateway_env;

/// Usage text printed beneath every argument error.
const USAGE: &str = "usage: promptforge-dev <prompt.md> [input] [--watch]";

/// One parsed invocation.
#[derive(Debug, PartialEq, Eq)]
struct Args {
    /// The prompt file to run.
    prompt: PathBuf,
    /// The prompt input, defaulting to empty.
    input: String,
    /// When `true`, keep watching and rerun on every save.
    watch: bool,
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
    // Fail on missing gateway credentials before any argument-driven parse of
    // a prompt file, so authors see a clear "start the gateway" message.
    if let Err(error) = require_gateway_env() {
        eprintln!("{error:#}");
        return ExitCode::FAILURE;
    }

    let args = match parse_args(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let prompt_path = args.prompt.clone();

    let result = tokio::select! {
        result = dispatch(args) => result,
        signal = tokio::signal::ctrl_c() => {
            match signal {
                Ok(()) => Err(anyhow::anyhow!("interrupted by Ctrl-C")),
                Err(error) => Err(error).context("install Ctrl-C handler"),
            }
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{}", run::format_dev_failure(&prompt_path, &error));
            ExitCode::FAILURE
        }
    }
}

/// Routes one parsed invocation to single-shot or watch mode.
async fn dispatch(args: Args) -> Result<()> {
    if args.watch {
        return watch::run(&args.prompt, &args.input).await;
    }
    let result = run::run_once(&args.prompt, &args.input).await?;
    println!("{result}");
    Ok(())
}

/// Parses the argument list after the program name.
fn parse_args(args: impl IntoIterator<Item = String>) -> Result<Args, UsageError> {
    let mut prompt: Option<PathBuf> = None;
    let mut input: Option<String> = None;
    let mut watch = false;
    for argument in args {
        match argument.as_str() {
            "--watch" => watch = true,
            flag if flag.starts_with("--") => {
                return Err(UsageError(format!("unknown flag `{flag}`")));
            }
            _ if prompt.is_none() => prompt = Some(PathBuf::from(&argument)),
            _ if input.is_none() => input = Some(argument),
            extra => return Err(UsageError(format!("unexpected argument `{extra}`"))),
        }
    }
    let Some(prompt) = prompt else {
        return Err(UsageError("requires a prompt file".to_owned()));
    };
    Ok(Args {
        prompt,
        input: input.unwrap_or_default(),
        watch,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(list: &[&str]) -> Result<Args, UsageError> {
        parse_args(list.iter().map(|argument| (*argument).to_owned()))
    }

    #[test]
    fn prompt_alone_uses_empty_input_and_no_watch() {
        assert_eq!(
            parse(&["prompt.md"]),
            Ok(Args {
                prompt: PathBuf::from("prompt.md"),
                input: String::new(),
                watch: false,
            })
        );
    }

    #[test]
    fn parses_input_and_watch() {
        assert_eq!(
            parse(&["prompt.md", "hello", "--watch"]),
            Ok(Args {
                prompt: PathBuf::from("prompt.md"),
                input: "hello".to_owned(),
                watch: true,
            })
        );
    }

    #[test]
    fn watch_may_precede_the_positionals() {
        assert_eq!(
            parse(&["--watch", "prompt.md", "hello"]),
            Ok(Args {
                prompt: PathBuf::from("prompt.md"),
                input: "hello".to_owned(),
                watch: true,
            })
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
            "unknown flag `--context`\n\
             usage: promptforge-dev <prompt.md> [input] [--watch]"
        );
    }
}
