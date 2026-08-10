//! The `promptforge-gateway` binary:
//! `promptforge-gateway serve [--profiles-dir DIR] [--profile NAME] [config.toml]`.
//!
//! This is a thin shell: it parses arguments into a typed [`ServeOptions`] and
//! hands off to [`run`], which owns the tokio runtime, provisioning, and serving.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use promptforge_gateway::{ConfigSource, ProfileName, ServeOptions, default_profiles_dir, run};

const USAGE: &str =
    "usage: promptforge-gateway serve [--profiles-dir DIR] [--profile NAME] [config.toml]";

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
/// Uses `OsString` operands so non-UTF-8 config paths survive; `--profile`
/// names must be valid UTF-8 and parse as a [`ProfileName`]. A profile and an
/// explicit config path are mutually exclusive.
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

    let mut profiles_dir: Option<PathBuf> = None;
    let mut profile: Option<String> = None;
    let mut config_path: Option<PathBuf> = None;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--profiles-dir") => {
                let dir = args.next().ok_or_else(|| {
                    ParseError::Usage("--profiles-dir requires a path".to_string())
                })?;
                profiles_dir = Some(PathBuf::from(dir));
            }
            Some("--profile") => {
                let name = args
                    .next()
                    .ok_or_else(|| ParseError::Usage("--profile requires a name".to_string()))?;
                let name = name.into_string().map_err(|_| {
                    ParseError::Usage("--profile name must be valid UTF-8".to_string())
                })?;
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

    let profiles_dir = profiles_dir.unwrap_or_else(default_profiles_dir);
    let source = match (profile, config_path) {
        (Some(_), Some(_)) => {
            return Err(ParseError::Usage(
                "pass either --profile or a config path, not both".to_string(),
            ));
        }
        (Some(name), None) => {
            let name = ProfileName::parse(&name)
                .map_err(|error| ParseError::Usage(format!("invalid profile name: {error}")))?;
            ConfigSource::Profile(name)
        }
        (None, Some(path)) => ConfigSource::Path(path),
        (None, None) => {
            return Err(ParseError::Usage(
                "provide --profile NAME or a config.toml path".to_string(),
            ));
        }
    };

    Ok(ServeOptions::new(profiles_dir, source))
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
    fn parses_profile_source() {
        let options = parse_args(args(&["serve", "--profile", "dev"])).expect("parse");
        assert!(matches!(options.source, ConfigSource::Profile(_)));
    }

    #[test]
    fn parses_config_path_source() {
        let options = parse_args(args(&["serve", "gateway.toml"])).expect("parse");
        assert!(matches!(options.source, ConfigSource::Path(_)));
    }

    #[test]
    fn profile_and_path_are_mutually_exclusive() {
        let error = parse_args(args(&["serve", "--profile", "dev", "gateway.toml"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn requires_a_source() {
        let error = parse_args(args(&["serve"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_unknown_command() {
        let error = parse_args(args(&["frobnicate"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn help_is_recognized() {
        let error = parse_args(args(&["serve", "--help"])).unwrap_err();
        assert_eq!(error, ParseError::Help);
    }

    #[test]
    fn rejects_traversal_profile_name() {
        let error = parse_args(args(&["serve", "--profile", "../escape"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }
}
