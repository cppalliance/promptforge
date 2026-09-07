//! The `promptforge-gateway` binary:
//! `promptforge-gateway [--config PATH] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]`.
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
use gateway_logging::{LogConfig, LogRuntime};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The log filter when `RUST_LOG` is unset: the gateway crates at `info`
/// (a download or a switch must say what it is doing), the chatty HTTP
/// dependencies at `warn`. `RUST_LOG` overrides the whole string.
const DEFAULT_LOG_FILTER: &str = "info,whisper_cpp=warn,hyper=warn,h2=warn,reqwest=warn,tower=warn";

const USAGE: &str = concat!(
    "usage: promptforge-gateway [--config PATH] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]\n",
    "       promptforge-gateway diagnostics [--config PATH]\n",
    "       promptforge-gateway --version\n",
    "the config path may also be set with the PROMPTFORGE_GATEWAY_CONFIG environment variable;\n",
    "--config wins over it\n",
    "with no config path, the gateway searches beside the executable, the current directory,\n",
    "and the profile's .promptforge directory, generating a default config on first run\n",
    "diagnostics  print a JSON report of the state dir, config, logs, and connection file;\n",
    "             never serves, rotates logs, or parses the config\n",
    "--no-tray    run headless (Ctrl-C driven); for servers and CI\n",
    "--login      the launch came from the OS autostart entry; never opens a browser\n",
    "--print-url  print the Settings handoff URL once bound, then serve headless;\n",
    "             with a gateway already running, print its URL instead\n",
    "--browser  open the Settings page in the default browser once bound;\n",
    "                 the installer's first run uses this",
);

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
    unsafe {
        // SAFETY: called once at process start, before any window exists.
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
    if invocation.command == Command::Diagnostics {
        print!(
            "{}",
            gateway::diagnostics_json(invocation.serve.config_path)
        );
        return ExitCode::SUCCESS;
    }

    // A second launch never boots a duplicate server: when a live gateway
    // owns the connection file, hand off to it and exit. This runs before
    // logging starts and before any bind attempt - a handoff must not
    // rotate the running gateway's log out from under it. On the desktop
    // it is also the `.desktop` launcher's relaunch behavior.
    if let Some(url) = gateway::running_gateway_settings_url(&invocation.serve) {
        if invocation.print_url {
            println!("{url}");
        } else if invocation.login {
            // A login-triggered start never opens a browser; the running
            // gateway leaves this launch nothing to do.
        } else if let Err(error) = open::that(&url) {
            eprintln!(
                "could not open the browser: {error}; the running gateway's Settings URL is {url}"
            );
        }
        return ExitCode::SUCCESS;
    }

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
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // A fatal error is logged once with its complete source chain;
            // raw stderr is only the fallback when the logger never
            // started.
            if logging.is_some() {
                log_error_chain(&error);
            } else {
                print_error_chain(&error);
            }
            ExitCode::FAILURE
        }
    };
    // The logger shuts down last, so the terminal outcome and every record
    // behind it drain to the disk before the process exits.
    if let Some(runtime) = logging
        && let Err(error) = runtime.shutdown()
    {
        eprintln!("could not shut down the log worker: {error}");
    }
    exit
}

/// Installs the global subscriber and starts the log pipeline: the filtered
/// stream on stdout, plus the same stream through the bounded queue into
/// `<state dir>/logs/gateway.log`, where the state dir is the
/// `.promptforge` directory the run directory's resolver already knows
/// (it holds `gateway.toml`, `run/`, and `models/`). A log file that cannot
/// be opened warns on stdout and never stops the gateway. The returned
/// runtime must be shut down last.
fn init_logging() -> Option<LogRuntime> {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
    };
    let stdout = tracing_subscriber::fmt::layer().with_filter(filter());
    let runtime = shared_sidecar::default_run_dir()
        .and_then(|run_dir| run_dir.parent().map(PathBuf::from))
        .map(|state_dir| LogRuntime::start(LogConfig::new(state_dir)));
    match runtime {
        Some(Ok(runtime)) => {
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(runtime.writer())
                .with_filter(filter());
            tracing_subscriber::registry()
                .with(stdout)
                .with(file_layer)
                .init();
            tracing::info!("logging to {}", runtime.path().display());
            Some(runtime)
        }
        Some(Err(error)) => {
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("could not start file logging: {error}; logging to stdout only");
            None
        }
        None => {
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("no user profile directory found; logging to stdout only");
            None
        }
    }
}

/// Log the error and its full `source()` chain through the subscriber, so
/// the fatal outcome lands in the drained queue.
fn log_error_chain(error: &dyn std::error::Error) {
    tracing::error!("error: {error}");
    let mut source = error.source();
    while let Some(cause) = source {
        tracing::error!("  caused by: {cause}");
        source = cause.source();
    }
}

/// Print the error and its full `source()` chain to stderr: the fallback
/// when the logger itself never started.
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

/// What this launch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// Serve (the default and only serving mode).
    Serve,
    /// Print the diagnostics report and exit.
    Diagnostics,
}

/// The parsed invocation: the serve options plus how the main thread runs.
#[derive(Debug)]
struct Invocation {
    /// What the launch does.
    command: Command,
    /// What to serve. Under [`Command::Diagnostics`] only the config path
    /// is meaningful: the report names it.
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

/// Parse the command line into a typed [`Invocation`].
///
/// The bare invocation serves; there are no subcommands. Uses `OsString`
/// operands so non-UTF-8 config paths survive. The config path
/// (`--config PATH`, falling back to `PROMPTFORGE_GATEWAY_CONFIG`) stays
/// optional: with neither set, the gateway discovers or generates the
/// boot config itself. `--profile NAME` is validated into a
/// [`ProfileName`] at parse time.
fn parse_args(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, ParseError> {
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

    let config_path =
        resolve_config_path(config_path, std::env::var_os("PROMPTFORGE_GATEWAY_CONFIG"));

    Ok(Invocation {
        command: Command::Serve,
        // `--login`'s contract is absolute - a login launch never opens a
        // browser - so it wins over `--browser`.
        serve: ServeOptions::new(config_path, profile).with_browser(browser && !login),
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
    let config_path =
        resolve_config_path(config_path, std::env::var_os("PROMPTFORGE_GATEWAY_CONFIG"));
    Ok(Invocation {
        command: Command::Diagnostics,
        serve: ServeOptions::new(config_path, None),
        tray: false,
        login: false,
        print_url: false,
    })
}

/// Resolves the config path: the `--config` flag wins, then the
/// `PROMPTFORGE_GATEWAY_CONFIG` environment variable - but only when it
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
            "PROMPTFORGE_GATEWAY_CONFIG names no file; falling back to discovery"
        );
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_manifest_constant_is_compiled_only_for_windows() {
        let source: String = include_str!("../build.rs")
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        assert!(
            source.contains("#[cfg(windows)]constMANIFEST:&str="),
            "the manifest constant must not exist on hosts that cannot embed it"
        );
    }

    #[test]
    fn the_dpi_unsafe_expectation_exists_only_for_windows() {
        let source: String = include_str!("main.rs")
            .chars()
            .filter(|character| !character.is_ascii_whitespace())
            .collect();
        let main = source
            .find("fnmain()->ExitCode")
            .expect("the binary entry point exists");
        assert!(
            source[..main].contains("#[cfg_attr(windows,expect(unsafe_code,reason="),
            "the unsafe expectation must exist only with the Windows DPI shim"
        );
    }

    #[test]
    fn the_default_filter_keeps_gateway_info_and_quiets_whisper_cpp() {
        let filter = tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER);
        assert_eq!(
            filter.max_level_hint(),
            Some(tracing::level_filters::LevelFilter::INFO),
            "gateway crates log at info and nothing enables debug"
        );
        assert!(
            DEFAULT_LOG_FILTER.contains("whisper_cpp=warn"),
            "the noisy STT dependency stays at warn: {DEFAULT_LOG_FILTER}"
        );
        assert!(
            !DEFAULT_LOG_FILTER.contains("debug") && !DEFAULT_LOG_FILTER.contains("trace"),
            "the default never enables debug or trace: {DEFAULT_LOG_FILTER}"
        );
    }

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
        let file = tempfile::NamedTempFile::new().expect("temp config");
        let path = resolve_config_path(None, Some(file.path().as_os_str().to_os_string()));
        assert_eq!(path, Some(file.path().to_path_buf()));
    }

    #[test]
    fn a_stale_env_path_falls_back_to_discovery() {
        let missing = PathBuf::from("definitely-not-here-env.toml");
        let path = resolve_config_path(None, Some(missing.into_os_string()));
        assert_eq!(
            path, None,
            "a stale env var warns and defers to boot discovery"
        );
    }

    #[test]
    fn neither_path_set_defers_to_boot_discovery() {
        let path = resolve_config_path(None, None);
        assert_eq!(path, None, "the gateway discovers or generates the config");
    }

    #[test]
    fn the_root_invocation_serves_with_discovery() {
        let invocation = parse_args(args(&[])).expect("the bare invocation parses");
        assert_eq!(
            invocation.serve.config_path, None,
            "no --config defers to boot discovery"
        );
        assert!(invocation.serve.profile.is_none());
        assert!(invocation.tray, "the tray is the default main loop");
        assert!(!invocation.login);
        assert!(!invocation.print_url);
        assert!(
            !invocation.serve.browser,
            "embedders and ordinary launches never open a browser"
        );
    }

    #[test]
    fn the_serve_verb_is_rejected() {
        let error = parse_args(args(&["serve"])).unwrap_err();
        assert!(
            matches!(error, ParseError::Usage(_)),
            "the removed subcommand is a usage error, never an alias: {error:?}"
        );
    }

    #[test]
    fn a_positional_config_path_is_rejected() {
        let error = parse_args(args(&["gateway.toml"])).unwrap_err();
        assert!(
            matches!(error, ParseError::Usage(_)),
            "the config path is --config PATH, never a positional: {error:?}"
        );
    }

    #[test]
    fn the_config_flag_sets_the_path() {
        let invocation = parse_args(args(&["--config", "gateway.toml"])).expect("parse");
        assert_eq!(
            invocation.serve.config_path,
            Some(PathBuf::from("gateway.toml"))
        );
    }

    #[test]
    fn the_config_flag_requires_a_value() {
        let error = parse_args(args(&["--config"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn the_config_flag_is_given_once() {
        let error = parse_args(args(&["--config", "a.toml", "--config", "b.toml"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn parses_path_and_profile() {
        let invocation =
            parse_args(args(&["--config", "gateway.toml", "--profile", "dev"])).expect("parse");
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
        let invocation = parse_args(args(&["--config", "gateway.toml"])).expect("parse");
        assert!(invocation.tray, "the tray is the default main loop");
        assert!(!invocation.login);
    }

    #[test]
    fn no_tray_selects_the_headless_loop() {
        let invocation = parse_args(args(&["--no-tray"])).expect("parse");
        assert!(!invocation.tray);
        assert!(!invocation.login);
    }

    #[test]
    fn the_autostart_command_line_parses() {
        // The Run-key entry is `"<exe>" --login`; a login launch must
        // never fail on its own command line.
        let invocation = parse_args(args(&["--login"])).expect("parse");
        assert!(invocation.login);
        assert!(invocation.tray, "a login launch still shows the tray");
    }

    #[test]
    fn print_url_parses_and_leaves_the_other_flags_alone() {
        let invocation = parse_args(args(&["--print-url"])).expect("parse");
        assert!(invocation.print_url);
        assert!(
            invocation.tray,
            "the flag is independent; the dispatch makes it headless"
        );
        assert!(!invocation.login);
    }

    #[test]
    fn print_url_combines_with_no_tray_and_a_config_path() {
        let invocation = parse_args(args(&[
            "--config",
            "gateway.toml",
            "--no-tray",
            "--print-url",
        ]))
        .expect("parse");
        assert!(invocation.print_url);
        assert!(!invocation.tray);
        assert_eq!(
            invocation.serve.config_path,
            Some(PathBuf::from("gateway.toml"))
        );
    }

    #[test]
    fn browser_parses_and_rides_the_serve_options() {
        let invocation = parse_args(args(&["--browser"])).expect("parse");
        assert!(
            invocation.serve.browser,
            "the flag reaches the spawn hook through ServeOptions"
        );
        assert!(invocation.tray, "the flag is independent of the run loop");
    }

    #[test]
    fn login_wins_over_browser() {
        let invocation = parse_args(args(&["--login", "--browser"])).expect("parse");
        assert!(
            !invocation.serve.browser,
            "a login launch never opens a browser"
        );
    }

    #[test]
    fn missing_profile_defers_to_environment_or_state() {
        let invocation = parse_args(args(&["--config", "gateway.toml"])).expect("parse");
        assert!(invocation.serve.profile.is_none());
    }

    #[test]
    fn invalid_profile_name_is_a_usage_error() {
        let error = parse_args(args(&["--config", "gateway.toml", "--profile", ""])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_traversal_profile_name() {
        let error = parse_args(args(&[
            "--config",
            "gateway.toml",
            "--profile",
            "../escape",
        ]))
        .unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn rejects_an_unknown_argument() {
        let error = parse_args(args(&["frobnicate"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn help_is_recognized() {
        let error = parse_args(args(&["--help"])).unwrap_err();
        assert_eq!(error, ParseError::Help);
        let error = parse_args(args(&["-h"])).unwrap_err();
        assert_eq!(error, ParseError::Help);
    }

    #[test]
    fn version_is_recognized() {
        let error = parse_args(args(&["--version"])).unwrap_err();
        assert_eq!(error, ParseError::Version);
    }

    #[test]
    fn rejects_unknown_flag() {
        let error = parse_args(args(&["--profiles-dir", "x", "--profile", "dev"])).unwrap_err();
        assert!(matches!(error, ParseError::Usage(_)));
    }

    #[test]
    fn diagnostics_is_the_only_subcommand() {
        let invocation = parse_args(args(&["diagnostics"])).expect("parses");
        assert_eq!(invocation.command, Command::Diagnostics);
        assert_eq!(invocation.serve.config_path, None);
        let invocation = parse_args(args(&[])).expect("the bare invocation parses");
        assert_eq!(invocation.command, Command::Serve);
    }

    #[test]
    fn diagnostics_accepts_a_config_path() {
        let invocation =
            parse_args(args(&["diagnostics", "--config", "gateway.toml"])).expect("parses");
        assert_eq!(invocation.command, Command::Diagnostics);
        assert_eq!(
            invocation.serve.config_path,
            Some(PathBuf::from("gateway.toml"))
        );
    }

    #[test]
    fn diagnostics_rejects_serving_flags() {
        for rest in ["--no-tray", "--login", "--print-url", "--browser"] {
            let error = parse_args(args(&["diagnostics", rest])).unwrap_err();
            assert!(
                matches!(error, ParseError::Usage(_)),
                "diagnostics rejects {rest}: {error:?}"
            );
        }
    }

    #[test]
    fn diagnostics_after_a_flag_is_not_a_subcommand() {
        let error = parse_args(args(&["--no-tray", "diagnostics"])).unwrap_err();
        assert!(
            matches!(error, ParseError::Usage(_)),
            "the subcommand must come first: {error:?}"
        );
    }

    /// A fatal returned error is logged once with its complete source
    /// chain, and the queue drains to disk before the process exits.
    #[test]
    fn a_fatal_error_is_logged_once_with_its_full_chain_then_drained() {
        #[derive(Debug)]
        struct Chain(&'static str, Option<Box<Chain>>);

        impl std::fmt::Display for Chain {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(self.0)
            }
        }

        impl std::error::Error for Chain {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                self.1
                    .as_deref()
                    .map(|cause| cause as &dyn std::error::Error)
            }
        }

        let temp = tempfile::tempdir().expect("tempdir");
        let runtime = LogRuntime::start(LogConfig::new(temp.path().join("state")))
            .expect("start the log pipeline");
        let log_path = runtime.path().to_path_buf();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(runtime.writer())
            .finish();
        let error = Chain(
            "serve the gateway",
            Some(Box::new(Chain(
                "bind 127.0.0.1:8081",
                Some(Box::new(Chain("address already in use", None))),
            ))),
        );
        tracing::subscriber::with_default(subscriber, || log_error_chain(&error));
        // The logger shuts down last, which is what drains the chain.
        runtime.shutdown().expect("the queue drains before exit");

        let log = std::fs::read_to_string(&log_path).expect("read the log");
        for link in [
            "error: serve the gateway",
            "caused by: bind 127.0.0.1:8081",
            "caused by: address already in use",
        ] {
            assert_eq!(
                log.matches(link).count(),
                1,
                "each chain link lands exactly once: {link}\n{log}"
            );
        }
    }
}
