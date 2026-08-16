//! Unit tests for `prompts.toml` parsing.

use super::interpolate::interpolate_with;
use super::*;
use crate::error::ConfigErrorKind;

/// A configuration exercising every field the plan names.
const FULL: &str = r#"
[server]
bind = "127.0.0.1:9310"
api_key = "shared-bearer"
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
api_key = "gateway-bearer"

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
api_key = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "gateway-bearer"
"#;

#[test]
fn parses_a_full_config() {
    let config = Config::from_toml_str(FULL).expect("full config parses");

    assert_eq!(config.server.bind.port(), 9310);
    assert_eq!(
        config.server.api_key.as_ref().map(Secret::expose),
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

    assert_eq!(config.gateway.url.as_str(), "http://127.0.0.1:8081/v1");
    assert_eq!(config.gateway.api_key.expose(), "gateway-bearer");

    let include: Vec<&str> = config
        .catalog
        .include
        .iter()
        .map(GlobPattern::as_str)
        .collect();
    let exclude: Vec<&str> = config
        .catalog
        .exclude
        .iter()
        .map(GlobPattern::as_str)
        .collect();
    assert_eq!(include, ["*.md", "governance/**/*.md"]);
    assert_eq!(exclude, ["_*.md", "drafts/**"]);

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
    assert_eq!(
        staker.file.as_ref().map(RelativePromptPath::as_path),
        Some(Path::new("experiments/staker-v3.md"))
    );
    assert!(staker.enabled);
}

#[test]
fn a_config_with_no_server_api_key_loads() {
    // The API key is a property of the HTTP surface: `serve` refuses to bind
    // without one, and `serve --stdio` never reads it. Requiring it here is
    // what stopped a local stdio install over a credential its transport does
    // not use.
    let toml = r#"
[server]
bind = "127.0.0.1:9310"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "t"
"#;
    let config = Config::from_toml_str(toml).expect("a config with no api_key loads");
    assert!(config.server.api_key.is_none());
    assert_eq!(config.gateway.api_key.expose(), "t");
}

#[test]
fn an_unset_variable_in_the_server_api_key_leaves_it_absent() {
    // Interpolation happens after the parse, so an unset variable is
    // attributed to the field that carried it. This is the one field that
    // survives one, because stdio boots without an API key at all.
    let toml = MINIMAL.replace(
        "api_key = \"shared-bearer\"",
        "api_key = \"${NOT_SET_ANYWHERE}\"",
    );
    let config = Config::from_toml_str(&toml).expect("an unset server api_key is not a load failure");
    assert!(config.server.api_key.is_none());
}

#[test]
fn an_unset_variable_outside_the_server_api_key_is_still_an_error() {
    // The gateway API key is required on both transports, so an unset variable
    // there fails the load rather than starting with a blank credential.
    let toml = MINIMAL.replace("api_key = \"gateway-bearer\"", "api_key = \"${NOT_SET_ANYWHERE}\"");
    let err = Config::from_toml_str(&toml).expect_err("an unset gateway api_key is refused");
    assert_eq!(err.kind(), ConfigErrorKind::UnresolvedVar, "{err}");
    assert!(err.to_string().contains("NOT_SET_ANYWHERE"), "{err}");
}

#[test]
fn an_empty_or_whitespace_server_api_key_is_an_error() {
    // An empty shared bearer would make a request presenting no credential
    // compare equal, so an API key that carries nothing is refused where it is
    // read. Whitespace alone is the same mistake with a space in it.
    // The third spelling reaches TOML as an escape, so the parsed key is a
    // real tab and a real newline rather than four characters of prose.
    for api_key in ["", " ", r"\t\n"] {
        let toml = format!(
            "[server]\napi_key = \"{api_key}\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"t\"\n"
        );
        let err = Config::from_toml_str(&toml).expect_err("an api_key carrying nothing is refused");
        assert_eq!(err.kind(), ConfigErrorKind::EmptyToken, "{err}");
        let text = err.to_string();
        assert!(text.contains("api_key"), "{text}");
        assert!(text.contains("must not be empty"), "{text}");
    }
}

#[test]
fn an_unknown_key_is_an_error() {
    let toml = format!("{MINIMAL}\n[catalog]\ninculde = [\"*.md\"]\n");
    let err = Config::from_toml_str(&toml).expect_err("a misspelled key is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains("inculde"), "{err}");
}

#[test]
fn a_catalog_default_expose_is_rejected_by_name() {
    // Every prompt is reached through `run_prompt`, so there is nothing left to
    // promote. Ignoring the key would leave an operator believing a prompt was
    // published under its own name, so the load fails and names the key.
    let toml = format!("{MINIMAL}\n[catalog]\ndefault_expose = \"tool\"\n");
    let err = Config::from_toml_str(&toml).expect_err("default_expose is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains("default_expose"), "{err}");
}

#[test]
fn a_per_prompt_expose_is_rejected_by_name() {
    let toml = format!("{MINIMAL}\n[prompts.staker]\nexpose = \"tool\"\n");
    let err = Config::from_toml_str(&toml).expect_err("a per-prompt expose is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
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
    assert_eq!(err.kind(), ConfigErrorKind::UnresolvedVar, "{err}");
    assert!(err.to_string().contains("NOPE"), "{err}");
}

#[test]
fn an_unclosed_interpolation_is_an_error() {
    let lookup = |_: &str| Some(String::new());
    let err = interpolate_with("${OPEN", &lookup).expect_err("an unclosed ${ is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Interpolation, "{err}");
}

#[test]
fn loads_from_a_file() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("prompts.toml");
    std::fs::write(&path, MINIMAL).expect("write config");

    let config = Config::load(&path).expect("config loads");
    assert_eq!(config.gateway.url.as_str(), "http://127.0.0.1:8081/v1");
}

#[test]
fn an_unreadable_file_names_its_path_and_a_not_found_source() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("absent.toml");

    let err = Config::load(&path).expect_err("a missing file is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Read, "{err}");
    // The path survives losslessly rather than through a rendered string.
    assert_eq!(err.path(), Some(path.as_path()), "{err}");
    // The io cause survives, so a caller can tell a missing file from a
    // permission failure without parsing the message.
    let source = std::error::Error::source(&err).expect("a read error carries an io source");
    let io = source
        .downcast_ref::<std::io::Error>()
        .expect("the source is an io::Error");
    assert_eq!(io.kind(), std::io::ErrorKind::NotFound, "{io}");
}

#[test]
fn from_str_agrees_with_from_toml_str() {
    let parsed: Config = MINIMAL.parse().expect("FromStr parses the minimal config");
    let direct = Config::from_toml_str(MINIMAL).expect("from_toml_str parses it too");
    assert_eq!(parsed.gateway.url.as_str(), direct.gateway.url.as_str());
    assert_eq!(parsed.gateway.api_key.expose(), direct.gateway.api_key.expose());
}

#[test]
fn a_blank_gateway_api_key_is_refused() {
    let source = r#"
[server]
api_key = "shared-bearer"

[gateway]
url = "http://127.0.0.1:8081/v1"
api_key = "   "
"#;
    let err = Config::from_toml_str(source).expect_err("a blank gateway api_key is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains("[gateway].api_key"), "{err}");
}

#[test]
fn an_empty_gateway_url_is_refused() {
    let source = r#"
[server]
api_key = "shared-bearer"

[gateway]
url = ""
api_key = "gateway-bearer"
"#;
    let err = Config::from_toml_str(source).expect_err("an empty gateway url is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains("[gateway].url"), "{err}");
}

#[test]
fn a_schemeless_gateway_url_is_refused() {
    // The URL is validated at the parse boundary, so a value with no http(s)
    // scheme is refused there rather than reaching the gateway client as an
    // endpoint it cannot use.
    let source = r#"
[server]
api_key = "shared-bearer"

[gateway]
url = "127.0.0.1:8081/v1"
api_key = "gateway-bearer"
"#;
    let err = Config::from_toml_str(source).expect_err("a schemeless gateway url is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains("http"), "{err}");
}

#[test]
fn a_prompt_block_keyed_on_an_illegal_name_is_refused() {
    // A `[prompts.NAME]` key is a PromptName, held to the same shape a
    // published tool name must have, so a key no prompt could declare is
    // refused at the parse boundary.
    let toml = format!("{MINIMAL}\n[prompts.Research-Person]\nenabled = false\n");
    let err = Config::from_toml_str(&toml).expect_err("an illegal block key is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
}

#[test]
fn a_relative_block_file_is_accepted_and_an_escaping_one_is_not() {
    let ok = format!("{MINIMAL}\n[prompts.staker]\nfile = \"experiments/staker.md\"\n");
    Config::from_toml_str(&ok).expect("a relative block file parses");

    let escaping = format!("{MINIMAL}\n[prompts.staker]\nfile = \"../staker.md\"\n");
    let err = Config::from_toml_str(&escaping).expect_err("an escaping block file is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
    assert!(err.to_string().contains(".."), "{err}");
}

#[test]
fn a_malformed_catalog_pattern_is_refused() {
    // The pattern is a GlobPattern, validated to compile at the parse boundary,
    // so a syntactically broken glob is refused there rather than mid-resolve.
    let toml = format!("{MINIMAL}\n[catalog]\ninclude = [\"a[b\"]\n");
    let err = Config::from_toml_str(&toml).expect_err("a malformed glob is refused");
    assert_eq!(err.kind(), ConfigErrorKind::Parse, "{err}");
}

#[test]
fn interpolation_reaches_array_and_nested_table_values() {
    // cargo sets CARGO_PKG_NAME in the test process's environment, so this
    // exercises the real interpolation path through `Config` for an array value
    // (`[server].allowed_hosts`) and a nested-table value
    // (`[prompts.NAME].file`), not just a single scalar.
    let pkg = std::env::var("CARGO_PKG_NAME").expect("cargo sets CARGO_PKG_NAME");
    let toml = "\
[server]
api_key = \"shared-bearer\"
allowed_hosts = [\"${CARGO_PKG_NAME}\", \"static-host\"]

[gateway]
url = \"http://127.0.0.1:8081/v1\"
api_key = \"gateway-bearer\"

[prompts.example]
file = \"${CARGO_PKG_NAME}.md\"
";
    let config = Config::from_toml_str(toml).expect("config parses");

    assert_eq!(
        config.server.allowed_hosts,
        vec![pkg.clone(), "static-host".to_string()]
    );
    assert_eq!(
        config.prompts["example"]
            .file
            .as_ref()
            .map(RelativePromptPath::as_path),
        Some(Path::new(&format!("{pkg}.md")))
    );
}

#[test]
fn malformed_values_are_refused_at_parse() {
    let cases = [
        (
            "zero concurrency",
            "[server]\napi_key = \"t\"\nmax_concurrent_runs = 0\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"k\"\n",
        ),
        (
            "invalid bind address",
            "[server]\napi_key = \"t\"\nbind = \"not-an-address\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"k\"\n",
        ),
        (
            "invalid duration",
            "[server]\napi_key = \"t\"\nadmission_timeout = \"not-a-duration\"\n\n\
             [gateway]\nurl = \"http://127.0.0.1:8081/v1\"\napi_key = \"k\"\n",
        ),
        ("a missing gateway section", "[server]\napi_key = \"t\"\n"),
        (
            "a missing gateway url",
            "[server]\napi_key = \"t\"\n\n[gateway]\napi_key = \"k\"\n",
        ),
    ];
    for (label, toml) in cases {
        let err = Config::from_toml_str(toml).expect_err(label);
        assert_eq!(err.kind(), ConfigErrorKind::Parse, "{label}: {err}");
    }
}

#[test]
fn secret_redacts() {
    let secret = Secret::try_from("hunter2".to_string()).expect("a non-blank secret is accepted");

    assert_eq!(format!("{secret}"), "redacted");
    assert_eq!(format!("{secret:?}"), "Secret(redacted)");
    assert_eq!(secret.expose(), "hunter2");
    assert!(!secret.is_empty());
    assert!(!secret.is_blank());
}

#[test]
fn a_blank_secret_is_refused_at_the_type_boundary() {
    // The blank rejection lives in `Secret` itself, so no `Config` field can
    // ever hold a token or key that carries nothing usable. Empty, spaces, and
    // tabs-and-newlines are the same mistake with different whitespace.
    for blank in ["", "   ", "\t\n"] {
        Secret::try_from(blank).expect_err("a blank secret is refused");
        Secret::try_from(blank.to_string()).expect_err("a blank owned secret is refused");
    }
    assert_eq!(
        Secret::try_from("k")
            .expect("a non-blank secret is accepted")
            .expose(),
        "k"
    );
}
