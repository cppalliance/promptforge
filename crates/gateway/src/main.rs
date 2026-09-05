//! The `promptforge-gateway` binary:
//! `promptforge-gateway serve [config.toml] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]`.
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
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use gateway::{ProfileName, ServeOptions, run, run_printing_url, run_with_tray};
use tracing_subscriber::Layer as _;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The log filter when `RUST_LOG` is unset: the gateway crates at `info`
/// (a download or a switch must say what it is doing), the chatty HTTP
/// dependencies at `warn`. `RUST_LOG` overrides the whole string.
const DEFAULT_LOG_FILTER: &str = "info,whisper_cpp=warn,hyper=warn,h2=warn,reqwest=warn,tower=warn";

const USAGE: &str = concat!(
    "usage: promptforge-gateway serve [config.toml] [--profile NAME] [--no-tray] [--login] [--print-url] [--browser]\n",
    "       promptforge-gateway --version\n",
    "the config path may also be set with the PROMPTFORGE_GATEWAY_CONFIG environment variable\n",
    "with no config path, the gateway searches beside the executable, the current directory,\n",
    "and the profile's .promptforge directory, generating a default config on first run\n",
    "--no-tray    run headless (Ctrl-C driven); for servers and CI\n",
    "--login      the launch came from the OS autostart entry; never opens a browser\n",
    "--print-url  print the Settings handoff URL once bound, then serve headless;\n",
    "             with a gateway already running, print its URL instead\n",
    "--browser  open the Settings page in the default browser once bound;\n",
    "                 the installer's first run uses this",
);

#[expect(
    unsafe_code,
    reason = "the one-call DPI-awareness shim at process start; every other unsafe lives in the tray and registry modules"
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

    // Logging starts only for a serve launch: a `--version` or `--help`
    // call must not rotate the running gateway's log out from under it.
    init_logging();

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

/// Installs the global subscriber: the filtered stream on stdout, plus the
/// same stream in `<state dir>/logs/gateway.log`, where the state dir is the
/// `.promptforge` directory the run directory's resolver already knows
/// (it holds `gateway.toml`, `run/`, and `models/`). A log file that cannot
/// be opened warns on stdout and never stops the gateway.
fn init_logging() {
    let filter = || {
        tracing_subscriber::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(DEFAULT_LOG_FILTER))
    };
    let stdout = tracing_subscriber::fmt::layer().with_filter(filter());
    let log_file = shared_sidecar::default_run_dir()
        .and_then(|run_dir| run_dir.parent().map(Path::to_path_buf))
        .map(|state_dir| open_log_file(&state_dir));
    match log_file {
        Some(Ok((path, file))) => {
            let file_layer = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .with_filter(filter());
            tracing_subscriber::registry()
                .with(stdout)
                .with(file_layer)
                .init();
            tracing::info!("logging to {}", path.display());
        }
        Some(Err(error)) => {
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("could not open the log file: {error}; logging to stdout only");
        }
        None => {
            tracing_subscriber::registry().with(stdout).init();
            tracing::warn!("no user profile directory found; logging to stdout only");
        }
    }
}

/// Opens `<state_dir>/logs/gateway.log` fresh for this run, first rotating
/// an existing log to `gateway.log.1` and overwriting any older rotation,
/// so one previous run is kept and disk use stays bounded.
///
/// # Errors
/// Returns the I/O failure from creating the directory, rotating the
/// existing log, or opening the fresh one.
fn open_log_file(state_dir: &Path) -> std::io::Result<(PathBuf, std::fs::File)> {
    let logs = state_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let current = logs.join("gateway.log");
    let previous = logs.join("gateway.log.1");
    if current.is_file() {
        // A rename cannot overwrite an existing destination on Windows, so
        // the older rotation is removed first.
        if previous.is_file() {
            std::fs::remove_file(&previous)?;
        }
        std::fs::rename(&current, &previous)?;
    }
    let file = std::fs::File::create(&current)?;
    Ok((current, file))
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
    let mut browser = false;

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
            Some("--browser") => browser = true,
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
        // `--login`'s contract is absolute - a login launch never opens a
        // browser - so it wins over `--browser`.
        serve: ServeOptions::new(config_path, profile).with_browser(browser && !login),
        tray,
        login,
        print_url,
    })
}

/// Resolves the config path: the CLI positional wins, then the
/// `PROMPTFORGE_GATEWAY_CONFIG` environment variable - but only when it
/// names an existing file. A stale env var warns and falls through to boot
/// discovery: ambient state rots in ways a typed CLI path does not, and a
/// forgotten variable must not hard-fail a first-run boot. A CLI
/// positional is deliberate, so a missing file there stays an error
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

    #[test]
    fn the_log_rotation_keeps_one_previous_run() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(temp.path().join("logs")).expect("logs dir");
        std::fs::write(temp.path().join("logs/gateway.log"), "first run").expect("seed log");

        let (path, file) = open_log_file(temp.path()).expect("first rotation opens");
        drop(file);
        assert_eq!(path, temp.path().join("logs/gateway.log"));
        assert_eq!(
            std::fs::read_to_string(temp.path().join("logs/gateway.log.1")).expect("rotated log"),
            "first run",
            "the previous run's log rotates to .1"
        );
        assert_eq!(
            std::fs::read_to_string(&path).expect("fresh log"),
            "",
            "the new run starts on a fresh file"
        );

        std::fs::write(&path, "second run").expect("write second run");
        let (_path, file) = open_log_file(temp.path()).expect("second rotation opens");
        drop(file);
        assert_eq!(
            std::fs::read_to_string(temp.path().join("logs/gateway.log.1")).expect("rotated log"),
            "second run",
            "a second rotation overwrites the older .1"
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
        // The Run-key entry is `"<exe>" serve --login`; a login launch must
        // never fail on its own command line.
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
    fn browser_parses_and_rides_the_serve_options() {
        let invocation = parse_args(args(&["serve", "--browser"])).expect("parse");
        assert!(
            invocation.serve.browser,
            "the flag reaches the spawn hook through ServeOptions"
        );
        assert!(invocation.tray, "the flag is independent of the run loop");
    }

    #[test]
    fn browser_defaults_off() {
        let invocation = parse_args(args(&["serve"])).expect("parse");
        assert!(
            !invocation.serve.browser,
            "embedders and ordinary launches never open a browser"
        );
    }

    #[test]
    fn login_wins_over_browser() {
        let invocation = parse_args(args(&["serve", "--login", "--browser"])).expect("parse");
        assert!(
            !invocation.serve.browser,
            "a login launch never opens a browser"
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
