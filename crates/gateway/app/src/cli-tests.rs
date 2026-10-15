//! Tests for the binary's command-line parsing.

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
    assert_eq!(invocation.command, LaunchCommand::Diagnostics);
    assert_eq!(invocation.serve.config_path, None);
    let invocation = parse_args(args(&[])).expect("the bare invocation parses");
    assert_eq!(invocation.command, LaunchCommand::Serve);
}

#[test]
fn diagnostics_accepts_a_config_path() {
    let invocation =
        parse_args(args(&["diagnostics", "--config", "gateway.toml"])).expect("parses");
    assert_eq!(invocation.command, LaunchCommand::Diagnostics);
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

#[test]
fn supervised_option_parses_and_reaches_the_serve_options() {
    let invocation = parse_args(args(&["--supervised", "--no-tray"])).expect("should parse");

    assert!(invocation.serve.supervised);
    assert!(
        !invocation.tray,
        "--supervised leaves the tray choice to --no-tray"
    );
}

#[test]
fn supervised_option_does_not_override_no_tray_option() {
    let invocation = parse_args(args(&["--supervised"])).expect("should parse");

    assert!(invocation.tray, "--supervised should not set tray to false");

    let invocation = parse_args(args(&["--supervised", "--no-tray"])).expect("should parse");

    assert!(
        !invocation.tray,
        "--supervised should not override --no-tray"
    );
}

#[test]
fn a_launch_is_not_supervised_by_default() {
    let invocation = parse_args(args(&[])).expect("should parse");

    assert!(!invocation.serve.supervised);
}

#[test]
fn supervised_and_print_url_options_cannot_be_used_together() {
    let print_url = "--print-url";
    let error = parse_args(args(&["--supervised", print_url])).unwrap_err();

    assert!(
        matches!(error, ParseError::Usage(_)),
        "--supervised {print_url} cannot both be set: {error:?}"
    );
}
