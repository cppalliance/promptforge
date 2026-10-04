//! Tests for the binary's log filter, command-line parsing, and fatal-error logging.

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
fn browser_parses_and_reaches_the_serve_options() {
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
