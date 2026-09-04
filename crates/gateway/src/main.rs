//! The `promptforge-gateway` binary:
//! `promptforge-gateway serve [config.toml] [--profile NAME] [--no-tray] [--login] [--print-url]`.
//!
//! This is a thin shell: it parses arguments into a typed [`ServeOptions`] and
//! hands off to [`run_with_tray`], which owns the tokio runtime, provisioning,
//! and serving while the system tray occupies the main thread. `--no-tray`
//! keeps the headless Ctrl-C loop ([`run`]) for servers and CI. With no config
//! path from either source, the gateway runs boot discovery and, on first run,
//! generates a default config. A second launch while a gateway is already
//! running never boots a duplicate: it opens the running gateway's Settings
//! page (or prints its URL under `--print-url`) and exits.

use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use gateway::{ProfileName, ServeOptions, run, run_printing_url, run_with_tray};

const USAGE: &str = concat!(
    "usage: promptforge-gateway serve [config.toml] [--profile NAME] [--no-tray] [--login] [--print-url]\n",
    "       promptforge-gateway --version\n",
    "the config path may also be set with the PROMPTFORGE_GATEWAY_CONFIG environment variable\n",
    "with no config path, the gateway searches beside the executable, the current directory,\n",
    "and the profile's .promptforge directory, generating a default config on first run\n",
    "--no-tray    run headless (Ctrl-C driven); for servers and CI\n",
    "--login      the launch came from the OS autostart entry; never opens a browser\n",
    "--print-url  print the Settings handoff URL once bound, then serve headless;\n",
    "             with a gateway already running, print its URL instead",
);

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("whisper_cpp=warn")),
        )
        .init();

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

    // A second launch never boots a duplicate server: when a live gateway
    // owns the connection file, hand off to it and exit. This runs before
    // any bind attempt; on the desktop it is also the `.desktop` launcher's
    // relaunch behavior.
    if let Some(url) = gateway::running_gateway_settings_url(&invocation.serve) {
        if invocation.print_url {
            println!("{url}");
        } else if invocation.login {
            // A login-triggered start never opens a browser; the running
            // gateway leaves this launch nothing to do.
            tracing::info!("a gateway is already running; the login-triggered launch exits");
        } else {
            tracing::info!("a gateway is already running; opening its Settings page");
            if let Err(error) = open::that(&url) {
                tracing::warn!(
                    "could not open the browser: {error}; the running gateway's Settings URL is {url}"
                );
            }
        }
        return ExitCode::SUCCESS;
    }

    let result = if invocation.print_url {
        run_printing_url(&invocation.serve)
    } else if invocation.tray {
        run_with_tray(&invocation.serve)
    } else {
        run(&invocation.serve)
    };
    match result {
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
    /// `--version` was requested.
    Version,
    /// The arguments were invalid; the string is the operator-facing reason.
    Usage(String),
}

/// The parsed invocation: the serve options plus how the main thread runs.
#[derive(Debug)]
struct Invocation {
    /// What to serve.
    serve: ServeOptions,
    /// Whether the system tray occupies the main thread (default).
    /// `--no-tray` keeps the headless Ctrl-C loop for servers and CI.
    tray: bool,
    /// Whether the launch came from the OS autostart entry (`--login`).
    login: bool,
    /// Whether to print the Settings handoff URL to stdout (`--print-url`).
    /// Implies the headless loop: the flag exists for tray-less
    /// environments.
    print_url: bool,
}

/// Parse `serve` arguments into a typed [`Invocation`].
///
/// Uses `OsString` operands so non-UTF-8 config paths survive. The config
/// path (the one optional positional, falling back to
/// `PROMPTFORGE_GATEWAY_CONFIG`) stays optional: with neither set, the
/// gateway discovers or generates the boot config itself. `--profile NAME`
/// is validated into a [`ProfileName`] at parse time.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, ParseError> {
    let mut args = args.into_iter();
    let _binary = args.next();

    match args.next() {
        Some(command) if command == *"serve" => {}
        Some(flag) if flag == *"--version" => return Err(ParseError::Version),
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
    let mut tray = true;
    let mut login = false;
    let mut print_url = false;

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
            Some("--no-tray") => tray = false,
            Some("--login") => login = true,
            Some("--print-url") => print_url = true,
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

    let config_path =
        resolve_config_path(config_path, std::env::var_os("PROMPTFORGE_GATEWAY_CONFIG"));

    Ok(Invocation {
        serve: ServeOptions::new(config_path, profile),
        tray,
        login,
        print_url,
    })
}

/// Resolves the config path: the CLI positional wins, then the
/// `PROMPTFORGE_GATEWAY_CONFIG` environment variable. `None` when neither
/// is set, deferring to the gateway's boot discovery and first-run
/// generation.
///
/// Pure, so tests pass both sources explicitly and never touch the process
/// environment (edition 2024 makes `set_var` unsafe).
fn resolve_config_path(cli: Option<PathBuf>, env: Option<OsString>) -> Option<PathBuf> {
    cli.or_else(|| env.map(PathBuf::from))
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
        );
        assert_eq!(path, Some(PathBuf::from("cli.toml")));
    }

    #[test]
    fn env_path_is_the_fallback() {
        let path = resolve_config_path(None, Some(OsString::from("env.toml")));
        assert_eq!(path, Some(PathBuf::from("env.toml")));
    }

    #[test]
    fn neither_path_set_defers_to_boot_discovery() {
        let path = resolve_config_path(None, None);
        assert_eq!(path, None, "the gateway discovers or generates the config");
    }

    #[test]
    fn parses_path_and_profile() {
        let invocation =
            parse_args(args(&["serve", "gateway.toml", "--profile", "dev"])).expect("parse");
        assert_eq!(
            invocation.serve.profile.as_ref().map(ProfileName::as_str),
            Some("dev")
        );
        assert_eq!(
            invocation.serve.config_path,
            Some(PathBuf::from("gateway.toml"))
        );
    }

    #[test]
    fn the_tray_is_default_and_login_is_off() {
        let invocation = parse_args(args(&["serve", "gateway.toml"])).expect("parse");
        assert!(invocation.tray, "the tray is the default main loop");
        assert!(!invocation.login);
    }

    #[test]
    fn no_tray_selects_the_headless_loop() {
        let invocation = parse_args(args(&["serve", "--no-tray"])).expect("parse");
        assert!(!invocation.tray);
        assert!(!invocation.login);
    }

    #[test]
    fn the_autostart_command_line_parses() {
        // The Run-key entry is `"<exe>" --login`; a login launch must never
        // fail on its own flag.
        let invocation = parse_args(args(&["serve", "--login"])).expect("parse");
        assert!(invocation.login);
        assert!(invocation.tray, "a login launch still shows the tray");
    }

    #[test]
    fn print_url_parses_and_leaves_the_other_flags_alone() {
        let invocation = parse_args(args(&["serve", "--print-url"])).expect("parse");
        assert!(invocation.print_url);
        assert!(
            invocation.tray,
            "the flag is independent; the dispatch makes it headless"
        );
        assert!(!invocation.login);
    }

    #[test]
    fn print_url_combines_with_no_tray_and_a_config_path() {
        let invocation = parse_args(args(&["serve", "gateway.toml", "--no-tray", "--print-url"]))
            .expect("parse");
        assert!(invocation.print_url);
        assert!(!invocation.tray);
        assert_eq!(
            invocation.serve.config_path,
            Some(PathBuf::from("gateway.toml"))
        );
    }

    #[test]
    fn missing_profile_defers_to_environment_or_state() {
        let invocation = parse_args(args(&["serve", "gateway.toml"])).expect("parse");
        assert!(invocation.serve.profile.is_none());
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
    fn version_is_recognized() {
        let error = parse_args(args(&["--version"])).unwrap_err();
        assert_eq!(error, ParseError::Version);
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
