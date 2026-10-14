//! Tests for the config schema version, unknown-key rejection, and profile selection.

use std::fs;

use tempfile::TempDir;

use super::super::*;
use crate::{ConfigErrorKind, ProfileSelection, clear_profile_state, profile_state_path};

const CATALOG: &str = r#"
config-version = 0

[server]
bind = "127.0.0.1:8081"
api_key = "t"

[[endpoint]]
id = "e"
protocol = "openai"
base_url = "http://127.0.0.1:9"
api_key = ""

[[model]]
name = "cloud"
description = "a remote model"
context = 8192
upstream = "u"
endpoints = ["e"]

[[local_model]]
name = "a"
description = "model a"
source = "/models/a.gguf"
context = 4096

[[local_model]]
name = "b"
description = "model b"
source = "/models/b.gguf"
context = 4096

[[stt_model]]
name = "speech"
role = "interim"
source = "/models/base.en.bin"
vram_gb = 1.0

[[profile]]
name = "work"
models = ["a", "speech"]

[[profile]]
name = "travel"
models = ["b"]
"#;

fn file_fixture() -> (TempDir, std::path::PathBuf) {
    let temp = TempDir::new().expect("temp dir");
    let path = temp.path().join("gateway.toml");
    fs::write(&path, CATALOG).expect("write config");
    (temp, path)
}

#[test]
fn version_zero_schema_has_profiles_and_stt_models() {
    let config = Config::from_toml_str(CATALOG).expect("schema parses");

    assert_eq!(config.config_version(), 0);
    assert_eq!(config.profiles().len(), 2);
    assert_eq!(config.profiles()[0].name(), "work");
    assert_eq!(config.profiles()[0].models(), ["a", "speech"]);
    assert_eq!(config.catalog_stt_models()[0].name(), "speech");
    assert_eq!(config.catalog_stt_models()[0].role(), SttRole::Interim);
    assert!((config.catalog_stt_models()[0].vram_gb() - 1.0).abs() < f64::EPSILON);
}

#[test]
fn canonical_example_uses_the_validated_section_layout() {
    let example = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../gateway.local.example.toml"
    ));
    let catalog = Config::from_toml_str(example).expect("canonical example validates");
    let selected = catalog
        .select_profile(Some(
            &crate::ProfileName::parse("work").expect("profile name"),
        ))
        .expect("work profile selects");

    assert_eq!(selected.local_models()[0].name(), "qwen-local");
    assert_eq!(selected.stt_models().len(), 2);
}

#[test]
fn canonical_stt_section_parses_into_the_runtime_shape() {
    let config = Config::from_toml_str(&format!(
        "{CATALOG}\n[stt]\nwindow_seconds = 8\ninterval_ms = 250\nvocabulary = [\"WG21\"]\n"
    ))
    .expect("canonical STT section parses");

    let stt = config.stt().expect("canonical STT settings are present");
    assert_eq!(stt.window_seconds(), 8);
    assert_eq!(stt.interval_ms(), 250);
    assert_eq!(stt.vocabulary(), ["WG21"]);
    assert_eq!(stt.whisper_backend(), WhisperBackend::Auto);
}

/// The error's message followed by every source in its chain, so a test can
/// read the deserializer's detail behind the opaque wrapper.
fn error_chain(error: &crate::ConfigError) -> String {
    let mut chain = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(inner) = source {
        chain.push_str(": ");
        chain.push_str(&inner.to_string());
        source = inner.source();
    }
    chain
}

/// A minimal valid `[server]` section.
const SERVER: &str = "[server]\nbind = \"127.0.0.1:8081\"\napi_key = \"t\"\n";

#[test]
fn an_unknown_root_key_fails_to_parse_naming_the_key() {
    let error = Config::from_toml_str(&format!("config-version = 0\nextra = 1\n{SERVER}"))
        .expect_err("an unknown root key must fail");

    assert_eq!(error.kind(), ConfigErrorKind::Parse);
    let chain = error_chain(&error);
    assert!(chain.contains("extra"), "key named: {chain}");
}

#[test]
fn an_unknown_section_key_fails_to_parse_naming_the_key() {
    let error = Config::from_toml_str(&format!("config-version = 0\n{SERVER}extra = 1\n"))
        .expect_err("an unknown section key must fail");

    assert_eq!(error.kind(), ConfigErrorKind::Parse);
    let chain = error_chain(&error);
    assert!(chain.contains("extra"), "key named: {chain}");
}

#[test]
fn an_unknown_nested_table_key_fails_to_parse_naming_the_key() {
    let error = Config::from_toml_str(&format!(
        "config-version = 0\n{SERVER}\
         [tools.web_search]\nprovider = \"brave\"\napi_key = \"s\"\nextra = 1\n"
    ))
    .expect_err("an unknown nested table key must fail");

    assert_eq!(error.kind(), ConfigErrorKind::Parse);
    let chain = error_chain(&error);
    assert!(chain.contains("extra"), "key named: {chain}");
}

#[test]
fn a_missing_config_version_fails_to_parse_naming_the_key() {
    let error = Config::from_toml_str(SERVER).expect_err("the version is required");

    assert_eq!(error.kind(), ConfigErrorKind::Parse);
    let chain = error_chain(&error);
    assert!(chain.contains("config-version"), "key named: {chain}");
}

#[test]
fn a_config_version_other_than_zero_fails_validation_naming_required_and_actual() {
    let error = Config::from_toml_str(&format!("config-version = 1\n{SERVER}"))
        .expect_err("only version 0 is accepted");

    assert_eq!(error.kind(), ConfigErrorKind::Validation);
    assert!(
        error
            .to_string()
            .contains("config-version must be 0, got 1"),
        "required and actual named: {error}"
    );
}

#[test]
fn selection_precedence_is_cli_then_env_then_state() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"work\"\n").expect("write state");

    let state = Config::load(&path, &ProfileSelection::default()).expect("state selects");
    assert_eq!(state.active_profile().expect("active").name(), "work");
    assert_eq!(state.local_models()[0].name(), "a");
    assert_eq!(state.stt_models()[0].name(), "speech");
    assert_eq!(state.models()[0].name(), "cloud");
    assert_eq!(state.stale_state_selection(), None);

    let environment =
        Config::load(&path, &ProfileSelection::new(None, Some("travel"))).expect("env selects");
    assert_eq!(
        environment.active_profile().expect("active").name(),
        "travel"
    );
    assert_eq!(environment.local_models()[0].name(), "b");

    let command_line = Config::load(&path, &ProfileSelection::new(Some("work"), Some("travel")))
        .expect("cli selects");
    assert_eq!(
        command_line.active_profile().expect("active").name(),
        "work"
    );
}

#[test]
fn stale_state_file_loads_with_no_profile_and_records_the_name() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"deleted\"\n").expect("write state");

    let config = Config::load(&path, &ProfileSelection::default())
        .expect("a stale state file degrades instead of refusing");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), Some("deleted"));
    assert!(config.local_models().is_empty());
    assert!(config.stt_models().is_empty());
    assert_eq!(config.models()[0].name(), "cloud");
}

#[test]
fn undefined_command_line_or_environment_profile_still_refuses() {
    let (_temp, path) = file_fixture();

    for inputs in [
        ProfileSelection::new(Some("x"), None),
        ProfileSelection::new(None, Some("x")),
    ] {
        let error = Config::load(&path, &inputs).expect_err("an ephemeral selection must exist");
        let message = error.to_string();
        assert_eq!(error.kind(), ConfigErrorKind::Validation);
        assert!(
            message.contains("active profile x is not defined"),
            "undefined name named: {message}"
        );
        assert!(
            message.contains("work, travel"),
            "defined profiles named: {message}"
        );
    }
}

#[test]
fn malformed_state_selection_still_refuses() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"../work\"\n").expect("write state");

    let error = Config::load(&path, &ProfileSelection::default())
        .expect_err("a malformed state name is not a stale selection");

    assert_eq!(error.kind(), ConfigErrorKind::Validation);
    assert!(error.to_string().contains("../work"));
}

#[test]
fn absent_selection_loads_with_no_profile_and_every_remote_model() {
    let (_temp, path) = file_fixture();

    let config = Config::load(&path, &ProfileSelection::default())
        .expect("no selection is a supported startup state");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), None);
    assert!(config.local_models().is_empty());
    assert!(config.stt_models().is_empty());
    assert_eq!(config.models().len(), 1);
    assert_eq!(config.models()[0].name(), "cloud");
    assert_eq!(config.catalog_local_models().len(), 2);
    assert_eq!(config.catalog_stt_models().len(), 1);
}

#[test]
fn an_in_memory_document_has_no_active_profile_or_stale_selection() {
    let config = Config::from_toml_str(CATALOG).expect("catalog parses");

    assert!(config.active_profile().is_none());
    assert_eq!(config.stale_state_selection(), None);
}

#[test]
fn clear_profile_state_deletes_the_state_file_and_tolerates_absence() {
    let (_temp, path) = file_fixture();
    let state_path = profile_state_path(&path);
    fs::write(&state_path, "active_profile = \"work\"\n").expect("write state");

    clear_profile_state(&path).expect("present state clears");
    assert!(!state_path.exists(), "the state file is deleted");

    clear_profile_state(&path).expect("absent state is already clear");
    assert!(!state_path.exists());
}

#[test]
fn clear_profile_state_reports_a_failed_removal_as_a_removal() {
    let (_temp, path) = file_fixture();
    let state_path = profile_state_path(&path);
    fs::create_dir(&state_path).expect("occupy the state path with a directory");

    let error = clear_profile_state(&path).expect_err("a directory is not removable as a file");

    assert_eq!(error.kind(), ConfigErrorKind::Write);
    let mut chain = error.to_string();
    let mut source = std::error::Error::source(&error);
    while let Some(inner) = source {
        chain.push_str(": ");
        chain.push_str(&inner.to_string());
        source = inner.source();
    }
    assert!(
        chain.contains("remove state file"),
        "the operator reads a removal, not a write: {chain}"
    );
    assert!(
        chain.contains(&state_path.display().to_string()),
        "the failing path is named: {chain}"
    );
}

#[test]
fn selecting_a_profile_clears_a_stale_state_selection() {
    let (_temp, path) = file_fixture();
    fs::write(profile_state_path(&path), "active_profile = \"deleted\"\n").expect("write state");
    let stale = Config::load(&path, &ProfileSelection::default()).expect("stale load degrades");
    assert_eq!(stale.stale_state_selection(), Some("deleted"));

    let work = crate::ProfileName::parse("work").expect("profile name");
    let selected = stale.select_profile(Some(&work)).expect("work selects");
    assert_eq!(selected.active_profile().expect("active").name(), "work");
    assert_eq!(
        selected.stale_state_selection(),
        None,
        "a fresh selection supersedes the stale name"
    );

    let unselected = stale.select_profile(None).expect("no profile selects");
    assert!(unselected.active_profile().is_none());
    assert_eq!(unselected.stale_state_selection(), None);
}
