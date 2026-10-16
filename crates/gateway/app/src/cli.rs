use std::{ffi::OsString, path::PathBuf};

use gateway::{ProfileName, ServeOptions};

pub(crate) const USAGE: &str = concat!(
    "usage: runvil-gateway [--config PATH] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]\n",
    "       runvil-gateway diagnostics [--config PATH]\n",
    "       runvil-gateway --version\n",
    "the config path may also be set with the RUNVIL_GATEWAY_CONFIG environment variable;\n",
    "--config wins over it\n",
    "with no config path, the gateway searches beside the executable, the current directory,\n",
    "and the profile's .runvil directory, generating a default config on first run\n",
    "diagnostics  print a JSON report of the state dir, config, logs, and gateway discovery file;\n",
    "             never serves, rotates logs, or parses the config\n",
    "--no-tray    run headless (Ctrl-C driven); for servers and CI\n",
    "--login      the launch came from the OS autostart entry; never opens a browser\n",
    "--print-url  print the Settings handoff URL once bound, then serve headless;\n",
    "             with a gateway already running, print its URL instead\n",
    "--browser  open the Settings page in the default browser once bound;\n",
    "                 the installer's first run uses this",
    // --supervised is internal only, so not included in the user-facing usage message
);

/// Why argument parsing stopped.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ParseError {
    /// `-h`/`--help` was requested.
    Help,
    /// `--version` was requested.
    Version,
    /// The arguments were invalid; the string is the operator-facing reason.
    Usage(String),
}

/// What this launch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LaunchCommand {
    /// Serve (the default and only serving mode).
    Serve,
    /// Prints the diagnostics report and exits.
    Diagnostics,
}

/// The parsed invocation: the serve options plus how the main thread runs.
#[derive(Debug)]
pub(crate) struct Invocation {
    /// What the launch does.
    pub command: LaunchCommand,
    /// What to serve. Under [`LaunchCommand::Diagnostics`] only the config path
    /// is meaningful: the report names it.
    pub serve: ServeOptions,
    /// Whether the system tray occupies the main thread (default).
    /// `--no-tray` keeps the headless Ctrl-C loop for servers and CI.
    pub tray: bool,
    /// Whether the launch came from the OS autostart entry (`--login`).
    pub login: bool,
    /// Whether to print the Settings handoff URL to stdout (`--print-url`).
    /// Implies the headless loop: the flag exists for tray-less
    /// environments.
    pub print_url: bool,
}

/// Parses the command line into a typed [`Invocation`].
///
/// The bare invocation serves; there are no subcommands. Uses `OsString`
/// operands so non-UTF-8 config paths survive. The config path
/// (`--config PATH`, falling back to `RUNVIL_GATEWAY_CONFIG`) stays
/// optional: with neither set, the gateway discovers or generates the
/// boot config itself. `--profile NAME` is validated into a
/// [`ProfileName`] at parse time.
pub(crate) fn parse_args(
    args: impl IntoIterator<Item = OsString>,
) -> Result<Invocation, ParseError> {
    let mut args = args.into_iter();
    let _binary = args.next();

    // `diagnostics` is the only subcommand and must come first.
    let mut args = args.peekable();
    if args.peek().and_then(|arg| arg.to_str()) == Some("diagnostics") {
        args.next();
        return parse_diagnostics_args(args);
    }

    let mut profile: Option<ProfileName> = None;
    let mut config_path: Option<PathBuf> = None;
    let mut tray = true;
    let mut login = false;
    let mut print_url = false;
    let mut browser = false;
    let mut supervised = false;

    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--config") => {
                let path = args
                    .next()
                    .ok_or_else(|| ParseError::Usage("--config requires a path".to_string()))?;
                if config_path.is_some() {
                    return Err(ParseError::Usage("--config accepts one path".to_string()));
                }
                config_path = Some(PathBuf::from(path));
            }
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
            Some("--browser") => browser = true,
            Some("--supervised") => supervised = true,
            Some("-h" | "--help") => return Err(ParseError::Help),
            Some("--version") => return Err(ParseError::Version),
            Some(other) if other.starts_with('-') => {
                return Err(ParseError::Usage(format!("unknown flag {other}")));
            }
            _ => {
                return Err(ParseError::Usage(format!(
                    "unexpected argument {}",
                    arg.to_string_lossy()
                )));
            }
        }
    }

    if supervised && print_url {
        return Err(ParseError::Usage(
            "--supervised cannot be combined with --print-url".to_string(),
        ));
    }

    let config_path = resolve_config_path(config_path, std::env::var_os("RUNVIL_GATEWAY_CONFIG"));

    Ok(Invocation {
        command: LaunchCommand::Serve,
        // `--login`'s contract is absolute - a login launch never opens a
        // browser - so it wins over `--browser`.
        serve: ServeOptions::new(config_path, profile)
            .with_browser(browser && !login)
            .with_supervised(supervised),
        tray,
        login,
        print_url,
    })
}

/// Parses what may follow `diagnostics`: at most `--config PATH`. Every
/// other flag belongs to a serving launch and is a usage error here.
fn parse_diagnostics_args(args: impl Iterator<Item = OsString>) -> Result<Invocation, ParseError> {
    let mut args = args;
    let mut config_path: Option<PathBuf> = None;
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--config") => {
                let path = args
                    .next()
                    .ok_or_else(|| ParseError::Usage("--config requires a path".to_string()))?;
                if config_path.is_some() {
                    return Err(ParseError::Usage("--config accepts one path".to_string()));
                }
                config_path = Some(PathBuf::from(path));
            }
            Some("-h" | "--help") => return Err(ParseError::Help),
            _ => {
                return Err(ParseError::Usage(format!(
                    "diagnostics accepts only --config PATH, got {}",
                    arg.to_string_lossy()
                )));
            }
        }
    }
    let config_path = resolve_config_path(config_path, std::env::var_os("RUNVIL_GATEWAY_CONFIG"));
    Ok(Invocation {
        command: LaunchCommand::Diagnostics,
        serve: ServeOptions::new(config_path, None),
        tray: false,
        login: false,
        print_url: false,
    })
}

/// Resolves the config path: the `--config` flag wins, then the
/// `RUNVIL_GATEWAY_CONFIG` environment variable - but only when it
/// names an existing file. A stale env var warns and falls through to boot
/// discovery: ambient state rots in ways a typed CLI path does not, and a
/// forgotten variable must not hard-fail a first-run boot. A `--config`
/// path is deliberate, so a missing file there stays an error
/// downstream.
///
/// Tests pass both sources explicitly and never touch the process
/// environment (edition 2024 makes `set_var` unsafe); the existence check
/// touches only the paths the test itself creates.
fn resolve_config_path(cli: Option<PathBuf>, env: Option<OsString>) -> Option<PathBuf> {
    cli.or_else(|| {
        let path = PathBuf::from(env?);
        if path.is_file() {
            return Some(path);
        }
        tracing::warn!(
            path = %path.display(),
            "RUNVIL_GATEWAY_CONFIG names no file; falling back to discovery"
        );
        None
    })
}

#[cfg(test)]
#[path = "cli-tests.rs"]
mod tests;
