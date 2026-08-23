//! The `promptforge-gateway` binary:
//! `promptforge-gateway serve [config.toml] --profile NAME`.
//!
//! This is a thin shell: it parses arguments into a typed [`ServeOptions`] and
//! hands off to [`run`], which owns the tokio runtime, provisioning, and serving.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use promptforge_gateway::{ConfigSource, ProfileName, ServeOptions, run};

const USAGE: &str = concat!(
    "usage: promptforge-gateway serve [config.toml] --profile NAME\n",
    "the config path may also be set with the PROMPTFORGE_GATEWAY_CONFIG environment variable",
);

fn main() -> ExitCode {
    tracing_subscriber::fmt::init();

    let options = match parse_args(std::env::args_os()) {
        Ok(options) => options,
        Err(ParseError::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Err(ParseError::Usage(message)) => {
            eprintln!("error: {message}");
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match run(options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            print_error_chain(&error);
            ExitCode::FAILURE
        }
    }
}

/// Print the error and its full `source()` chain to stderr.
fn print_error_chain(error: &dyn std::error::Error) {
    eprintln!("error: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        eprintln!("  caused by: {cause}");
        source = cause.source();
    }
}

/// Why argument parsing stopped.
#[derive(Debug, PartialEq, Eq)]
enum ParseError {
    /// `-h`/`--help` was requested.
    Help,
    /// The arguments were invalid; the string is the operator-facing reason.
    Usage(String),
}

/// Parse `serve` arguments into typed [`ServeOptions`].
///
/// Uses `OsString` operands so non-UTF-8 config paths survive. Boot requires
/// two things: a config path (the one optional positional, falling back to
/// `PROMPTFORGE_GATEWAY_CONFIG`) and `--profile NAME`, which is validated
/// into a [`ProfileName`] at parse time.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<ServeOptions, ParseError> {
    let mut args = args.into_iter();
    let _binary = args.next();

    match args.next() {
        Some(command) if command == *"serve" => {}
        Some(other) => {
            return Err(ParseError::Usage(format!(
                "unknown command {}",
                other.to_string_lossy()
            )));
        }
        None => return Err(ParseError::Usage("missing 'serve' subcommand".to_string())),
    }

    let mut profile: Option<ProfileName> = None;
    let mut config_path: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--profile") => {
                let name = args
                    .next()
                    .ok_or_else(|| ParseError::Usage("--profile requires a name".to_string()))?;
                let name = name.into_string().map_err(|_| {
                    ParseError::Usage("--profile name must be valid UTF-8".to_string())
                })?;
                let name = ProfileName::parse(&name)
                    .map_err(|error| ParseError::Usage(format!("invalid profile name: {error}")))?;
                profile = Some(name);
            }
            Some("-h" | "--help") => return Err(ParseError::Help),
            Some(other) if other.starts_with('-') => {
                return Err(ParseError::Usage(format!("unknown flag {other}")));
            }
            _ => {
                if config_path.is_some() {
                    return Err(ParseError::Usage(format!(
                        "unexpected argument {}",
                        arg.to_string_lossy()
                    )));
                }
                config_path = Some(PathBuf::from(arg));
            }
        }
    }

    let Some(profile) = profile else {
        return Err(ParseError::Usage(
            "--profile NAME is required; there is no anonymous boot".to_string(),
        ));
    };
    let config_path =
        resolve_config_path(config_path, std::env::var_os("PROMPTFORGE_GATEWAY_CONFIG"))?;

    // Interim mapping until the runner takes {config_path, profile} directly:
    // the profiles dir is the config file's sibling `profiles/`, and the boot
    // source is the named profile within it.
    let profiles_dir = config_path.parent().map_or_else(
        || PathBuf::from("profiles"),
        |parent| parent.join("profiles"),
    );
    Ok(ServeOptions::new(
        profiles_dir,
        ConfigSource::Profile(profile),
    ))
}

/// Resolve the boot config path: the CLI positional wins, then the
/// `PROMPTFORGE_GATEWAY_CONFIG` environment variable.
///
/// Pure, so tests pass both sources explicitly and never touch the process
/// environment (edition 2024 makes `set_var` unsafe).
fn resolve_config_path(cli: Option<PathBuf>, env: Option<OsString>) -> Result<PathBuf, ParseError> {
    match (cli, env) {
        (Some(path), _) => Ok(path),
        (None, Some(value)) => Ok(PathBuf::from(value)),
        (None, None) => Err(ParseError::Usage(
            "provide a config.toml path or set PROMPTFORGE_GATEWAY_CONFIG".into(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<OsString> {
        std::iter::once("promptforge-gateway")
            .chain(items.iter().copied())
            .map(OsString::from)
            .collect()
    }

    #[test]
    fn cli_path_wins_over_env() {
        let path = resolve_config_path(
            Some(PathBuf::from("cli.toml")),
            Some(OsString::from("env.toml")),
        )
        .expect("resolve");
        assert_eq!(path, PathBuf::from("cli.toml"));
    }

    #[test]
    fn env_path_is_the_fallback() {
        let path = resolve_config_path(None, Some(OsString::from("env.toml"))).expect("resolve");
        assert_eq!(path, PathBuf::from("env.toml"));
    }

    #[test]
    fn neither_path_set_is_a_usage_error() {
        let error = resolve_config_path(None, None).unwrap_err();
        assert!(
            matches!(&error, ParseError::Usage(message) if message.contains("PROMPTFORGE_GATEWAY_CONFIG"))
        );
    }

    #[test]
    fn parses_path_and_profile() {
        let options =
            parse_args(args(&["serve", "gateway.toml", "--profile", "dev"])).expect("parse");
        let ConfigSource::Profile(name) = options.source else {
            panic!("expected a profile source");
        };
        assert_eq!(name.to_string(), "dev");
        assert_eq!(options.profiles_dir, PathBuf::from("profiles"));
    }

    #[test]
    fn missing_profile_is_a_usage_error() {
        let error = parse_args(args(&["serve", "gateway.toml"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn invalid_profile_name_is_a_usage_error() {
        let error = parse_args(args(&["serve", "gateway.toml", "--profile", ""])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_traversal_profile_name() {
        let error =
            parse_args(args(&["serve", "gateway.toml", "--profile", "../escape"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_unknown_command() {
        let error = parse_args(args(&["frobnicate"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn requires_serve_subcommand() {
        let error = parse_args(args(&[])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn help_is_recognized() {
        let error = parse_args(args(&["serve", "--help"])).unwrap_err();
        assert_eq!(error, ParseError::Help);
    }

    #[test]
    fn rejects_unknown_flag() {
        let error =
            parse_args(args(&["serve", "--profiles-dir", "x", "--profile", "dev"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_a_second_positional() {
        let error =
            parse_args(args(&["serve", "a.toml", "b.toml", "--profile", "dev"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }
}
