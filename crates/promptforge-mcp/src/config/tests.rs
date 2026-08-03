//! Unit tests for `prompts.toml` parsing.

use super::*;

/// A configuration exercising every field the plan names.
const FULL: &str = r#"
[server]
bind = "127.0.0.1:9310"
token = "shared-bearer"
max_concurrent_runs = 4
admission_timeout = "30s"
reply_deadline = "240s"
retain_completed = "1h"
watch = true
watch_debounce = "500ms"

[paths]
prompts = 'C:\ProgramData\promptforge\prompts'

[gateway]
url = "http://127.0.0.1:8081/v1"
token = "gateway-bearer"
model = "claude-sonnet-4-6"

[catalog]
include = ["*.md", "governance/**/*.md"]
exclude = ["_*.md", "drafts/**"]

[prompts.scratch_test]
enabled = false

[prompts.staker]
file = "experiments/staker-v3.md"
"#;

/// The smallest configuration that parses: the two required sections.
const MINIMAL: &str = r#"
[server]
token = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
token = "gateway-bearer"
"#;

#[test]
fn parses_a_full_config() {
    let config = Config::from_toml_str(FULL).expect("full config parses");

    assert_eq!(config.server.bind.port(), 9310);
    assert_eq!(
        config.server.token.as_ref().map(Secret::expose),
        Some("shared-bearer")
    );
    assert_eq!(config.server.max_concurrent_runs.get(), 4);
    assert_eq!(config.server.admission_timeout, Duration::from_secs(30));
    assert_eq!(config.server.reply_deadline, Duration::from_secs(240));
    assert_eq!(config.server.retain_completed, Duration::from_secs(3600));
    assert!(config.server.watch);
    assert_eq!(config.server.watch_debounce, Duration::from_millis(500));

    assert_eq!(
        config.paths.prompts,
        PathBuf::from(r"C:\ProgramData\promptforge\prompts")
    );

    assert_eq!(config.gateway.url, "http://127.0.0.1:8081/v1");
    assert_eq!(config.gateway.token.expose(), "gateway-bearer");
    assert_eq!(config.gateway.model.as_deref(), Some("claude-sonnet-4-6"));

    assert_eq!(config.catalog.include, ["*.md", "governance/**/*.md"]);
    assert_eq!(config.catalog.exclude, ["_*.md", "drafts/**"]);

    assert_eq!(config.prompts.len(), 2);
}

#[test]
fn defaults_fill_in_every_optional_setting() {
    let config = Config::from_toml_str(MINIMAL).expect("minimal config parses");

    assert_eq!(config.server.bind, SocketAddr::from(([127, 0, 0, 1], 9310)));
    assert_eq!(config.server.max_concurrent_runs.get(), 4);
    assert_eq!(config.server.admission_timeout, Duration::from_secs(30));
    assert_eq!(config.server.reply_deadline, Duration::from_secs(240));
    assert_eq!(config.server.retain_completed, Duration::from_secs(3600));
    assert!(config.server.watch);
    assert_eq!(config.server.watch_debounce, Duration::from_millis(500));
    assert_eq!(config.paths.prompts, PathBuf::from("prompts"));
    assert!(config.gateway.model.is_none());
}

#[test]
fn parses_without_a_catalog_section() {
    let config = Config::from_toml_str(MINIMAL).expect("a config with no [catalog] parses");

    assert!(config.catalog.include.is_empty());
    assert!(config.catalog.exclude.is_empty());
    assert!(config.prompts.is_empty());
}

#[test]
fn a_named_block_drops_a_globbed_prompt() {
    let toml = format!(
        "{MINIMAL}\n[catalog]\ninclude = [\"*.md\"]\n\n[prompts.dropped]\nenabled = false\n"
    );
    let config = Config::from_toml_str(&toml).expect("config parses");

    let dropped = &config.prompts["dropped"];
    assert!(!dropped.enabled);
    assert!(dropped.file.is_none());
}

#[test]
fn a_named_block_can_carry_a_file() {
    let config = Config::from_toml_str(FULL).expect("full config parses");

    let staker = &config.prompts["staker"];
    assert_eq!(staker.file, Some(PathBuf::from("experiments/staker-v3.md")));
    assert!(staker.enabled);
}

#[test]
fn a_config_with_no_server_token_loads() {
    // The token is a property of the HTTP surface: `serve` refuses to bind
    // without one, and `serve --stdio` never reads it. Requiring it here is
    // what stopped a local stdio install over a credential its transport does
    // not use.
    let toml = r#"
[server]
bind = "127.0.0.1:9310"

[gateway]
url = "http://127.0.0.1:8081/v1"
token = "t"
"#;
    let config = Config::from_toml_str(toml).expect("a config with no token loads");
    assert!(config.server.token.is_none());
    assert_eq!(config.gateway.token.expose(), "t");
}

#[test]
fn an_unset_variable_in_the_server_token_leaves_it_absent() {
    // Interpolation happens after the parse, so an unset variable is
    // attributed to the field that carried it. This is the one field that
    // survives one, because stdio boots without a token at all.
    let toml = MINIMAL.replace(
        "token = \"shared-bearer\"",
        "token = \"${NOT_SET_ANYWHERE}\"",
    );
    let config = Config::from_toml_str(&toml).expect("an unset server token is not a load failure");
    assert!(config.server.token.is_none());
}

#[test]
fn an_unset_variable_outside_the_server_token_is_still_an_error() {
    // The gateway token is required on both transports, so an unset variable
    // there fails the load rather than starting with a blank credential.
    let toml = MINIMAL.replace(
        "token = \"gateway-bearer\"",
        "token = \"${NOT_SET_ANYWHERE}\"",
    );
    let err = Config::from_toml_str(&toml).expect_err("an unset gateway token is refused");
    assert!(
        matches!(err, ConfigError::UnresolvedVar(ref name) if name == "NOT_SET_ANYWHERE"),
        "{err}"
    );
}

#[test]
fn an_empty_or_whitespace_server_token_is_an_error() {
    // An empty shared bearer would make a request presenting no credential
    // compare equal, so a token that carries nothing is refused where it is
    // read. Whitespace alone is the same mistake with a space in it.
    // The third spelling reaches TOML as an escape, so the parsed token is a
    // real tab and a real newline rather than four characters of prose.
    for token in ["", " ", r"\t\n"] {
        let toml = format!(
            "[server]\ntoken = \"{token}\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\ntoken = \"t\"\n"
        );
        let err = Config::from_toml_str(&toml).expect_err("a token carrying nothing is refused");
        assert!(matches!(err, ConfigError::EmptyToken), "{err}");
        let text = err.to_string();
        assert!(text.contains("token"), "{text}");
        assert!(text.contains("must not be empty"), "{text}");
    }
}

#[test]
fn an_unknown_key_is_an_error() {
    let toml = format!("{MINIMAL}\n[catalog]\ninculde = [\"*.md\"]\n");
    let err = Config::from_toml_str(&toml).expect_err("a misspelled key is refused");
    assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    assert!(err.to_string().contains("inculde"), "{err}");
}

#[test]
fn a_catalog_default_expose_is_rejected_by_name() {
    // Every prompt is reached through `run_prompt`, so there is nothing left to
    // promote. Ignoring the key would leave an operator believing a prompt was
    // published under its own name, so the load fails and names the key.
    let toml = format!("{MINIMAL}\n[catalog]\ndefault_expose = \"tool\"\n");
    let err = Config::from_toml_str(&toml).expect_err("default_expose is refused");
    assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    assert!(err.to_string().contains("default_expose"), "{err}");
}

#[test]
fn a_per_prompt_expose_is_rejected_by_name() {
    let toml = format!("{MINIMAL}\n[prompts.staker]\nexpose = \"tool\"\n");
    let err = Config::from_toml_str(&toml).expect_err("a per-prompt expose is refused");
    assert!(matches!(err, ConfigError::Parse(_)), "{err}");
    assert!(err.to_string().contains("expose"), "{err}");
}

#[test]
fn interpolates_from_the_environment() {
    // `CARGO_MANIFEST_DIR` is set by cargo for the test binary, so this
    // exercises the real environment path rather than an injected lookup. It
    // goes in a TOML literal string, since a Windows path carries backslashes.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let toml = format!("{MINIMAL}\n[paths]\nprompts = '${{CARGO_MANIFEST_DIR}}'\n");
    let config = Config::from_toml_str(&toml).expect("config parses");

    assert_eq!(config.paths.prompts, PathBuf::from(manifest_dir));
}

#[test]
fn interpolation_substitutes_escapes_and_passes_a_lone_dollar_through() {
    let lookup = |name: &str| (name == "TOKEN").then(|| "s3cret".to_string());

    assert_eq!(
        interpolate_with("token = \"${TOKEN}\"", &lookup).expect("resolves"),
        "token = \"s3cret\""
    );
    assert_eq!(interpolate_with("a$$b", &lookup).expect("escapes"), "a$b");
    assert_eq!(interpolate_with("100$", &lookup).expect("passes"), "100$");
    assert_eq!(
        interpolate_with("no vars here", &lookup).expect("passes"),
        "no vars here"
    );
}

#[test]
fn an_unset_variable_is_an_error() {
    let lookup = |_: &str| None;
    let err = interpolate_with("${NOPE}", &lookup).expect_err("an unset variable is refused");
    assert!(
        matches!(err, ConfigError::UnresolvedVar(ref name) if name == "NOPE"),
        "{err}"
    );
}

#[test]
fn an_unclosed_interpolation_is_an_error() {
    let lookup = |_: &str| Some(String::new());
    let err = interpolate_with("${OPEN", &lookup).expect_err("an unclosed ${ is refused");
    assert!(matches!(err, ConfigError::Interpolation(_)), "{err}");
}

#[test]
fn loads_from_a_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompts.toml");
    std::fs::write(&path, MINIMAL).expect("write config");

    let config = Config::load(&path).expect("config loads");
    assert_eq!(config.gateway.url, "http://127.0.0.1:8081/v1");
}

#[test]
fn an_unreadable_file_is_an_error() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("absent.toml");

    let err = Config::load(&path).expect_err("a missing file is refused");
    assert!(matches!(err, ConfigError::Read { .. }), "{err}");
}

#[test]
fn secret_redacts() {
    let secret = Secret::from("hunter2".to_string());

    assert_eq!(format!("{secret}"), "redacted");
    assert_eq!(format!("{secret:?}"), "Secret(redacted)");
    assert_eq!(secret.expose(), "hunter2");
    assert!(!secret.is_empty());
    assert!(!secret.is_blank());
    assert!(Secret::from(String::new()).is_empty());
    assert!(Secret::from("  \t".to_string()).is_blank());
}
