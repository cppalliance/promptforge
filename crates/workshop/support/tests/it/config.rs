//! `workshop.toml` loading tests: parsing, interpolation, and defaults,
//! driven through the public API.

use std::path::{Path, PathBuf};

use workshop_support::{Config, ConfigError};

#[test]
fn parses_fixture_and_interpolates_from_environment() {
    let path_value = std::env::var("PATH").expect("PATH is set on every supported platform");
    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "${PATH}"
"#;
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.gateway.base_url, "http://127.0.0.1:8081");
    assert_eq!(config.gateway.api_key, path_value);
}

#[test]
fn defaults_fill_server() {
    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"
"#;
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.server.bind, "127.0.0.1:7910");
}

#[test]
fn removed_voice_section_is_rejected() {
    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[voice]
interim_model = "old.bin"
"#;
    assert!(
        matches!(Config::from_toml_str(raw), Err(ConfigError::Parse { .. })),
        "workshop voice ownership was removed instead of silently ignored"
    );
}

#[test]
fn explicit_sections_override_defaults() {
    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[server]
bind = "127.0.0.1:9000"
"#;
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.server.bind, "127.0.0.1:9000");
}

#[test]
fn path_defaults_anchor_beside_the_config_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("workshop.toml");
    std::fs::write(
        &path,
        "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
    )
    .expect("write fixture");
    let config = Config::load(&path).expect("fixture loads");
    assert_eq!(
        config.server.state_dir,
        dir.path(),
        "an absent state_dir is the config file's directory"
    );
    assert_eq!(
        config.agents.path,
        dir.path().join("agents"),
        "an absent agents path is agents/ beside the config file"
    );

    // Without a file, the anchor degrades to the working directory.
    let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"k\"\n";
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.server.state_dir, PathBuf::from("."));
    assert_eq!(config.agents.path, Path::new(".").join("agents"));
}

#[test]
fn explicit_state_dir_and_agents_path_are_kept_verbatim() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("workshop.toml");
    std::fs::write(
        &path,
        "[gateway]\nbase_url = \"http://x\"\napi_key = \"k\"\n\n\
         [server]\nstate_dir = \"state\"\n\n[agents]\npath = \"my-agents\"\n",
    )
    .expect("write fixture");
    let config = Config::load(&path).expect("fixture loads");
    assert_eq!(
        config.server.state_dir,
        PathBuf::from("state"),
        "an explicit state_dir is not re-anchored"
    );
    assert_eq!(
        config.agents.path,
        PathBuf::from("my-agents"),
        "an explicit agents path is not re-anchored"
    );
}

#[test]
fn open_browser_defaults_to_false_and_parses_when_set() {
    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"
"#;
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert!(!config.server.open_browser, "default is off");

    let raw = r#"
[gateway]
base_url = "http://127.0.0.1:8081"
api_key = "k"

[server]
open_browser = true
"#;
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert!(config.server.open_browser);
    assert_eq!(config.server.bind, "127.0.0.1:7910", "bind still defaults");
}

#[test]
fn double_dollar_is_literal() {
    let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"cost $$5\"\n";
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.gateway.api_key, "cost $5");
}

#[test]
fn unset_variable_interpolates_to_empty() {
    let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"${PFG_WB_DEFINITELY_UNSET_XYZ}\"\n";
    let config = Config::from_toml_str(raw).expect("unset variable resolves to empty");
    assert_eq!(config.gateway.api_key, "");
}

#[test]
fn an_empty_base_url_is_kept_as_the_not_explicit_signal() {
    let raw = "[gateway]\nbase_url = \"${PFG_WB_DEFINITELY_UNSET_XYZ}\"\napi_key = \"k\"\n";
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(
        config.gateway.base_url, "",
        "no default is filled: endpoint resolution reads empty as not explicit"
    );
}

#[test]
fn explicit_base_url_is_kept() {
    let raw = "[gateway]\nbase_url = \"http://gw:9999\"\napi_key = \"k\"\n";
    let config = Config::from_toml_str(raw).expect("fixture parses");
    assert_eq!(config.gateway.base_url, "http://gw:9999");
}

#[test]
fn unclosed_interpolation_is_an_error() {
    let raw = "[gateway]\nbase_url = \"http://x\"\napi_key = \"${UNCLOSED\"\n";
    let err = Config::from_toml_str(raw).expect_err("unclosed interpolation must fail");
    assert!(
        err.to_string().starts_with("interpolation: "),
        "expected Interpolation, got {err:?}"
    );
}

#[test]
fn missing_gateway_section_is_an_error() {
    let err = Config::from_toml_str("[server]\nbind = \"127.0.0.1:9000\"\n")
        .expect_err("gateway section is required");
    assert!(
        matches!(err, ConfigError::Parse { .. }),
        "expected Parse, got {err:?}"
    );
}

#[test]
fn missing_file_names_the_expected_path() {
    let err = Config::load(Path::new("definitely-missing-workshop.toml"))
        .expect_err("missing file must fail");
    assert!(
        matches!(err, ConfigError::NotFound { .. }),
        "expected NotFound, got {err:?}"
    );
    assert!(
        err.to_string().contains("definitely-missing-workshop.toml"),
        "error names the path: {err}"
    );
}

#[test]
fn unreadable_existing_file_is_a_read_error() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("workshop.toml");
    std::fs::write(&path, "[gateway]\n").expect("write fixture");
    // A directory-shaped read failure: replace the file with a
    // directory of the same name so the read fails for a reason other
    // than NotFound.
    std::fs::remove_file(&path).expect("remove fixture");
    std::fs::create_dir(&path).expect("directory in the file's place");
    let err = Config::load(&path).expect_err("unreadable path must fail");
    assert!(
        matches!(err, ConfigError::Read { .. }),
        "expected Read, got {err:?}"
    );
}

#[test]
fn parse_error_names_the_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("broken-workshop.toml");
    std::fs::write(&path, "[gateway\n").expect("write fixture");
    let err = Config::load(&path).expect_err("malformed TOML must fail");
    assert!(
        matches!(err, ConfigError::Parse { .. }),
        "expected Parse, got {err:?}"
    );
    assert!(
        err.to_string().contains("broken-workshop.toml"),
        "error names the path: {err}"
    );
}

#[test]
fn load_reads_and_parses_a_file() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let path = dir.path().join("workshop.toml");
    std::fs::write(
        &path,
        "[gateway]\nbase_url = \"http://127.0.0.1:8081\"\napi_key = \"k\"\n",
    )
    .expect("write fixture");
    let config = Config::load(&path).expect("fixture loads");
    assert_eq!(config.gateway.api_key, "k");
}
